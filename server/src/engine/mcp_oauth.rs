//! Secure browser OAuth runtime for downstream HTTP MCP servers.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{LOCATION, WWW_AUTHENTICATE};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use rmcp::transport::auth::{
    AuthError, AuthorizationManager, AuthorizationMetadata, CredentialStore, OAuthClientConfig,
    StoredCredentials,
};

const OAUTH_BROWSER_TIMEOUT: Duration = Duration::from_secs(300);
const CALLBACK_READ_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_DISCOVERY_REDIRECTS: usize = 5;

pub(crate) async fn resolve_browser_oauth(
    server_name: &str,
    server_url: &str,
    scope: Option<&[String]>,
    client_id: Option<&str>,
    client_secret: Option<&str>,
    redirect_port: Option<u16>,
    token_cache: Option<&str>,
) -> Result<String, String> {
    resolve_browser_oauth_with_opener(
        server_name,
        server_url,
        scope,
        client_id,
        client_secret,
        redirect_port,
        token_cache,
        open_browser,
    )
    .await
}

async fn resolve_browser_oauth_with_opener<F>(
    server_name: &str,
    server_url: &str,
    scope: Option<&[String]>,
    client_id: Option<&str>,
    client_secret: Option<&str>,
    redirect_port: Option<u16>,
    token_cache: Option<&str>,
    open: F,
) -> Result<String, String>
where
    F: FnOnce(&str) -> std::io::Result<()>,
{
    validate_oauth_endpoint(server_url, "protected resource URL")?;
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| oauth_failure(server_name, "HTTP client setup"))?;
    let discovered = discover_authorization(server_url, &http_client)
        .await
        .map_err(|_| oauth_failure(server_name, "metadata discovery"))?;
    let cache_path = token_cache
        .map(PathBuf::from)
        .unwrap_or_else(|| default_token_cache_path(server_name));
    let scopes = scope.unwrap_or_default();
    let scope_refs: Vec<&str> = scopes.iter().map(String::as_str).collect();
    let binding = CacheBinding::new(server_url, scope, client_id, client_secret, &discovered);
    let store = FileCredentialStore::new(cache_path, binding);

    let mut manager = AuthorizationManager::new(server_url)
        .await
        .map_err(|_| oauth_failure(server_name, "manager initialization"))?;
    manager.set_metadata(discovered.metadata.clone());
    manager.set_credential_store(store.clone());

    let cached_client = if let Some(client_id) = client_id {
        Some(CachedClientRegistration {
            client_id: client_id.to_string(),
            client_secret: client_secret.map(str::to_string),
            redirect_uri: "http://localhost".to_string(),
            scopes: scopes.to_vec(),
        })
    } else {
        None
    };
    if let Some(token) =
        resolve_cached_token(&store, cached_client, &discovered.metadata, &http_client)
            .await
            .map_err(|_| oauth_failure(server_name, "cached token retrieval"))?
    {
        return Ok(token);
    }

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", redirect_port.unwrap_or(0)))
        .await
        .map_err(|_| oauth_failure(server_name, "callback listener setup"))?;
    let port = listener
        .local_addr()
        .map_err(|_| oauth_failure(server_name, "callback listener setup"))?
        .port();
    let redirect_uri = format!("http://localhost:{port}/callback");

    match client_id {
        Some(client_id) => configure_static_client(
            &mut manager,
            client_id,
            client_secret,
            &redirect_uri,
            scopes,
        )
        .map_err(|_| oauth_failure(server_name, "client configuration"))?,
        None => {
            let registration = register_dynamic_client(
                &discovered.metadata,
                &http_client,
                "mcp-js",
                &redirect_uri,
                &scope_refs,
            )
            .await
            .map_err(|_| oauth_failure(server_name, "dynamic client registration"))?;
            manager
                .configure_client(registration.clone())
                .map_err(|_| oauth_failure(server_name, "dynamic client configuration"))?;
            store.set_registration(CachedClientRegistration::from_oauth_config(&registration));
        }
    }

    let authorization_url = manager
        .get_authorization_url(&scope_refs)
        .await
        .map_err(|_| oauth_failure(server_name, "authorization URL construction"))?;
    let expected_state = url::Url::parse(&authorization_url)
        .ok()
        .and_then(|url| {
            url.query_pairs()
                .find(|(key, _)| key == "state")
                .map(|(_, value)| value.into_owned())
        })
        .ok_or_else(|| oauth_failure(server_name, "authorization state generation"))?;

    println!("[mcp-js] Authorize '{server_name}' by opening:\n  {authorization_url}");
    if open(&authorization_url).is_err() {
        tracing::warn!(server = %server_name, "Could not open browser; use the printed authorization URL");
    }

    let code = tokio::time::timeout(
        OAUTH_BROWSER_TIMEOUT,
        await_authorization_code(listener, &expected_state),
    )
    .await
    .map_err(|_| oauth_failure(server_name, "browser authorization timeout"))??;

    manager
        .exchange_code_for_token(&code, &expected_state)
        .await
        .map_err(|_| oauth_failure(server_name, "authorization code exchange"))?;
    manager
        .get_access_token()
        .await
        .map_err(|_| oauth_failure(server_name, "access token retrieval"))
}

#[derive(Clone)]
struct DiscoveredAuthorization {
    authorization_server: String,
    metadata: AuthorizationMetadata,
}

#[derive(Deserialize)]
struct ResourceServerMetadata {
    #[serde(default)]
    authorization_server: Option<String>,
    #[serde(default)]
    authorization_servers: Vec<String>,
}

async fn discover_authorization(
    protected_resource: &str,
    client: &reqwest::Client,
) -> Result<DiscoveredAuthorization, String> {
    let base = url::Url::parse(protected_resource)
        .map_err(|_| "OAuth protected resource URL is invalid".to_string())?;
    let mut resource_metadata_url = None;

    let response = safe_get(client, base.clone(), "protected resource URL").await?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        resource_metadata_url = response
            .headers()
            .get_all(WWW_AUTHENTICATE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find_map(|header| extract_resource_metadata_url(header, &base));
    } else if response.status().is_success() {
        if let Ok(metadata) = response.json::<ResourceServerMetadata>().await {
            if let Some(found) = discover_from_resource_metadata(metadata, client).await? {
                return Ok(found);
            }
        }
    }

    if let Some(resource_metadata_url) = resource_metadata_url {
        validate_oauth_url(&resource_metadata_url, "protected-resource metadata URL")?;
        if let Some(metadata) = fetch_json::<ResourceServerMetadata>(
            client,
            resource_metadata_url,
            "protected-resource metadata URL",
        )
        .await?
        {
            if let Some(found) = discover_from_resource_metadata(metadata, client).await? {
                return Ok(found);
            }
        }
    }

    for path in well_known_paths(base.path(), "oauth-protected-resource") {
        let mut candidate = base.clone();
        candidate.set_query(None);
        candidate.set_fragment(None);
        candidate.set_path(&path);
        if let Some(metadata) = fetch_json::<ResourceServerMetadata>(
            client,
            candidate,
            "protected-resource metadata URL",
        )
        .await?
        {
            if let Some(found) = discover_from_resource_metadata(metadata, client).await? {
                return Ok(found);
            }
        }
    }

    discover_authorization_server(&base, client)
        .await?
        .ok_or_else(|| "OAuth authorization metadata was not found".to_string())
}

async fn discover_from_resource_metadata(
    metadata: ResourceServerMetadata,
    client: &reqwest::Client,
) -> Result<Option<DiscoveredAuthorization>, String> {
    let mut candidates = Vec::new();
    if let Some(candidate) = metadata.authorization_server {
        candidates.push(candidate);
    }
    candidates.extend(metadata.authorization_servers);
    for candidate in candidates {
        let url = url::Url::parse(&candidate)
            .map_err(|_| "OAuth authorization server URL is invalid".to_string())?;
        validate_oauth_url(&url, "authorization server URL")?;
        if let Some(found) = discover_authorization_server(&url, client).await? {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

async fn discover_authorization_server(
    authorization_server: &url::Url,
    client: &reqwest::Client,
) -> Result<Option<DiscoveredAuthorization>, String> {
    validate_oauth_url(authorization_server, "authorization server URL")?;
    for candidate in authorization_discovery_urls(authorization_server) {
        if let Some(metadata) = fetch_json::<AuthorizationMetadata>(
            client,
            candidate,
            "authorization-server metadata URL",
        )
        .await?
        {
            validate_authorization_metadata(&metadata)?;
            return Ok(Some(DiscoveredAuthorization {
                authorization_server: authorization_server
                    .as_str()
                    .trim_end_matches('/')
                    .to_string(),
                metadata,
            }));
        }
    }
    Ok(None)
}

async fn resolve_cached_token(
    store: &FileCredentialStore,
    static_client: Option<CachedClientRegistration>,
    metadata: &AuthorizationMetadata,
    client: &reqwest::Client,
) -> Result<Option<String>, AuthError> {
    let Some(credentials) = store.load().await? else {
        return Ok(None);
    };
    let client_config = static_client.or_else(|| store.loaded_registration());
    let Some(client_config) = client_config else {
        return Ok(None);
    };
    let token = serde_json::to_value(&credentials)
        .map_err(|_| AuthError::InternalError("token cache serialization error".to_string()))?;
    let Some(access_token) = token
        .pointer("/token_response/access_token")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(None);
    };
    let expires_in = token
        .pointer("/token_response/expires_in")
        .and_then(serde_json::Value::as_u64);
    let expired = expires_in.is_some_and(|expires_in| {
        let received_at = credentials.token_received_at.unwrap_or(0);
        let elapsed = now_epoch_secs().saturating_sub(received_at);
        elapsed.saturating_add(30) >= expires_in
    });
    if !expired {
        return Ok(Some(access_token.to_string()));
    }
    let Some(refresh_token) = token
        .pointer("/token_response/refresh_token")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(None);
    };

    refresh_cached_token(
        store,
        &credentials,
        &client_config,
        metadata,
        client,
        refresh_token,
    )
    .await
}

async fn refresh_cached_token(
    store: &FileCredentialStore,
    credentials: &StoredCredentials,
    client_config: &CachedClientRegistration,
    metadata: &AuthorizationMetadata,
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<Option<String>, AuthError> {
    let mut url = url::Url::parse(&metadata.token_endpoint)
        .map_err(|_| AuthError::InternalError("invalid token endpoint".to_string()))?;
    let mut form = vec![
        ("grant_type".to_string(), "refresh_token".to_string()),
        ("refresh_token".to_string(), refresh_token.to_string()),
    ];
    if !credentials.granted_scopes.is_empty() {
        form.push(("scope".to_string(), credentials.granted_scopes.join(" ")));
    }
    if client_config.client_secret.is_none() {
        form.push(("client_id".to_string(), client_config.client_id.clone()));
    }

    for _ in 0..=MAX_DISCOVERY_REDIRECTS {
        validate_oauth_url(&url, "token endpoint").map_err(AuthError::InternalError)?;
        let mut request = client.post(url.clone()).form(&form);
        if let Some(secret) = &client_config.client_secret {
            request = request.basic_auth(&client_config.client_id, Some(secret));
        }
        let response = match request.send().await {
            Ok(response) => response,
            Err(_) => return Ok(None),
        };
        if response.status().is_redirection() {
            let Some(location) = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
            else {
                return Ok(None);
            };
            url = url
                .join(location)
                .map_err(|_| AuthError::InternalError("invalid token redirect".to_string()))?;
            validate_oauth_url(&url, "token endpoint").map_err(AuthError::InternalError)?;
            continue;
        }
        if !response.status().is_success() {
            return Ok(None);
        }
        let mut token_response = response
            .json::<serde_json::Value>()
            .await
            .map_err(|_| AuthError::InternalError("invalid token response".to_string()))?;
        if token_response
            .get("refresh_token")
            .and_then(serde_json::Value::as_str)
            .is_none()
        {
            token_response["refresh_token"] = serde_json::json!(refresh_token);
        }
        let access_token = token_response
            .get("access_token")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| AuthError::InternalError("missing access token".to_string()))?
            .to_string();
        let stored: StoredCredentials = serde_json::from_value(serde_json::json!({
            "client_id": client_config.client_id,
            "token_response": token_response,
            "granted_scopes": credentials.granted_scopes,
            "token_received_at": now_epoch_secs(),
        }))
        .map_err(|_| AuthError::InternalError("invalid token response".to_string()))?;
        store.save(stored).await?;
        return Ok(Some(access_token));
    }
    Ok(None)
}

fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Deserialize)]
struct DynamicClientRegistrationResponse {
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
}

async fn register_dynamic_client(
    metadata: &AuthorizationMetadata,
    client: &reqwest::Client,
    client_name: &str,
    redirect_uri: &str,
    scopes: &[&str],
) -> Result<OAuthClientConfig, String> {
    let endpoint = metadata.registration_endpoint.as_deref().ok_or_else(|| {
        "OAuth authorization server does not support dynamic registration".to_string()
    })?;
    let mut url = url::Url::parse(endpoint)
        .map_err(|_| "OAuth registration endpoint is invalid".to_string())?;
    let body = serde_json::json!({
        "client_name": client_name,
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_method": "none",
        "response_types": ["code"],
        "scope": scopes.join(" "),
    });
    for _ in 0..=MAX_DISCOVERY_REDIRECTS {
        validate_oauth_url(&url, "registration endpoint")?;
        let response = client
            .post(url.clone())
            .json(&body)
            .send()
            .await
            .map_err(|_| "OAuth dynamic registration request failed".to_string())?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| "OAuth registration redirect is missing Location".to_string())?;
            url = url
                .join(location)
                .map_err(|_| "OAuth registration redirect URL is invalid".to_string())?;
            validate_oauth_url(&url, "registration endpoint")?;
            continue;
        }
        if !response.status().is_success() {
            return Err("OAuth dynamic registration was rejected".to_string());
        }
        let response = response
            .json::<DynamicClientRegistrationResponse>()
            .await
            .map_err(|_| "OAuth dynamic registration response is invalid".to_string())?;
        let mut config = OAuthClientConfig::new(response.client_id, redirect_uri)
            .with_scopes(scopes.iter().map(|scope| (*scope).to_string()).collect());
        if let Some(secret) = response.client_secret.filter(|secret| !secret.is_empty()) {
            config = config.with_client_secret(secret);
        }
        return Ok(config);
    }
    Err("OAuth registration exceeded redirect limit".to_string())
}

async fn fetch_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: url::Url,
    label: &str,
) -> Result<Option<T>, String> {
    let response = safe_get(client, url, label).await?;
    if response.status() != reqwest::StatusCode::OK {
        return Ok(None);
    }
    Ok(response.json::<T>().await.ok())
}

async fn safe_get(
    client: &reqwest::Client,
    mut url: url::Url,
    label: &str,
) -> Result<reqwest::Response, String> {
    for _ in 0..=MAX_DISCOVERY_REDIRECTS {
        validate_oauth_url(&url, label)?;
        let response = client
            .get(url.clone())
            .header("MCP-Protocol-Version", "2024-11-05")
            .send()
            .await
            .map_err(|_| format!("OAuth {label} request failed"))?;
        if !response.status().is_redirection() {
            return Ok(response);
        }
        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| format!("OAuth {label} redirect is missing Location"))?;
        url = url
            .join(location)
            .map_err(|_| format!("OAuth {label} redirect URL is invalid"))?;
        validate_oauth_url(&url, label)?;
    }
    Err(format!("OAuth {label} exceeded redirect limit"))
}

fn extract_resource_metadata_url(header: &str, base: &url::Url) -> Option<url::Url> {
    let lower = header.to_ascii_lowercase();
    let start = lower.find("resource_metadata=")? + "resource_metadata=".len();
    let remainder = header[start..].trim_start();
    let value = if let Some(quoted) = remainder.strip_prefix('"') {
        quoted.split('"').next()?
    } else {
        remainder.split([',', ' ']).next()?
    };
    url::Url::parse(value).or_else(|_| base.join(value)).ok()
}

fn well_known_paths(base_path: &str, resource: &str) -> Vec<String> {
    let trimmed = base_path.trim_matches('/');
    let canonical = format!("/.well-known/{resource}");
    if trimmed.is_empty() {
        vec![canonical]
    } else {
        vec![
            format!("{canonical}/{trimmed}"),
            format!("/{trimmed}/.well-known/{resource}"),
            canonical,
        ]
    }
}

fn authorization_discovery_urls(base: &url::Url) -> Vec<url::Url> {
    let trimmed = base.path().trim_matches('/');
    let paths = if trimmed.is_empty() {
        vec![
            "/.well-known/oauth-authorization-server".to_string(),
            "/.well-known/openid-configuration".to_string(),
        ]
    } else {
        vec![
            format!("/.well-known/oauth-authorization-server/{trimmed}"),
            format!("/.well-known/openid-configuration/{trimmed}"),
            format!("/{trimmed}/.well-known/openid-configuration"),
            "/.well-known/oauth-authorization-server".to_string(),
        ]
    };
    paths
        .into_iter()
        .map(|path| {
            let mut candidate = base.clone();
            candidate.set_query(None);
            candidate.set_fragment(None);
            candidate.set_path(&path);
            candidate
        })
        .collect()
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CacheBinding {
    version: u8,
    fingerprint: String,
}

impl CacheBinding {
    fn new(
        protected_resource: &str,
        requested_scopes: Option<&[String]>,
        client_id: Option<&str>,
        client_secret: Option<&str>,
        discovered: &DiscoveredAuthorization,
    ) -> Self {
        let mut scopes = requested_scopes.unwrap_or_default().to_vec();
        scopes.sort();
        scopes.dedup();
        let payload = serde_json::json!({
            "version": 2,
            "protected_resource": protected_resource,
            "requested_scopes": scopes,
            "client_id": client_id,
            "client_secret": client_secret,
            "authorization_server": discovered.authorization_server,
            "issuer": discovered.metadata.issuer,
            "authorization_endpoint": discovered.metadata.authorization_endpoint,
            "token_endpoint": discovered.metadata.token_endpoint,
            "registration_endpoint": discovered.metadata.registration_endpoint,
        });
        let mut hasher = Sha256::new();
        hasher.update(serde_json::to_vec(&payload).expect("cache binding is serializable"));
        Self {
            version: 2,
            fingerprint: format!("{:x}", hasher.finalize()),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedClientRegistration {
    client_id: String,
    client_secret: Option<String>,
    redirect_uri: String,
    scopes: Vec<String>,
}

impl CachedClientRegistration {
    fn from_oauth_config(config: &OAuthClientConfig) -> Self {
        Self {
            client_id: config.client_id.clone(),
            client_secret: config.client_secret.clone(),
            redirect_uri: config.redirect_uri.clone(),
            scopes: config.scopes.clone(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct DiskTokenCache {
    binding: CacheBinding,
    credentials: StoredCredentials,
    #[serde(default)]
    client_registration: Option<CachedClientRegistration>,
}

#[derive(Clone)]
struct FileCredentialStore {
    path: PathBuf,
    binding: CacheBinding,
    registration: Arc<Mutex<Option<CachedClientRegistration>>>,
}

impl FileCredentialStore {
    fn new(path: PathBuf, binding: CacheBinding) -> Self {
        Self {
            path,
            binding,
            registration: Arc::new(Mutex::new(None)),
        }
    }

    fn set_registration(&self, registration: CachedClientRegistration) {
        *self
            .registration
            .lock()
            .expect("registration lock poisoned") = Some(registration);
    }

    fn loaded_registration(&self) -> Option<CachedClientRegistration> {
        self.registration
            .lock()
            .expect("registration lock poisoned")
            .clone()
    }
}

impl std::fmt::Debug for FileCredentialStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileCredentialStore")
            .field("path", &self.path)
            .finish()
    }
}

#[async_trait]
impl CredentialStore for FileCredentialStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        validate_cache_file(&self.path)?;
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                let cache: DiskTokenCache = serde_json::from_slice(&bytes)
                    .map_err(|_| AuthError::InternalError("token cache parse error".to_string()))?;
                if cache.binding != self.binding {
                    return Ok(None);
                }
                *self
                    .registration
                    .lock()
                    .expect("registration lock poisoned") = cache.client_registration;
                Ok(Some(cache.credentials))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(AuthError::InternalError(
                "token cache read error".to_string(),
            )),
        }
    }

    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        let bytes = serde_json::to_vec_pretty(&DiskTokenCache {
            binding: self.binding.clone(),
            credentials,
            client_registration: self.loaded_registration(),
        })
        .map_err(|_| AuthError::InternalError("token cache serialization error".to_string()))?;
        write_private_file(&self.path, &bytes)
            .map_err(|_| AuthError::InternalError("token cache write error".to_string()))
    }

    async fn clear(&self) -> Result<(), AuthError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(AuthError::InternalError(
                "token cache clear error".to_string(),
            )),
        }
    }
}

pub(crate) fn invalidate_cached_access_token(
    server_name: &str,
    token_cache: Option<&str>,
) -> Result<(), String> {
    let path = token_cache
        .map(PathBuf::from)
        .unwrap_or_else(|| default_token_cache_path(server_name));
    validate_cache_file(&path).map_err(|_| oauth_failure(server_name, "token cache validation"))?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(oauth_failure(server_name, "token cache read")),
    };
    let mut cache: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| oauth_failure(server_name, "token cache parse"))?;
    cache["credentials"]["token_received_at"] = serde_json::json!(0);
    cache["credentials"]["token_response"]["expires_in"] = serde_json::json!(0);
    let bytes = serde_json::to_vec_pretty(&cache)
        .map_err(|_| oauth_failure(server_name, "token cache serialization"))?;
    write_private_file(&path, &bytes).map_err(|_| oauth_failure(server_name, "token cache write"))
}

fn configure_static_client(
    manager: &mut AuthorizationManager,
    client_id: &str,
    client_secret: Option<&str>,
    redirect_uri: &str,
    scopes: &[String],
) -> Result<(), AuthError> {
    let mut config = OAuthClientConfig::new(client_id, redirect_uri).with_scopes(scopes.to_vec());
    if let Some(client_secret) = client_secret {
        config = config.with_client_secret(client_secret);
    }
    manager.configure_client(config)
}

fn validate_oauth_endpoint(endpoint: &str, label: &str) -> Result<(), String> {
    let url = url::Url::parse(endpoint).map_err(|_| format!("OAuth {label} is not a valid URL"))?;
    validate_oauth_url(&url, label)
}

fn validate_oauth_url(url: &url::Url, label: &str) -> Result<(), String> {
    if url.scheme() == "https" {
        return Ok(());
    }
    let loopback = matches!(url.host_str(), Some("localhost"))
        || url
            .host_str()
            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
            .is_some_and(|address| address.is_loopback());
    if url.scheme() == "http" && loopback {
        return Ok(());
    }
    Err(format!(
        "OAuth {label} must use HTTPS unless it is loopback"
    ))
}

fn validate_authorization_metadata(metadata: &AuthorizationMetadata) -> Result<(), String> {
    if let Some(issuer) = &metadata.issuer {
        validate_oauth_endpoint(issuer, "issuer URL")?;
    }
    validate_oauth_endpoint(&metadata.authorization_endpoint, "authorization endpoint")?;
    validate_oauth_endpoint(&metadata.token_endpoint, "token endpoint")?;
    if let Some(registration_endpoint) = &metadata.registration_endpoint {
        validate_oauth_endpoint(registration_endpoint, "registration endpoint")?;
    }
    Ok(())
}

fn validate_cache_file(path: &Path) -> Result<(), AuthError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(AuthError::InternalError(
                "token cache metadata error".to_string(),
            ));
        }
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(AuthError::InternalError(
            "token cache is not a regular file".to_string(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(AuthError::InternalError(
                "token cache owner mismatch".to_string(),
            ));
        }
        if metadata.mode() & 0o077 != 0 {
            return Err(AuthError::InternalError(
                "token cache permissions are too broad".to_string(),
            ));
        }
    }
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }

    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);

    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn default_token_cache_path(server_name: &str) -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    default_token_cache_path_from(&base, server_name)
}

fn default_token_cache_path_from(base: &Path, server_name: &str) -> PathBuf {
    let safe_name: String = server_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    base.join("mcp-js").join(format!("oauth-{safe_name}.json"))
}

fn open_browser(url: &str) -> std::io::Result<()> {
    use std::process::Stdio;

    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    };
    command
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|_| ())
}

async fn await_authorization_code(
    listener: tokio::net::TcpListener,
    expected_state: &str,
) -> Result<String, String> {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.map_err(|_| "OAuth callback listener failed".to_string())?;
                let sender = sender.clone();
                let expected_state = expected_state.to_string();
                tokio::spawn(async move {
                    if let Some(result) = handle_callback(stream, &expected_state).await {
                        let _ = sender.send(result);
                    }
                });
            }
            result = receiver.recv() => {
                return result.ok_or_else(|| "OAuth callback listener failed".to_string())?;
            }
        }
    }
}

async fn handle_callback(
    mut stream: tokio::net::TcpStream,
    expected_state: &str,
) -> Option<Result<String, String>> {
    use tokio::io::AsyncReadExt;

    let mut buffer = [0_u8; 8192];
    let Ok(Ok(size)) = tokio::time::timeout(CALLBACK_READ_TIMEOUT, stream.read(&mut buffer)).await
    else {
        return None;
    };
    let request = String::from_utf8_lossy(&buffer[..size]);
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("");
    let callback = parse_callback_target(target);

    if callback.state.as_deref() != Some(expected_state) {
        let _ = write_callback_response(&mut stream, "Waiting", "Waiting for authorization.").await;
        return None;
    }
    if callback.error.is_some() {
        let _ = write_callback_response(
            &mut stream,
            "Authorization failed",
            "The authorization server rejected the request.",
        )
        .await;
        return Some(Err(
            "OAuth authorization server returned an error".to_string()
        ));
    }
    let Some(code) = callback.code else {
        let _ = write_callback_response(&mut stream, "Waiting", "Waiting for authorization.").await;
        return None;
    };

    let _ = write_callback_response(
        &mut stream,
        "Authorized",
        "Authorization complete. You can close this tab.",
    )
    .await;
    Some(Ok(code))
}

struct CallbackParameters {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

fn parse_callback_target(target: &str) -> CallbackParameters {
    let Ok(url) = url::Url::parse(&format!("http://localhost{target}")) else {
        return CallbackParameters {
            code: None,
            state: None,
            error: None,
        };
    };
    if url.path() != "/callback" {
        return CallbackParameters {
            code: None,
            state: None,
            error: None,
        };
    }
    let mut result = CallbackParameters {
        code: None,
        state: None,
        error: None,
    };
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => result.code = Some(value.into_owned()),
            "state" => result.state = Some(value.into_owned()),
            "error" => result.error = Some(value.into_owned()),
            _ => {}
        }
    }
    result
}

async fn write_callback_response(
    stream: &mut tokio::net::TcpStream,
    title: &str,
    message: &str,
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;

    let body = format!("<!doctype html><title>{title}</title><h1>{title}</h1><p>{message}</p>");
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await
}

fn oauth_failure(server_name: &str, action: &str) -> String {
    format!(
        "MCP server '{}': OAuth {} failed",
        redact_oauth_error(server_name),
        redact_oauth_error(action)
    )
}

fn redact_oauth_error(message: &str) -> String {
    let mut redacted = message.to_string();
    for key in ["client_secret", "refresh_token", "access_token", "code"] {
        let pattern = format!("{key}=");
        let mut cursor = 0;
        while let Some(offset) = redacted[cursor..].find(&pattern) {
            let value_start = cursor + offset + pattern.len();
            let value_end = redacted[value_start..]
                .find(['&', ' ', '\n', '\r'])
                .map(|offset| value_start + offset)
                .unwrap_or(redacted.len());
            redacted.replace_range(value_start..value_end, "[REDACTED]");
            cursor = value_start + "[REDACTED]".len();
        }
    }
    redacted
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::Mutex;

    use super::*;

    #[derive(Clone, Copy)]
    enum TokenMode {
        Refreshes,
        RejectsRefresh,
    }

    struct TestOAuthServer {
        url: String,
        calls: Arc<Mutex<Vec<String>>>,
        requests: Arc<Mutex<Vec<String>>>,
        task: tokio::task::JoinHandle<()>,
    }

    impl TestOAuthServer {
        async fn start(mode: TokenMode) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let calls = Arc::new(Mutex::new(Vec::new()));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let task_calls = calls.clone();
            let task_requests = requests.clone();
            let task_url = url.clone();
            let task = tokio::spawn(async move {
                loop {
                    let Ok((mut stream, _)) = listener.accept().await else {
                        break;
                    };
                    let calls = task_calls.clone();
                    let requests = task_requests.clone();
                    let url = task_url.clone();
                    tokio::spawn(async move {
                        let mut buffer = [0_u8; 8192];
                        let size = stream.read(&mut buffer).await.unwrap_or(0);
                        let request = String::from_utf8_lossy(&buffer[..size]).to_string();
                        let line = request.lines().next().unwrap_or_default().to_string();
                        calls.lock().await.push(line.clone());
                        requests.lock().await.push(request.clone());
                        let (status, body) = if line.contains("oauth-protected-resource") {
                            (
                                "200 OK",
                                format!(r#"{{"authorization_servers":["{url}"]}}"#),
                            )
                        } else if line.contains("oauth-authorization-server") {
                            (
                                "200 OK",
                                format!(
                                    r#"{{"authorization_endpoint":"{url}/authorize","token_endpoint":"{url}/token","response_types_supported":["code"],"code_challenge_methods_supported":["S256"]}}"#
                                ),
                            )
                        } else if line.starts_with("POST /token")
                            && request.contains("grant_type=refresh_token")
                        {
                            match mode {
                                TokenMode::Refreshes => ("200 OK", r#"{"access_token":"refreshed-access","token_type":"Bearer","refresh_token":"rotated-refresh","expires_in":3600}"#.to_string()),
                                TokenMode::RejectsRefresh => ("400 Bad Request", r#"{"error":"invalid_grant","error_description":"refresh-secret"}"#.to_string()),
                            }
                        } else if line.starts_with("POST /token") {
                            ("200 OK", r#"{"access_token":"browser-access","token_type":"Bearer","refresh_token":"browser-refresh","expires_in":3600}"#.to_string())
                        } else {
                            ("404 Not Found", "{}".to_string())
                        };
                        let response = format!(
                            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                    });
                }
            });
            Self {
                url,
                calls,
                requests,
                task,
            }
        }
    }

    impl Drop for TestOAuthServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    fn credentials(
        access_token: &str,
        refresh_token: Option<&str>,
        expires_in: u64,
        received_at: u64,
    ) -> StoredCredentials {
        serde_json::from_value(serde_json::json!({
            "client_id": "client-id",
            "token_response": {
                "access_token": access_token,
                "token_type": "Bearer",
                "refresh_token": refresh_token,
                "expires_in": expires_in
            },
            "granted_scopes": ["calendar.read"],
            "token_received_at": received_at
        }))
        .unwrap()
    }

    fn test_discovered(server_url: &str) -> DiscoveredAuthorization {
        let authorization_server = server_url.trim_end_matches("/mcp").to_string();
        DiscoveredAuthorization {
            authorization_server: authorization_server.clone(),
            metadata: serde_json::from_value(serde_json::json!({
                "authorization_endpoint": format!("{authorization_server}/authorize"),
                "token_endpoint": format!("{authorization_server}/token"),
                "response_types_supported": ["code"],
                "code_challenge_methods_supported": ["S256"]
            }))
            .unwrap(),
        }
    }

    async fn save_cache(path: PathBuf, credentials: StoredCredentials) {
        FileCredentialStore::new(
            path,
            CacheBinding::new(
                "https://calendar.example.com/mcp",
                None,
                None,
                None,
                &test_discovered("https://calendar.example.com/mcp"),
            ),
        )
        .save(credentials)
        .await
        .unwrap();
    }

    async fn save_bound_cache(
        path: PathBuf,
        server_url: &str,
        client_secret: Option<&str>,
        credentials: StoredCredentials,
    ) {
        FileCredentialStore::new(
            path,
            CacheBinding::new(
                server_url,
                None,
                Some("client-id"),
                client_secret,
                &test_discovered(server_url),
            ),
        )
        .save(credentials)
        .await
        .unwrap();
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn derives_a_safe_cache_path_from_the_server_name() {
        assert_eq!(
            default_token_cache_path_from(Path::new("/cache"), "calendar/prod"),
            Path::new("/cache/mcp-js/oauth-calendar_prod.json")
        );
    }

    #[tokio::test]
    async fn creates_owner_only_token_cache_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tokens.json");
        save_cache(
            path.clone(),
            credentials("access-secret", Some("refresh-secret"), 3600, now()),
        )
        .await;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[tokio::test]
    async fn valid_cached_token_is_reused() {
        let server = TestOAuthServer::start(TokenMode::Refreshes).await;
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path().join("tokens.json");
        save_bound_cache(
            cache.clone(),
            &format!("{}/mcp", server.url),
            None,
            credentials("cached-access", Some("refresh-secret"), 3600, now()),
        )
        .await;
        let token = resolve_browser_oauth(
            "calendar",
            &format!("{}/mcp", server.url),
            None,
            Some("client-id"),
            None,
            None,
            cache.to_str(),
        )
        .await
        .unwrap();
        assert_eq!(token, "cached-access");
        assert!(
            !server
                .calls
                .lock()
                .await
                .iter()
                .any(|call| call.starts_with("POST /token"))
        );
    }

    #[tokio::test]
    async fn expired_cached_token_is_refreshed() {
        let server = TestOAuthServer::start(TokenMode::Refreshes).await;
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path().join("tokens.json");
        save_bound_cache(
            cache.clone(),
            &format!("{}/mcp", server.url),
            None,
            credentials("expired-access", Some("refresh-secret"), 1, 0),
        )
        .await;
        let token = resolve_browser_oauth(
            "calendar",
            &format!("{}/mcp", server.url),
            None,
            Some("client-id"),
            None,
            None,
            cache.to_str(),
        )
        .await
        .unwrap();
        assert_eq!(token, "refreshed-access");
        assert!(
            server
                .calls
                .lock()
                .await
                .iter()
                .any(|call| call.starts_with("POST /token"))
        );
    }

    #[tokio::test]
    async fn expired_confidential_client_cache_refreshes_with_its_secret() {
        let server = TestOAuthServer::start(TokenMode::Refreshes).await;
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path().join("tokens.json");
        save_bound_cache(
            cache.clone(),
            &format!("{}/mcp", server.url),
            Some("client-secret"),
            credentials("expired-access", Some("refresh-secret"), 1, 0),
        )
        .await;

        let token = resolve_browser_oauth(
            "calendar",
            &format!("{}/mcp", server.url),
            None,
            Some("client-id"),
            Some("client-secret"),
            None,
            cache.to_str(),
        )
        .await
        .unwrap();
        assert_eq!(token, "refreshed-access");
        let requests = server.requests.lock().await;
        assert!(
            requests.iter().any(|request| request
                .to_ascii_lowercase()
                .contains("authorization: basic")),
            "confidential refresh request did not include HTTP Basic client authentication: {requests:?}"
        );
    }

    #[tokio::test]
    async fn refresh_failure_falls_back_to_browser_authorization() {
        let server = TestOAuthServer::start(TokenMode::RejectsRefresh).await;
        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path().join("tokens.json");
        save_bound_cache(
            cache.clone(),
            &format!("{}/mcp", server.url),
            None,
            credentials("expired-access", Some("refresh-secret"), 1, 0),
        )
        .await;
        let token = resolve_browser_oauth_with_opener("calendar", &format!("{}/mcp", server.url), None, Some("client-id"), None, None, cache.to_str(), |authorization_url| {
            let url = url::Url::parse(authorization_url).unwrap();
            let redirect_uri = url.query_pairs().find(|(key, _)| key == "redirect_uri").unwrap().1.into_owned();
            let state = url.query_pairs().find(|(key, _)| key == "state").unwrap().1.into_owned();
            tokio::spawn(async move {
                let redirect = url::Url::parse(&redirect_uri).unwrap();
                let mut stream = tokio::net::TcpStream::connect((redirect.host_str().unwrap(), redirect.port().unwrap())).await.unwrap();
                let request = format!("GET {}?code=browser-code&state={state} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n", redirect.path());
                let _ = stream.write_all(request.as_bytes()).await;
            });
            Ok(())
        }).await.unwrap();
        assert_eq!(token, "browser-access");
    }

    #[tokio::test]
    async fn rejects_unsafe_resource_metadata_url_before_contact() {
        let malicious = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let malicious_address = malicious.local_addr().unwrap();
        let protected = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let protected_url = format!("http://{}/mcp", protected.local_addr().unwrap());
        let advertised = format!("http://evil.test:{}/metadata", malicious_address.port());
        tokio::spawn(async move {
            let (mut stream, _) = protected.accept().await.unwrap();
            let mut buffer = [0_u8; 2048];
            let _ = stream.read(&mut buffer).await;
            let response = format!(
                "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer resource_metadata=\"{advertised}\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .resolve("evil.test", malicious_address)
            .build()
            .unwrap();

        assert!(
            discover_authorization(&protected_url, &client)
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(150), malicious.accept())
                .await
                .is_err(),
            "unsafe protected-resource metadata URL was contacted"
        );
    }

    #[tokio::test]
    async fn rejects_unsafe_discovery_redirect_before_contact() {
        let malicious = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let malicious_address = malicious.local_addr().unwrap();
        let protected = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let protected_address = protected.local_addr().unwrap();
        let protected_url = format!("http://{protected_address}/mcp");
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = protected.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buffer = [0_u8; 2048];
                    let size = stream.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..size]);
                    let (status, headers) = if request.contains("oauth-protected-resource") {
                        (
                            "302 Found",
                            format!(
                                "Location: http://evil.test:{}/metadata\r\n",
                                malicious_address.port()
                            ),
                        )
                    } else {
                        ("404 Not Found", String::new())
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\n{headers}Content-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .resolve("evil.test", malicious_address)
            .build()
            .unwrap();

        assert!(
            discover_authorization(&protected_url, &client)
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(150), malicious.accept())
                .await
                .is_err(),
            "unsafe discovery redirect was contacted"
        );
    }

    #[tokio::test]
    async fn rejects_unsafe_authorization_server_url_before_contact() {
        let malicious = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let malicious_address = malicious.local_addr().unwrap();
        let protected = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let protected_address = protected.local_addr().unwrap();
        let protected_url = format!("http://{protected_address}/mcp");
        let advertised = format!("http://evil.test:{}", malicious_address.port());
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = protected.accept().await else {
                    break;
                };
                let advertised = advertised.clone();
                tokio::spawn(async move {
                    let mut buffer = [0_u8; 2048];
                    let size = stream.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..size]);
                    let body = if request.contains("oauth-protected-resource") {
                        format!(r#"{{"authorization_servers":["{advertised}"]}}"#)
                    } else {
                        "{}".to_string()
                    };
                    let status = if request.contains("oauth-protected-resource") {
                        "200 OK"
                    } else {
                        "404 Not Found"
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .resolve("evil.test", malicious_address)
            .build()
            .unwrap();

        assert!(
            discover_authorization(&protected_url, &client)
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(150), malicious.accept())
                .await
                .is_err(),
            "unsafe authorization-server URL was contacted"
        );
    }

    #[test]
    fn rejects_plaintext_non_loopback_oauth_endpoints() {
        for endpoint in [
            "http://calendar.example.com/mcp",
            "http://auth.example.com/authorize",
        ] {
            assert!(validate_oauth_endpoint(endpoint, "test endpoint").is_err());
        }
        assert!(validate_oauth_endpoint("http://127.0.0.1:8080/mcp", "test endpoint").is_ok());
        assert!(
            validate_oauth_endpoint("https://calendar.example.com/mcp", "test endpoint").is_ok()
        );
    }

    #[tokio::test]
    async fn rejects_cache_metadata_that_does_not_match_the_connection() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tokens.json");
        let original = CacheBinding::new(
            "https://calendar.example.com/mcp",
            Some(&["calendar.read".to_string()]),
            Some("calendar-cli"),
            Some("client-secret"),
            &test_discovered("https://calendar.example.com/mcp"),
        );
        FileCredentialStore::new(path.clone(), original)
            .save(credentials(
                "access-secret",
                Some("refresh-secret"),
                3600,
                now(),
            ))
            .await
            .unwrap();

        let changed = CacheBinding::new(
            "https://calendar.example.com/mcp",
            Some(&["calendar.read".to_string()]),
            Some("calendar-cli"),
            Some("client-secret"),
            &DiscoveredAuthorization {
                authorization_server: "https://login.example.com".to_string(),
                metadata: serde_json::from_value(serde_json::json!({
                    "issuer": "https://login.example.com",
                    "authorization_endpoint": "https://login.example.com/authorize",
                    "token_endpoint": "https://login.example.com/token-v2",
                    "registration_endpoint": "https://login.example.com/register"
                }))
                .unwrap(),
            },
        );
        assert!(
            FileCredentialStore::new(path, changed)
                .load()
                .await
                .unwrap()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_insecure_or_symlinked_token_caches() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("tokens.json");
        std::fs::write(&path, "{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let binding = CacheBinding::new(
            "https://calendar.example.com/mcp",
            None,
            None,
            None,
            &test_discovered("https://calendar.example.com/mcp"),
        );
        assert!(
            FileCredentialStore::new(path.clone(), binding.clone())
                .load()
                .await
                .is_err()
        );

        let target = directory.path().join("target.json");
        std::fs::write(&target, "{}").unwrap();
        let link = directory.path().join("tokens-link.json");
        symlink(&target, &link).unwrap();
        assert!(
            FileCredentialStore::new(link, binding)
                .load()
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn ignores_invalid_error_state_then_accepts_the_valid_callback() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for request in [
                "GET /callback?error=access_denied&state=wrong HTTP/1.1\r\nHost: localhost\r\n\r\n",
                "GET /callback?code=valid-code&state=expected HTTP/1.1\r\nHost: localhost\r\n\r\n",
            ] {
                let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
                stream.write_all(request.as_bytes()).await.unwrap();
            }
        });
        assert_eq!(
            await_authorization_code(listener, "expected")
                .await
                .unwrap(),
            "valid-code"
        );
    }

    #[tokio::test]
    async fn stalled_callback_connection_does_not_block_valid_callback() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stalled = tokio::net::TcpStream::connect(address).await.unwrap();
        let valid = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(25)).await;
            let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
            stream
                .write_all(b"GET /callback?code=valid-code&state=expected HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .await
                .unwrap();
        });

        let code = tokio::time::timeout(
            Duration::from_millis(500),
            await_authorization_code(listener, "expected"),
        )
        .await
        .expect("stalled callback starved the valid callback")
        .unwrap();
        drop(stalled);
        valid.await.unwrap();
        assert_eq!(code, "valid-code");
    }

    #[test]
    fn errors_and_debug_output_redact_oauth_secrets() {
        let error = redact_oauth_error(
            "token exchange failed: client_secret=client-secret&refresh_token=refresh-secret&code=code-secret",
        );
        assert!(!error.contains("client-secret"));
        assert!(!error.contains("refresh-secret"));
        assert!(!error.contains("code-secret"));

        let credentials = credentials("access-secret", Some("refresh-secret"), 3600, now());
        let debug = format!("{credentials:?}");
        assert!(!debug.contains("access-secret"));
        assert!(!debug.contains("refresh-secret"));
    }
}
