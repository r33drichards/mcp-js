//! CompressionStream / DecompressionStream ops backed by flate2.
//!
//! One resource per stream holds the streaming (de)compressor; the JS
//! TransformStream wrapper (web_compat/compression.js) feeds chunks
//! through op_compression_write and drains with op_compression_finish.
//! Formats per the Compression Standard: "gzip", "deflate" (zlib-wrapped),
//! and "deflate-raw".

use std::cell::RefCell;
use std::io::Write;

use deno_core::{OpState, Resource, op2};
use deno_error::JsErrorBox;
use flate2::Compression;
use flate2::write::{
    DeflateDecoder, DeflateEncoder, GzDecoder, GzEncoder, ZlibDecoder, ZlibEncoder,
};

type BrotliEnc = brotli::CompressorWriter<Vec<u8>>;
type BrotliDec = brotli::DecompressorWriter<Vec<u8>>;

enum Ctx {
    GzEnc(GzEncoder<Vec<u8>>),
    ZlibEnc(ZlibEncoder<Vec<u8>>),
    RawEnc(DeflateEncoder<Vec<u8>>),
    GzDec(GzDecoder<Vec<u8>>),
    ZlibDec(ZlibDecoder<Vec<u8>>),
    RawDec(DeflateDecoder<Vec<u8>>),
    BrEnc(Box<BrotliEnc>),
    BrDec(Box<BrotliDec>),
}

impl Ctx {
    fn write(&mut self, data: &[u8]) -> std::io::Result<bool> {
        // Writes go through a helper that tolerates zero-progress (a
        // decoder that has consumed a complete stream reports 0 for
        // trailing padding — the spec wants that input ignored, not an
        // error), and flushes afterwards: flate2's write adapters buffer
        // ~32KB internally; without the flush, small chunks never reach
        // the inner Vec and a reader on the transform's readable side
        // would wait forever.
        fn write_loop<W: Write>(w: &mut W, mut data: &[u8]) -> std::io::Result<bool> {
            while !data.is_empty() {
                let n = w.write(data)?;
                if n == 0 {
                    // Complete stream already consumed; `data` is trailing junk.
                    w.flush()?;
                    return Ok(true);
                }
                data = &data[n..];
            }
            w.flush()?;
            Ok(false)
        }
        match self {
            Ctx::GzEnc(w) => write_loop(w, data),
            Ctx::ZlibEnc(w) => write_loop(w, data),
            Ctx::RawEnc(w) => write_loop(w, data),
            Ctx::GzDec(w) => write_loop(w, data),
            Ctx::ZlibDec(w) => write_loop(w, data),
            Ctx::RawDec(w) => write_loop(w, data),
            Ctx::BrEnc(w) => write_loop(w, data),
            Ctx::BrDec(w) => write_loop(w, data),
        }
    }

    fn drain(&mut self) -> Vec<u8> {
        let out = match self {
            Ctx::GzEnc(w) => w.get_mut(),
            Ctx::ZlibEnc(w) => w.get_mut(),
            Ctx::RawEnc(w) => w.get_mut(),
            Ctx::GzDec(w) => w.get_mut(),
            Ctx::ZlibDec(w) => w.get_mut(),
            Ctx::RawDec(w) => w.get_mut(),
            Ctx::BrEnc(w) => w.get_mut(),
            Ctx::BrDec(w) => w.get_mut(),
        };
        std::mem::take(out)
    }

    fn finish(self) -> std::io::Result<Vec<u8>> {
        match self {
            Ctx::GzEnc(w) => w.finish(),
            Ctx::ZlibEnc(w) => w.finish(),
            Ctx::RawEnc(w) => w.finish(),
            Ctx::GzDec(w) => w.finish(),
            Ctx::ZlibDec(w) => w.finish(),
            Ctx::RawDec(w) => w.finish(),
            Ctx::BrEnc(w) => {
                let mut w = *w;
                w.flush()?;
                Ok(w.into_inner())
            }
            Ctx::BrDec(w) => w
                .into_inner()
                .map_err(|_| std::io::Error::other("truncated brotli stream")),
        }
    }
}

struct CompressionResource {
    ctx: RefCell<Option<Ctx>>,
    /// Trailing junk after a complete compressed stream: the spec delivers
    /// the decoded output and then errors the stream.
    trailing_junk: std::cell::Cell<bool>,
}

impl Resource for CompressionResource {
    fn name(&self) -> std::borrow::Cow<'_, str> {
        "compressionStream".into()
    }
}

#[op2(fast)]
fn op_compression_has_junk(state: &mut OpState, rid: u32) -> bool {
    state
        .resource_table
        .get::<CompressionResource>(rid)
        .map(|r| r.trailing_junk.get())
        .unwrap_or(false)
}

#[op2(fast)]
fn op_compression_new(
    state: &mut OpState,
    #[string] format: String,
    decompress: bool,
) -> Result<u32, JsErrorBox> {
    let level = Compression::default();
    let ctx = match (format.as_str(), decompress) {
        ("gzip", false) => Ctx::GzEnc(GzEncoder::new(Vec::new(), level)),
        ("deflate", false) => Ctx::ZlibEnc(ZlibEncoder::new(Vec::new(), level)),
        ("deflate-raw", false) => Ctx::RawEnc(DeflateEncoder::new(Vec::new(), level)),
        ("gzip", true) => Ctx::GzDec(GzDecoder::new(Vec::new())),
        ("deflate", true) => Ctx::ZlibDec(ZlibDecoder::new(Vec::new())),
        ("deflate-raw", true) => Ctx::RawDec(DeflateDecoder::new(Vec::new())),
        ("brotli", false) => Ctx::BrEnc(Box::new(brotli::CompressorWriter::new(
            Vec::new(),
            4096,
            5,
            22,
        ))),
        ("brotli", true) => Ctx::BrDec(Box::new(brotli::DecompressorWriter::new(
            Vec::new(),
            4096,
        ))),
        _ => {
            return Err(JsErrorBox::type_error(format!(
                "Unsupported compression format: '{}'",
                format
            )));
        }
    };
    Ok(state.resource_table.add(CompressionResource {
        ctx: RefCell::new(Some(ctx)),
        trailing_junk: std::cell::Cell::new(false),
    }))
}

#[op2]
#[buffer]
fn op_compression_write(
    state: &mut OpState,
    rid: u32,
    #[buffer] chunk: &[u8],
) -> Result<Vec<u8>, JsErrorBox> {
    let res = state
        .resource_table
        .get::<CompressionResource>(rid)
        .map_err(|_| JsErrorBox::type_error("Invalid compression stream"))?;
    if res.trailing_junk.get() {
        return Err(JsErrorBox::type_error(
            "Junk found after end of compressed data.".to_string(),
        ));
    }
    let mut guard = res.ctx.borrow_mut();
    let ctx = guard
        .as_mut()
        .ok_or_else(|| JsErrorBox::type_error("Compression stream already finished"))?;
    let junk = ctx
        .write(chunk)
        .map_err(|e| JsErrorBox::type_error(format!("Compression error: {}", e)))?;
    if junk {
        res.trailing_junk.set(true);
    }
    Ok(ctx.drain())
}

#[op2]
#[buffer]
fn op_compression_finish(state: &mut OpState, rid: u32) -> Result<Vec<u8>, JsErrorBox> {
    let res = state
        .resource_table
        .take::<CompressionResource>(rid)
        .map_err(|_| JsErrorBox::type_error("Invalid compression stream"))?;
    if res.trailing_junk.get() {
        return Err(JsErrorBox::type_error(
            "Junk found after end of compressed data.".to_string(),
        ));
    }
    let ctx = res
        .ctx
        .borrow_mut()
        .take()
        .ok_or_else(|| JsErrorBox::type_error("Compression stream already finished"))?;
    ctx.finish()
        .map_err(|e| JsErrorBox::type_error(format!("Compression error: {}", e)))
}

deno_core::extension!(
    compression_ext,
    ops = [
        op_compression_new,
        op_compression_write,
        op_compression_finish,
        op_compression_has_junk,
    ],
);

pub fn create_extension() -> deno_core::Extension {
    compression_ext::init()
}
