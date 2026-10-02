//! v1 wallet engine backed by [`bdk_wallet`].
//!
//! BDK is an implementation detail: nothing in this crate's public API
//! exposes a `bdk_*` type. Everything goes through `wallet-core` types, so the
//! engine can be replaced (see `docs/v2`) without touching callers.

#![warn(missing_docs)]
// Explicit returns are a project convention; clippy's needless_return lint
// disagrees, so allow it crate-wide.
#![allow(clippy::needless_return)]

mod builder;
mod convert;
mod engine;
mod keys;
mod sync;

pub use builder::BdkEngineBuilder;
pub use engine::BdkEngine;
pub use keys::{generate_mnemonic, validate_mnemonic};
