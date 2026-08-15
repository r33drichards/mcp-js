//! Secure browser OAuth runtime for downstream HTTP MCP servers.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use rmcp::transport::auth::{
    AuthError, AuthorizationManager, CredentialStore, OAuthClientConfig, StoredCredentials,
};

const OAUTH_BROWSER_TIMEOUT: Duration = Duration::from_secs(300);

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
    let cache_path = token_cache
        .map(PathBuf::from)
        .unwrap_or_else(|| default_token_cache_path(server_name));
    let scopes = scope.unwrap_or_default();
    let scope_refs: Vec<&str> = scopes.iter().map(String::as_str).collect();
    let binding = CacheBinding::new(server_url, scope, client_id, client_secret);

    let mut manager = AuthorizationManager::new(server_url)
        .await
        .map_err(|_| oauth_failure(server_name, "manager initialization"))?;
    manager.set_credential_store(FileCredentialStore::new(cache_path, binding));

    let metadata = manager
        .discover_metadata()
        .await
        .map_err(|_| oauth_failure(server_name, "metadata discovery"))?;
    validate_authorization_metadata(&metadata)?;
    manager.set_metadata(metadata);

    if manager.initialize_from_store().await.unwrap_or(false) {
        if let Some(client_id) = client_id {
            configure_static_client(
                &mut manager,
                client_id,
                client_secret,
                "http://localhost",
                scopes,
            )
            .map_err(|_| oauth_failure(server_name, "cached client configuration"))?;
        }
        match manager.get_access_token().await {
            Ok(token) => return Ok(token),
            Err(AuthError::AuthorizationRequired) => {
                tracing::info!(server = %server_name, "Cached OAuth credentials require browser authorization");
            }
            Err(_) => return Err(oauth_failure(server_name, "cached token retrieval")),
        }
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
            manager
                .register_client("mcp-js", &redirect_uri, &scope_refs)
                .await
                .map_err(|_| oauth_failure(server_name, "dynamic client registration"))?;
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
    ) -> Self {
        let mut scopes = requested_scopes.unwrap_or_default().to_vec();
        scopes.sort();
        scopes.dedup();
        let payload = serde_json::json!({
            "version": 1,
            "protected_resource": protected_resource,
            "requested_scopes": scopes,
            "client_id": client_id,
            "client_secret": client_secret,
        });
        let mut hasher = Sha256::new();
        hasher.update(serde_json::to_vec(&payload).expect("cache binding is serializable"));
        Self {
            version: 1,
            fingerprint: format!("{:x}", hasher.finalize()),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct DiskTokenCache {
    binding: CacheBinding,
    credentials: StoredCredentials,
}

struct FileCredentialStore {
    path: PathBuf,
    binding: CacheBinding,
}

impl FileCredentialStore {
    fn new(path: PathBuf, binding: CacheBinding) -> Self {
        Self { path, binding }
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
                Ok((cache.binding == self.binding).then_some(cache.credentials))
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

fn validate_authorization_metadata(
    metadata: &rmcp::transport::auth::AuthorizationMetadata,
) -> Result<(), String> {
    validate_oauth_endpoint(&metadata.authorization_endpoint, "authorization endpoint")?;
    validate_oauth_endpoint(&metadata.token_endpoint, "token endpoint")?;
    if let Some(registration_endpoint) = &metadata.registration_endpoint {
        validate_oauth_endpoint(registration_endpoint, "registration endpoint")?;
    }
    Ok(())
}

fn validate_cache_file(path: &Path) -> Result<(), AuthError> {
    use std::io::ErrorKind;

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
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
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(AuthError::InternalError(
                "token cache permissions are insecure".to_string(),
            ));
        }
    }
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

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
    use tokio::io::AsyncReadExt;

    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|_| "OAuth callback listener failed".to_string())?;
        let mut buffer = [0_u8; 8192];
        let size = stream
            .read(&mut buffer)
            .await
            .map_err(|_| "OAuth callback could not be read".to_string())?;
        let request = String::from_utf8_lossy(&buffer[..size]);
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("");
        let callback = parse_callback_target(target);

        if callback.state.as_deref() != Some(expected_state) {
            let _ =
                write_callback_response(&mut stream, "Waiting", "Waiting for authorization.").await;
            continue;
        }
        if callback.error.is_some() {
            let _ = write_callback_response(
                &mut stream,
                "Authorization failed",
                "The authorization server rejected the request.",
            )
            .await;
            return Err("OAuth authorization server returned an error".to_string());
        }
        let Some(code) = callback.code else {
            let _ =
                write_callback_response(&mut stream, "Waiting", "Waiting for authorization.").await;
            continue;
        };

        let _ = write_callback_response(
            &mut stream,
            "Authorized",
            "Authorization complete. You can close this tab.",
        )
        .await;
        return Ok(code);
    }
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

    async fn save_cache(path: PathBuf, credentials: StoredCredentials) {
        FileCredentialStore::new(
            path,
            CacheBinding::new("https://calendar.example.com/mcp", None, None, None),
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
            CacheBinding::new(server_url, None, Some("client-id"), client_secret),
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
            Some(&["calendar.write".to_string()]),
            Some("calendar-cli"),
            Some("client-secret"),
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
        let binding = CacheBinding::new("https://calendar.example.com/mcp", None, None, None);
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
