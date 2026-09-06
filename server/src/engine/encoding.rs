//! TextDecoder ops backed by encoding_rs (the same encoding implementation
//! Firefox and Deno use), covering the full WHATWG label table, fatal and
//! ignoreBOM modes, and stateful streaming decodes. TextEncoder stays pure
//! JS (UTF-8 only, in web_compat/encoding.js).

use std::cell::RefCell;

use deno_core::{JsRuntime, OpState, Resource, op2};
use deno_error::JsErrorBox;
use encoding_rs::{Decoder, DecoderResult, Encoding};

fn lookup(label: &str) -> Result<&'static Encoding, JsErrorBox> {
    let enc = Encoding::for_label(label.as_bytes()).ok_or_else(|| {
        JsErrorBox::new(
            "RangeError",
            format!("The given encoding '{}' is not supported.", label.trim()),
        )
    })?;
    if enc == encoding_rs::REPLACEMENT {
        return Err(JsErrorBox::new(
            "RangeError",
            format!("The given encoding '{}' is not supported.", label.trim()),
        ));
    }
    Ok(enc)
}

/// Canonical (lowercased) encoding name for a label, or RangeError.
#[op2]
#[string]
fn op_encoding_normalize(#[string] label: String) -> Result<String, JsErrorBox> {
    Ok(lookup(&label)?.name().to_ascii_lowercase())
}

struct TextDecoderResource {
    decoder: RefCell<Decoder>,
    encoding: &'static Encoding,
    ignore_bom: bool,
}

impl Resource for TextDecoderResource {
    fn name(&self) -> std::borrow::Cow<'_, str> {
        "textDecoder".into()
    }
}

fn new_decoder(encoding: &'static Encoding, ignore_bom: bool) -> Decoder {
    if ignore_bom {
        encoding.new_decoder_without_bom_handling()
    } else {
        encoding.new_decoder_with_bom_removal()
    }
}

fn decode_chunk(
    decoder: &mut Decoder,
    data: &[u8],
    fatal: bool,
    last: bool,
) -> Result<String, JsErrorBox> {
    // decode_to_string only fills spare capacity, so loop on OutputFull —
    // a flush of pending state can need room even when `data` is empty.
    let mut out = String::with_capacity(data.len() * 3 + 16);
    let mut offset = 0usize;
    loop {
        if fatal {
            let (result, read) = decoder.decode_to_string_without_replacement(
                &data[offset..],
                &mut out,
                last,
            );
            offset += read;
            match result {
                DecoderResult::Malformed(_, _) => {
                    return Err(JsErrorBox::type_error(
                        "The encoded data was not valid.".to_string(),
                    ));
                }
                DecoderResult::InputEmpty => return Ok(out),
                DecoderResult::OutputFull => out.reserve(out.capacity() * 2 + 64),
            }
        } else {
            let (result, read, _) = decoder.decode_to_string(&data[offset..], &mut out, last);
            offset += read;
            match result {
                encoding_rs::CoderResult::InputEmpty => return Ok(out),
                encoding_rs::CoderResult::OutputFull => out.reserve(out.capacity() * 2 + 64),
            }
        }
    }
}

/// One-shot decode (the common non-streaming path; no resource needed).
#[op2]
#[string]
fn op_encoding_decode_oneshot(
    #[string] label: String,
    #[buffer] data: &[u8],
    fatal: bool,
    ignore_bom: bool,
) -> Result<String, JsErrorBox> {
    let encoding = lookup(&label)?;
    let mut decoder = new_decoder(encoding, ignore_bom);
    decode_chunk(&mut decoder, data, fatal, true)
}

#[op2(fast)]
fn op_encoding_new_decoder(
    state: &mut OpState,
    #[string] label: String,
    ignore_bom: bool,
) -> Result<u32, JsErrorBox> {
    let encoding = lookup(&label)?;
    Ok(state.resource_table.add(TextDecoderResource {
        decoder: RefCell::new(new_decoder(encoding, ignore_bom)),
        encoding,
        ignore_bom,
    }))
}

#[op2]
#[string]
fn op_encoding_decode_stream(
    state: &mut OpState,
    rid: u32,
    #[buffer] data: &[u8],
    fatal: bool,
    last: bool,
) -> Result<String, JsErrorBox> {
    let res = state
        .resource_table
        .get::<TextDecoderResource>(rid)
        .map_err(|_| JsErrorBox::type_error("Invalid decoder"))?;
    let mut decoder = res.decoder.borrow_mut();
    let out = decode_chunk(&mut decoder, data, fatal, last);
    if last || out.is_err() {
        // The spec resets the decoder after a final or errored decode.
        *decoder = new_decoder(res.encoding, res.ignore_bom);
    }
    out
}

#[op2(fast)]
fn op_encoding_close_decoder(state: &mut OpState, rid: u32) {
    let _ = state.resource_table.close(rid);
}

deno_core::extension!(
    encoding_ext,
    ops = [
        op_encoding_normalize,
        op_encoding_decode_oneshot,
        op_encoding_new_decoder,
        op_encoding_decode_stream,
        op_encoding_close_decoder,
    ],
);

pub fn create_extension() -> deno_core::Extension {
    encoding_ext::init()
}

pub fn inject_encoding(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script(
            "<web-compat-encoding>",
            include_str!("web_compat/encoding.js").to_string(),
        )
        .map_err(|e| format!("Failed to install TextEncoder/TextDecoder: {}", e))?;
    Ok(())
}

pub fn inject_encoding_snapshot(
    runtime: &mut deno_core::JsRuntimeForSnapshot,
) -> Result<(), String> {
    runtime
        .execute_script(
            "<web-compat-encoding>",
            include_str!("web_compat/encoding.js").to_string(),
        )
        .map_err(|e| format!("Failed to install TextEncoder/TextDecoder: {}", e))?;
    Ok(())
}
