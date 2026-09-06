//! Web Crypto ops: getRandomValues / randomUUID / SubtleCrypto digest +
//! HMAC sign/verify. Randomness comes from the OS CSPRNG (rand::rngs::OsRng);
//! digests from the RustCrypto sha1/sha2 crates.

use deno_core::{JsRuntime, op2};
use deno_error::JsErrorBox;
use hmac::Mac;
use rand::RngCore;
use sha1::{Digest, Sha1};
use sha2::{Sha256, Sha384, Sha512};

#[op2(fast)]
fn op_crypto_get_random_values(#[buffer] out: &mut [u8]) -> Result<(), JsErrorBox> {
    rand::rngs::OsRng
        .try_fill_bytes(out)
        .map_err(|e| JsErrorBox::generic(format!("OS RNG failure: {}", e)))
}

#[op2]
#[string]
fn op_crypto_random_uuid() -> Result<String, JsErrorBox> {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|e| JsErrorBox::generic(format!("OS RNG failure: {}", e)))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 10
    let h = |b: &[u8]| b.iter().map(|x| format!("{:02x}", x)).collect::<String>();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        h(&bytes[0..4]),
        h(&bytes[4..6]),
        h(&bytes[6..8]),
        h(&bytes[8..10]),
        h(&bytes[10..16]),
    ))
}

fn digest_bytes(algorithm: &str, data: &[u8]) -> Result<Vec<u8>, JsErrorBox> {
    Ok(match algorithm {
        "SHA-1" => Sha1::digest(data).to_vec(),
        "SHA-256" => Sha256::digest(data).to_vec(),
        "SHA-384" => Sha384::digest(data).to_vec(),
        "SHA-512" => Sha512::digest(data).to_vec(),
        _ => {
            return Err(JsErrorBox::new(
                "DOMExceptionNotSupportedError",
                format!("Unrecognized algorithm name: {}", algorithm),
            ));
        }
    })
}

#[op2]
#[buffer]
fn op_crypto_digest(
    #[string] algorithm: String,
    #[buffer] data: &[u8],
) -> Result<Vec<u8>, JsErrorBox> {
    digest_bytes(&algorithm, data)
}

#[op2]
#[buffer]
fn op_crypto_hmac_sign(
    #[string] hash: String,
    #[buffer] key: &[u8],
    #[buffer] data: &[u8],
) -> Result<Vec<u8>, JsErrorBox> {
    macro_rules! hmac {
        ($t:ty) => {{
            let mut mac = <hmac::Hmac<$t>>::new_from_slice(key)
                .map_err(|e| JsErrorBox::generic(format!("HMAC key error: {}", e)))?;
            mac.update(data);
            Ok(mac.finalize().into_bytes().to_vec())
        }};
    }
    match hash.as_str() {
        "SHA-1" => hmac!(Sha1),
        "SHA-256" => hmac!(Sha256),
        "SHA-384" => hmac!(Sha384),
        "SHA-512" => hmac!(Sha512),
        _ => Err(JsErrorBox::new(
            "DOMExceptionNotSupportedError",
            format!("Unrecognized hash name: {}", hash),
        )),
    }
}

deno_core::extension!(
    crypto_ext,
    ops = [
        op_crypto_get_random_values,
        op_crypto_random_uuid,
        op_crypto_digest,
        op_crypto_hmac_sign,
    ],
);

pub fn create_extension() -> deno_core::Extension {
    crypto_ext::init()
}

pub fn inject_crypto(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script(
            "<web-compat-crypto>",
            include_str!("web_compat/crypto.js").to_string(),
        )
        .map_err(|e| format!("Failed to install crypto: {}", e))?;
    Ok(())
}

pub fn inject_crypto_snapshot(
    runtime: &mut deno_core::JsRuntimeForSnapshot,
) -> Result<(), String> {
    runtime
        .execute_script(
            "<web-compat-crypto>",
            include_str!("web_compat/crypto.js").to_string(),
        )
        .map_err(|e| format!("Failed to install crypto: {}", e))?;
    Ok(())
}
