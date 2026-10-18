//! Experimental runner-only cluster member. Compiles the existing consensus module
//! directly, with no V8/Engine dependency. Use only on a trusted cluster network.
#[path = "../../server/src/cluster.rs"]
#[rustfmt::skip]
pub mod cluster;
mod remote;
mod replicated;
pub use remote::*;
pub use replicated::*;

#[cfg(test)]
mod model_replay;
