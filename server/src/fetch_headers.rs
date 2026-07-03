//! Fetch header injection rule configuration.
//!
//! JSON-friendly config types for `fetch()` header injection rules (static
//! headers or OAuth client-credentials bearer tokens), shared by the
//! `--fetch-header` / `--fetch-header-config` CLI paths in `main.rs` and by
//! library embedders (see [`crate::embed`]) that receive the same JSON shape
//! over their own control plane.

use std::fmt;

use anyhow::Result;
use serde::{
    Deserialize,
    de::{self, MapAccess, Visitor},
};

use crate::cli::FetchHeaderKey;
use crate::engine;

/// One fetch header injection rule, as it appears in `--fetch-header-config`
/// JSON: `{ "host": ..., "methods": [...], "headers": {...} }` for static
/// injection or `{ "host": ..., "auth": {...} }` for OAuth client-credentials.
/// Exactly one of `headers` / `auth` must be present.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FetchHeaderConfigRule {
    pub host: String,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub headers: Option<StaticHeadersConfig>,
    #[serde(default)]
    pub auth: Option<FetchHeaderAuthConfig>,
}

/// Static header map with duplicate-key detection at deserialization time.
#[derive(Debug, Clone)]
pub struct StaticHeadersConfig(pub std::collections::HashMap<String, String>);

impl<'de> Deserialize<'de> for StaticHeadersConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct StaticHeadersVisitor;

        impl<'de> Visitor<'de> for StaticHeadersVisitor {
            type Value = StaticHeadersConfig;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object mapping header names to header values")
            }

            fn visit_map<A>(self, mut map: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut headers = std::collections::HashMap::new();

                while let Some((key, value)) = map.next_entry::<String, String>()? {
                    if headers.insert(key.clone(), value).is_some() {
                        return Err(de::Error::custom(format!("duplicate field `{}`", key)));
                    }
                }

                Ok(StaticHeadersConfig(headers))
            }
        }

        deserializer.deserialize_map(StaticHeadersVisitor)
    }
}

/// OAuth client-credentials auth block of a [`FetchHeaderConfigRule`].
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FetchHeaderAuthConfig {
    #[serde(rename = "type")]
    pub auth_type: String,
    pub header: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default = "crate::engine::fetch::default_refresh_buffer_secs")]
    pub refresh_buffer_secs: u64,
}

impl FetchHeaderConfigRule {
    pub fn into_runtime_rule(self) -> Result<engine::fetch::HeaderRule> {
        let host = self.host;
        let methods = self.methods;

        match (self.headers, self.auth) {
            (Some(_), Some(_)) => anyhow::bail!(
                "Fetch header config rule for host '{}' cannot define both 'headers' and 'auth'",
                host
            ),
            (None, None) => anyhow::bail!(
                "Fetch header config rule for host '{}' must define either 'headers' or 'auth'",
                host
            ),
            (Some(headers), None) => engine::fetch::HeaderRule::new(
                host,
                methods,
                engine::fetch::HeaderInjection::Static { headers: headers.0 },
            ),
            (None, Some(auth)) => {
                if auth.auth_type != "oauth_client_credentials" {
                    anyhow::bail!(
                        "Unsupported fetch auth type '{}' for host '{}'",
                        auth.auth_type,
                        host
                    );
                }

                engine::fetch::HeaderRule::oauth_client_credentials(
                    host,
                    methods,
                    engine::fetch::OAuthClientCredentialsConfig {
                        header_name: auth.header,
                        token_url: auth.token_url,
                        client_id: auth.client_id,
                        client_secret: auth.client_secret,
                        scope: auth.scope,
                        refresh_buffer_secs: auth.refresh_buffer_secs,
                    },
                )
            }
        }
    }
}

/// Convert already-deserialized config rules into runtime `HeaderRule`s.
pub fn rules_from_config(
    rules: Vec<FetchHeaderConfigRule>,
) -> Result<Vec<engine::fetch::HeaderRule>> {
    rules.into_iter().map(|r| r.into_runtime_rule()).collect()
}

/// Load fetch header injection rules from CLI flags and/or a JSON config file.
pub fn load_fetch_header_rules(
    cli_rules: &[String],
    config_path: &Option<String>,
) -> Result<Vec<engine::fetch::HeaderRule>> {
    let mut rules = Vec::new();

    for entry in cli_rules {
        rules.push(parse_fetch_header_cli(entry)?);
    }

    if let Some(path) = config_path {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read fetch header config '{}': {}", path, e))?;
        let file_rules: Vec<FetchHeaderConfigRule> = serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Invalid JSON in fetch header config '{}': {}", path, e))?;
        rules.extend(rules_from_config(file_rules)?);
    }

    Ok(rules)
}

/// Parse a `--fetch-header` CLI string into a `HeaderRule`.
/// Format: host=<host>,header=<name>,value=<val>[,methods=GET;POST]
/// Or:     host=<host>,header=<name>,token_url=<url>,client_id=<id>,client_secret=<secret>[,scope=<scope>][,methods=GET;POST][,refresh_buffer_secs=30]
pub fn parse_fetch_header_cli(s: &str) -> Result<engine::fetch::HeaderRule> {
    let mut host = None;
    let mut methods = Vec::new();
    let mut header_name = None;
    let mut header_value = None;
    let mut token_url = None;
    let mut client_id = None;
    let mut client_secret = None;
    let mut scope = None;
    let mut refresh_buffer_secs = None;

    for part in s.split(',') {
        let (key, val) = part.split_once('=')
            .ok_or_else(|| anyhow::anyhow!(
                "Invalid --fetch-header segment '{}'. Expected key=value", part
            ))?;
        let parsed_key = FetchHeaderKey::from_key(key.trim()).ok_or_else(|| anyhow::anyhow!(
            "Unknown key '{}' in --fetch-header. Expected: {}",
            key.trim(),
            FetchHeaderKey::expected()
        ))?;
        match parsed_key {
            FetchHeaderKey::Host => host = Some(val.trim().to_string()),
            FetchHeaderKey::Methods => methods = val.split(';').map(|m| m.to_string()).collect(),
            FetchHeaderKey::Header => header_name = Some(val.trim().to_string()),
            FetchHeaderKey::Value => header_value = Some(val.to_string()),
            FetchHeaderKey::TokenUrl => token_url = Some(val.trim().to_string()),
            FetchHeaderKey::ClientId => client_id = Some(val.trim().to_string()),
            FetchHeaderKey::ClientSecret => client_secret = Some(val.to_string()),
            FetchHeaderKey::Scope => scope = Some(val.trim().to_string()),
            FetchHeaderKey::RefreshBufferSecs => {
                refresh_buffer_secs = Some(val.trim().parse::<u64>().map_err(|e| anyhow::anyhow!(
                    "Invalid 'refresh_buffer_secs' value '{}': {}",
                    val.trim(),
                    e
                ))?)
            }
        }
    }

    let host = host.ok_or_else(|| anyhow::anyhow!("--fetch-header missing 'host'"))?;
    let header_name = header_name.ok_or_else(|| anyhow::anyhow!("--fetch-header missing 'header'"))?;
    let has_dynamic_keys = token_url.is_some()
        || client_id.is_some()
        || client_secret.is_some()
        || scope.is_some()
        || refresh_buffer_secs.is_some();

    match (header_value, has_dynamic_keys) {
        (Some(_), true) => anyhow::bail!(
            "--fetch-header cannot mix static 'value' with dynamic oauth keys"
        ),
        (Some(value), false) => engine::fetch::HeaderRule::static_header(
            host,
            methods,
            header_name,
            value,
        ),
        (None, true) => engine::fetch::HeaderRule::oauth_client_credentials(
            host,
            methods,
            engine::fetch::OAuthClientCredentialsConfig {
                header_name,
                token_url: token_url.ok_or_else(|| anyhow::anyhow!(
                    "--fetch-header missing 'token_url' for dynamic oauth rule"
                ))?,
                client_id: client_id.ok_or_else(|| anyhow::anyhow!(
                    "--fetch-header missing 'client_id' for dynamic oauth rule"
                ))?,
                client_secret: client_secret.ok_or_else(|| anyhow::anyhow!(
                    "--fetch-header missing 'client_secret' for dynamic oauth rule"
                ))?,
                scope,
                refresh_buffer_secs: refresh_buffer_secs
                    .unwrap_or_else(engine::fetch::default_refresh_buffer_secs),
            },
        ),
        (None, false) => anyhow::bail!(
            "--fetch-header must provide either 'value' for a static rule or the full dynamic oauth key set: token_url, client_id, client_secret"
        ),
    }
}
