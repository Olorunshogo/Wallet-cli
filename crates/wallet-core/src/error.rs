//! One error enum per operation.
//!
//! Each public method returns the narrowest enum describing what can actually
//! go wrong in that call, so callers can match exhaustively on the cases they
//! care about. Variants carry structured data (amounts, heights, networks)
//! rather than strings. Failures from the underlying engine or backend that
//! have no stable meaning for callers are boxed into an `Engine`/`Connection`
//! variant with the original error kept as `source()`.
//!
//! [`Error`] unifies all of them for callers who just want `?`.

use bitcoin::{Amount, FeeRate, Network, Txid};

/// A type-erased error kept as the `source()` of a wrapping variant.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

// === Wallet lifecycle

/// Errors from creating a new wallet.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CreateError {
    /// The mnemonic is not a valid English BIP39 phrase.
    #[error("invalid mnemonic")]
    InvalidMnemonic(#[source] BoxError),
    /// A descriptor could not be parsed, or is for the wrong network.
    #[error("invalid descriptor")]
    InvalidDescriptor(#[source] BoxError),
    /// The database already holds a wallet. Load it instead.
    #[error("a wallet already exists in this database")]
    AlreadyExists,
    /// The database could not be opened or written.
    #[error("failed to access wallet storage")]
    Persist(#[source] BoxError),
}

/// Errors from loading an existing wallet.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LoadError {
    /// The database holds no wallet. Create one instead.
    #[error("no wallet found in this database")]
    NotFound,
    /// The stored wallet belongs to another network.
    #[error("wallet was created for {found}, but {expected} was requested")]
    NetworkMismatch {
        /// Network the caller asked for.
        expected: Network,
        /// Network the wallet was created on.
        found: Network,
    },
    /// The keys supplied do not belong to the stored wallet.
    #[error("provided keys do not match the stored wallet descriptors")]
    DescriptorMismatch,
    /// The mnemonic is not a valid English BIP39 phrase.
    #[error("invalid mnemonic")]
    InvalidMnemonic(#[source] BoxError),
    /// A descriptor could not be parsed, or is for the wrong network.
    #[error("invalid descriptor")]
    InvalidDescriptor(#[source] BoxError),
    /// The database is readable but its wallet data is incomplete.
    #[error("stored wallet data is incomplete or corrupt")]
    Corrupt(#[source] BoxError),
    /// The database could not be opened or read.
    #[error("failed to access wallet storage")]
    Persist(#[source] BoxError),
}

// === Addresses

/// Errors from revealing an address.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AddressError {
    /// The new derivation index could not be saved, so the address was not
    /// handed out (it could otherwise be reused after a restart).
    #[error("failed to persist address index")]
    Persist(#[source] BoxError),
}

// === Chain access

/// Errors from a [`BlockSource`](crate::BlockSource).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SourceError {
    /// The backend's best chain has no block at this height.
    #[error("block at height {0} not found")]
    HeightNotFound(u32),
    /// The backend does not know this block.
    #[error("block {0} not found")]
    BlockNotFound(bitcoin::BlockHash),
    /// The backend could not be reached, or timed out.
    #[error("chain backend unreachable")]
    Connection(#[source] BoxError),
    /// The backend answered with something that could not be understood.
    #[error("chain backend returned an invalid response")]
    InvalidResponse(#[source] BoxError),
}

/// Errors from syncing with a [`BlockSource`](crate::BlockSource).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SyncError {
    /// The backend failed while serving blocks or the mempool.
    #[error(transparent)]
    Source(#[from] SourceError),
    /// The backend serves a different network than the wallet's.
    #[error("backend is on a different chain: genesis block does not match the wallet")]
    WrongChain,
    /// A block could not be connected to the wallet's view of the chain.
    #[error("failed to apply block at height {height}")]
    ApplyBlock {
        /// Height of the block that failed.
        height: u32,
        /// Why it failed.
        #[source]
        source: BoxError,
    },
    /// The synced state could not be saved.
    #[error("failed to persist sync result")]
    Persist(#[source] BoxError),
}

/// Errors from a [`Broadcaster`](crate::Broadcaster).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BroadcastError {
    /// The node refused the transaction (double spend, fee too low, invalid).
    #[error("transaction rejected by the node: {reason}")]
    Rejected {
        /// The node's explanation, e.g. `insufficient fee`.
        reason: String,
    },
    /// The backend could not be reached, or timed out.
    #[error("chain backend unreachable")]
    Connection(#[source] BoxError),
}

/// Errors from a [`FeeEstimator`](crate::FeeEstimator).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FeeEstimateError {
    /// The backend has no data yet, common on fresh regtest and signet nodes.
    /// Callers usually fall back to a fixed rate.
    #[error("no fee estimate available for a {target_blocks}-block target")]
    Unavailable {
        /// The confirmation target that was asked for.
        target_blocks: u16,
    },
    /// The backend could not be reached, or timed out.
    #[error("chain backend unreachable")]
    Connection(#[source] BoxError),
}

// === Spending

/// Errors from building a transaction.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BuildTxError {
    /// The request had no outputs.
    #[error("transaction has no recipients")]
    NoRecipients,
    /// A recipient address belongs to another network.
    #[error("address {address} is not valid for {expected}")]
    WrongNetwork {
        /// The offending address as given.
        address: String,
        /// The wallet's network.
        expected: Network,
    },
    /// The wallet cannot cover the outputs plus fee.
    #[error("insufficient funds: need {needed}, available {available}")]
    InsufficientFunds {
        /// Outputs plus fee.
        needed: Amount,
        /// What the wallet can spend.
        available: Amount,
    },
    /// An output is too small to be relayed by nodes.
    #[error("output #{index} is below the dust limit")]
    OutputBelowDust {
        /// Position of the output in the request.
        index: usize,
    },
    /// The absolute fee would be below the relay minimum.
    #[error("absolute fee is below the required minimum of {required}")]
    FeeTooLow {
        /// Minimum fee.
        required: Amount,
    },
    /// The fee rate would be below the relay minimum.
    #[error("fee rate is below the required minimum of {required}")]
    FeeRateTooLow {
        /// Minimum fee rate.
        required: FeeRate,
    },
    /// The change address index could not be saved.
    #[error("failed to persist change address")]
    Persist(#[source] BoxError),
    /// Any other engine failure; the cause is kept as `source()`.
    #[error("wallet engine failed to build the transaction")]
    Engine(#[source] BoxError),
}

/// Errors from [`track_pending`](crate::WalletEngine::track_pending).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TrackTxError {
    /// The transaction neither spends from nor pays to this wallet.
    #[error("transaction does not involve this wallet")]
    NotRelevant,
    /// The wallet state could not be saved.
    #[error("failed to persist the pending transaction")]
    Persist(#[source] BoxError),
    /// Any other engine failure; the cause is kept as `source()`.
    #[error("wallet engine failed to track the transaction")]
    Engine(#[source] BoxError),
}

/// Errors from replacing an unconfirmed transaction with a higher-fee version
/// (BIP125 replace-by-fee).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BumpFeeError {
    /// The wallet does not know this transaction. Sync first.
    #[error("transaction {0} is not in this wallet")]
    NotFound(Txid),
    /// Mined transactions cannot be replaced.
    #[error("transaction {0} is already confirmed and cannot be replaced")]
    AlreadyConfirmed(Txid),
    /// The original transaction opted out of replace-by-fee.
    #[error("transaction {0} does not signal replace-by-fee")]
    NotReplaceable(Txid),
    /// The new rate must be higher than the original transaction's rate.
    #[error("new fee rate must be at least {required}")]
    FeeRateTooLow {
        /// Minimum rate for the replacement.
        required: FeeRate,
    },
    /// The replacement must pay a higher absolute fee than the original.
    #[error("absolute fee must be at least {required}")]
    FeeTooLow {
        /// Minimum fee for the replacement.
        required: Amount,
    },
    /// The change output cannot cover the higher fee and no other coins can.
    #[error("insufficient funds to pay the higher fee: need {needed}, available {available}")]
    InsufficientFunds {
        /// Outputs plus the new fee.
        needed: Amount,
        /// What the wallet can spend.
        available: Amount,
    },
    /// The change address index could not be saved.
    #[error("failed to persist wallet changes")]
    Persist(#[source] BoxError),
    /// Any other engine failure; the cause is kept as `source()`.
    #[error("wallet engine failed to bump the fee")]
    Engine(#[source] BoxError),
}

/// Errors from signing a PSBT.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SignError {
    /// The wallet was created from public descriptors only.
    #[error("wallet is watch-only and cannot sign")]
    WatchOnly,
    /// Keys are present but none of them matched any input.
    #[error("no inputs in the PSBT belong to this wallet")]
    NothingToSign,
    /// The PSBT lacks data needed to sign, such as the spent outputs.
    #[error("PSBT is missing data required for signing")]
    InvalidPsbt(#[source] BoxError),
    /// Any other engine failure; the cause is kept as `source()`.
    #[error("wallet engine failed to sign")]
    Engine(#[source] BoxError),
}

// === Umbrella

/// Any error the library can return. Use this when you do not need to match
/// on a specific operation's failure modes.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// See [`CreateError`].
    #[error(transparent)]
    Create(#[from] CreateError),
    /// See [`LoadError`].
    #[error(transparent)]
    Load(#[from] LoadError),
    /// See [`AddressError`].
    #[error(transparent)]
    Address(#[from] AddressError),
    /// See [`SourceError`].
    #[error(transparent)]
    Source(#[from] SourceError),
    /// See [`SyncError`].
    #[error(transparent)]
    Sync(#[from] SyncError),
    /// See [`BroadcastError`].
    #[error(transparent)]
    Broadcast(#[from] BroadcastError),
    /// See [`FeeEstimateError`].
    #[error(transparent)]
    FeeEstimate(#[from] FeeEstimateError),
    /// See [`BuildTxError`].
    #[error(transparent)]
    BuildTx(#[from] BuildTxError),
    /// See [`BumpFeeError`].
    #[error(transparent)]
    BumpFee(#[from] BumpFeeError),
    /// See [`TrackTxError`].
    #[error(transparent)]
    TrackTx(#[from] TrackTxError),
    /// See [`SignError`].
    #[error(transparent)]
    Sign(#[from] SignError),
    /// Signing succeeded but more signatures are required before broadcast.
    #[error("transaction is not fully signed")]
    NotFinalized,
    /// The signed PSBT could not be turned into a transaction, for example
    /// because its fee rate is absurdly high.
    #[error("failed to extract the signed transaction")]
    Extract(#[source] BoxError),
}
