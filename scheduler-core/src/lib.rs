//! Experimental, single-owner scheduling core. No MCP authority or delivery layer.
mod backend;
mod core;
mod runner;
pub use backend::PulseSource;
pub use core::*;
pub use runner::*;
pub type Result<T> = std::result::Result<T, String>;
