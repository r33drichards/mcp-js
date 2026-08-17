//! WHATWG URL ops backing the `URL` global.
//!
//! The heavy lifting is done by the `url` crate (rust-url, the same
//! spec-compliant parser Deno and Servo use). Its `quirks` module
//! implements the URL-standard setter semantics (e.g. what `url.protocol =
//! ...` may change), so the JS wrapper in `web_compat/url.js` stays thin:
//! one op parses, one op re-parses after a setter, both returning the
//! spec-shaped component strings.

use deno_core::{JsRuntime, op2};
use deno_error::JsErrorBox;
use url::Url;
use url::quirks;

/// Component getters, joined with '\n' (percent-encoding guarantees no
/// component contains a newline): href, protocol, username, password,
/// host, hostname, port, pathname, search, hash, origin.
fn origin_of(url: &Url) -> String {
    // The URL spec derives a blob: URL's origin from the URL parsed out of
    // its path; rust-url returns an opaque origin instead.
    if url.scheme() == "blob" {
        if let Ok(inner) = Url::parse(url.path()) {
            // Only http/https inner URLs yield a tuple origin.
            if matches!(inner.scheme(), "http" | "https") {
                return inner.origin().ascii_serialization();
            }
        }
        return "null".to_string();
    }
    url.origin().ascii_serialization()
}

fn components(url: &Url) -> String {
    [
        quirks::href(url),
        quirks::protocol(url),
        quirks::username(url),
        quirks::password(url),
        &quirks::host(url).to_string(),
        &quirks::hostname(url).to_string(),
        quirks::port(url),
        &quirks::pathname(url).to_string(),
        quirks::search(url),
        quirks::hash(url),
        &origin_of(url),
    ]
    .join("\n")
}

#[op2]
#[string]
fn op_url_parse(
    #[string] input: String,
    #[string] base: Option<String>,
) -> Result<String, JsErrorBox> {
    let base_url = match base {
        Some(b) => Some(
            Url::parse(&b)
                .map_err(|e| JsErrorBox::type_error(format!("Invalid base URL: {}", e)))?,
        ),
        None => None,
    };
    let url = Url::options()
        .base_url(base_url.as_ref())
        .parse(&input)
        .map_err(|e| JsErrorBox::type_error(format!("Invalid URL: {}", e)))?;
    Ok(components(&url))
}

/// Setter IDs, mirrored in web_compat/url.js.
const SET_PROTOCOL: u8 = 0;
const SET_USERNAME: u8 = 1;
const SET_PASSWORD: u8 = 2;
const SET_HOST: u8 = 3;
const SET_HOSTNAME: u8 = 4;
const SET_PORT: u8 = 5;
const SET_PATHNAME: u8 = 6;
const SET_SEARCH: u8 = 7;
const SET_HASH: u8 = 8;

#[op2]
#[string]
fn op_url_reparse(
    #[string] href: String,
    setter: u8,
    #[string] value: String,
) -> Result<String, JsErrorBox> {
    let mut url = Url::parse(&href)
        .map_err(|e| JsErrorBox::type_error(format!("Invalid URL: {}", e)))?;
    // Per the URL standard, setters that cannot apply are silent no-ops.
    match setter {
        SET_PROTOCOL => { let _ = quirks::set_protocol(&mut url, &value); }
        SET_USERNAME => { let _ = quirks::set_username(&mut url, &value); }
        SET_PASSWORD => { let _ = quirks::set_password(&mut url, &value); }
        SET_HOST => { let _ = quirks::set_host(&mut url, &value); }
        SET_HOSTNAME => { let _ = quirks::set_hostname(&mut url, &value); }
        SET_PORT => { let _ = quirks::set_port(&mut url, &value); }
        SET_PATHNAME => quirks::set_pathname(&mut url, &value),
        SET_SEARCH => quirks::set_search(&mut url, &value),
        SET_HASH => quirks::set_hash(&mut url, &value),
        _ => return Err(JsErrorBox::type_error("Unknown URL setter")),
    }
    Ok(components(&url))
}

deno_core::extension!(url_ext, ops = [op_url_parse, op_url_reparse]);

pub fn create_extension() -> deno_core::Extension {
    url_ext::init()
}

pub fn inject_url(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<web-compat-url>", include_str!("web_compat/url.js").to_string())
        .map_err(|e| format!("Failed to install URL: {}", e))?;
    Ok(())
}

pub fn inject_url_snapshot(
    runtime: &mut deno_core::JsRuntimeForSnapshot,
) -> Result<(), String> {
    runtime
        .execute_script("<web-compat-url>", include_str!("web_compat/url.js").to_string())
        .map_err(|e| format!("Failed to install URL: {}", e))?;
    Ok(())
}
