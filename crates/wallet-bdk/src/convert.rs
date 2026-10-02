//! Translation between BDK types and wallet-core types. Keeping this in one
//! place is what stops BDK types leaking into the public API.

use bdk_wallet::chain::{ChainPosition, ConfirmationBlockTime};
use bdk_wallet::error::{BuildFeeBumpError, CreateTxError};
use bdk_wallet::signer::SignerError;
use bdk_wallet::{KeychainKind, LoadError as BdkLoadError, LoadMismatch, LoadWithPersistError};
use wallet_core::{BlockId, BuildTxError, BumpFeeError, Keychain, LoadError, SignError, TxStatus};

pub(crate) fn keychain(k: KeychainKind) -> Keychain {
    match k {
        KeychainKind::External => Keychain::External,
        KeychainKind::Internal => Keychain::Internal,
    }
}

pub(crate) fn block_id(id: bdk_wallet::chain::BlockId) -> BlockId {
    BlockId {
        height: id.height,
        hash: id.hash,
    }
}

pub(crate) fn bdk_block_id(id: BlockId) -> bdk_wallet::chain::BlockId {
    bdk_wallet::chain::BlockId {
        height: id.height,
        hash: id.hash,
    }
}

pub(crate) fn status(pos: &ChainPosition<ConfirmationBlockTime>, tip_height: u32) -> TxStatus {
    match pos {
        ChainPosition::Confirmed { anchor, .. } => {
            let height = anchor.block_id.height;
            TxStatus::Confirmed {
                height,
                confirmations: tip_height.saturating_sub(height) + 1,
            }
        }
        ChainPosition::Unconfirmed { .. } => TxStatus::Unconfirmed,
    }
}

pub(crate) fn load_error(err: LoadWithPersistError<bdk_wallet::rusqlite::Error>) -> LoadError {
    match err {
        LoadWithPersistError::Persist(e) => LoadError::Persist(Box::new(e)),
        LoadWithPersistError::InvalidChangeSet(e) => match e {
            BdkLoadError::Mismatch(LoadMismatch::Network { loaded, expected }) => {
                LoadError::NetworkMismatch {
                    expected,
                    found: loaded,
                }
            }
            BdkLoadError::Mismatch(LoadMismatch::Descriptor { .. }) => {
                LoadError::DescriptorMismatch
            }
            BdkLoadError::Descriptor(e) => LoadError::InvalidDescriptor(Box::new(e)),
            other => LoadError::Corrupt(Box::new(other)),
        },
    }
}

pub(crate) fn build_tx_error(err: CreateTxError) -> BuildTxError {
    match err {
        CreateTxError::NoRecipients => BuildTxError::NoRecipients,
        CreateTxError::CoinSelection(e) => BuildTxError::InsufficientFunds {
            needed: e.needed,
            available: e.available,
        },
        CreateTxError::OutputBelowDustLimit(index) => BuildTxError::OutputBelowDust { index },
        CreateTxError::FeeTooLow { required } => BuildTxError::FeeTooLow { required },
        CreateTxError::FeeRateTooLow { required } => BuildTxError::FeeRateTooLow { required },
        other => BuildTxError::Engine(Box::new(other)),
    }
}

pub(crate) fn sign_error(err: SignerError) -> SignError {
    match err {
        SignerError::MissingWitnessUtxo
        | SignerError::MissingNonWitnessUtxo
        | SignerError::InvalidNonWitnessUtxo
        | SignerError::MissingWitnessScript
        | SignerError::MissingHdKeypath
        | SignerError::InputIndexOutOfRange(_) => SignError::InvalidPsbt(Box::new(err)),
        other => SignError::Engine(Box::new(other)),
    }
}

pub(crate) fn fee_bump_error(err: BuildFeeBumpError) -> BumpFeeError {
    match err {
        BuildFeeBumpError::TransactionNotFound(txid) => BumpFeeError::NotFound(txid),
        BuildFeeBumpError::TransactionConfirmed(txid) => BumpFeeError::AlreadyConfirmed(txid),
        BuildFeeBumpError::IrreplaceableTransaction(txid) => BumpFeeError::NotReplaceable(txid),
        other => BumpFeeError::Engine(Box::new(other)),
    }
}

pub(crate) fn bump_finish_error(err: CreateTxError) -> BumpFeeError {
    match err {
        CreateTxError::FeeRateTooLow { required } => BumpFeeError::FeeRateTooLow { required },
        CreateTxError::FeeTooLow { required } => BumpFeeError::FeeTooLow { required },
        CreateTxError::CoinSelection(e) => BumpFeeError::InsufficientFunds {
            needed: e.needed,
            available: e.available,
        },
        other => BumpFeeError::Engine(Box::new(other)),
    }
}
