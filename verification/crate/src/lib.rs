//! Pure logic extracted from r33drichards/mcp-js for Rust->Lean translation
//! with Charon + Aeneas, following the Leanstral bug-finding pipeline.
//!
//! `original` mirrors the server code as closely as Aeneas' supported Rust
//! subset allows; `model` is a byte-level port with identical semantics
//! (including panics) for the parts of `str` that Aeneas cannot handle.

pub mod model;
