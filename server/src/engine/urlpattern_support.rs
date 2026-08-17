//! URLPattern ops backed by the `urlpattern` crate (the spec
//! implementation Deno uses). Pattern compilation and input
//! canonicalization happen in Rust via the crate's browser-integration
//! `quirks` module; the JS wrapper (web_compat/urlpattern.js) executes the
//! generated ECMAScript regexes, because matching semantics are defined in
//! terms of JS RegExp.

use deno_core::op2;
use deno_error::JsErrorBox;
use urlpattern::UrlPatternOptions;
use urlpattern::quirks;
use urlpattern::RegexSyntax;

type StringOrInit = quirks::StringOrInit<'static>;

#[op2]
#[serde]
fn op_urlpattern_parse(
    #[serde] input: StringOrInit,
    #[string] base_url: Option<String>,
    ignore_case: bool,
) -> Result<quirks::UrlPattern, JsErrorBox> {
    let init = quirks::process_construct_pattern_input(input, base_url.as_deref())
        .map_err(|e| JsErrorBox::type_error(e.to_string()))?;
    let options = UrlPatternOptions {
        ignore_case,
        regex_syntax: RegexSyntax::EcmaScript,
    };
    quirks::parse_pattern::<quirks::EcmaRegexp>(init, options)
        .map_err(|e| JsErrorBox::type_error(e.to_string()))
}

#[op2]
#[serde]
#[allow(clippy::type_complexity)]
fn op_urlpattern_process_match_input(
    #[serde] input: StringOrInit,
    #[string] base_url: Option<String>,
) -> Result<Option<(quirks::MatchInput, quirks::Inputs<'static>)>, JsErrorBox> {
    let res = quirks::process_match_input(input, base_url.as_deref())
        .map_err(|e| JsErrorBox::type_error(e.to_string()))?;
    Ok(match res {
        Some((match_input, inputs)) => {
            quirks::parse_match_input(match_input).map(|i| (i, inputs))
        }
        None => None,
    })
}

deno_core::extension!(
    urlpattern_ext,
    ops = [op_urlpattern_parse, op_urlpattern_process_match_input],
);

pub fn create_extension() -> deno_core::Extension {
    urlpattern_ext::init()
}
