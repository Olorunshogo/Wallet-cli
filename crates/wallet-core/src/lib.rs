//! Engine-agnostic core of the wallet library.
//!
//! This crate defines the public vocabulary shared by every wallet engine and
//! chain backend: domain types, the [`WalletEngine`] and chain traits, and one
//! error enum per operation. It performs no IO and does not depend on BDK, so
//! a future native engine (see `docs/v2`) can implement the same traits.

#![warn(missing_docs)]
// Clippy's uninlined_format_args lint pushes the opposite way, so allow it.
#![allow(clippy::uninlined_format_args)]
// Explicit returns are a project convention; clippy's needless_return lint
// disagrees, so allow it crate-wide.
#![allow(clippy::needless_return)]

/// Chain backend traits.
pub mod chain;
/// The [`WalletEngine`] trait.
pub mod engine;
/// One error enum per operation.
pub mod error;
/// Key material.
pub mod keys;
pub mod testkit;
/// Domain types shared by every engine.
pub mod types;

pub use bitcoin;

pub use chain::{BlockSource, Broadcaster, FeeEstimator};
pub use engine::WalletEngine;
pub use error::{
    AddressError, BoxError, BroadcastError, BuildTxError, BumpFeeError, CreateError, Error,
    FeeEstimateError, LoadError, SignError, SourceError, SyncError, TrackTxError,
};
pub use keys::{KeySource, MnemonicLength};
pub use types::{
    AddressInfo, Balance, BlockId, CoinSelection, Keychain, Recipient, SignOutcome, SyncReport,
    TxDetails, TxRequest, TxStatus, Utxo,
};
