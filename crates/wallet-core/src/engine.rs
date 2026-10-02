use bitcoin::{Network, Psbt, Transaction, Txid};

use crate::chain::BlockSource;
use crate::error::{AddressError, BuildTxError, BumpFeeError, SignError, SyncError, TrackTxError};
use crate::types::{
    AddressInfo, Balance, BlockId, SignOutcome, SyncReport, TxDetails, TxRequest, TxStatus, Utxo,
};

/// The contract every wallet engine implements.
///
/// v1 ships `BdkEngine` (wraps `bdk_wallet`). v2 plans a native engine built on
/// `bitcoin` + `miniscript` (see `docs/v2`). Because callers only see this
/// trait and the types in this crate, swapping engines is not a breaking change.
///
/// Engines persist their own state: any method that mutates the wallet writes
/// the change to storage before returning.
pub trait WalletEngine {
    /// Network the wallet was created for.
    fn network(&self) -> Network;

    /// Whether the engine holds private keys. Watch-only wallets return false.
    fn can_sign(&self) -> bool;

    /// Latest block the wallet has processed.
    fn tip(&self) -> BlockId;

    /// Next unused receive address. Repeated calls return the same address
    /// until it receives funds, which avoids burning through the gap limit.
    fn new_address(&mut self) -> Result<AddressInfo, AddressError>;

    /// Reveal a fresh receive address even if earlier ones are still unused.
    fn reveal_next_address(&mut self) -> Result<AddressInfo, AddressError>;

    /// Current balance, as of the last sync.
    fn balance(&self) -> Balance;

    /// Unspent outputs on the canonical chain, including unconfirmed ones.
    fn list_utxos(&self) -> Vec<Utxo>;

    /// Wallet transactions, newest first. Unconfirmed transactions come first.
    fn transactions(&self) -> Vec<TxDetails>;

    /// Confirmation state of a wallet transaction, or `None` if unknown.
    fn tx_status(&self, txid: Txid) -> Option<TxStatus>;

    /// Build an unsigned PSBT paying `request.recipients`.
    fn build_tx(&mut self, request: &TxRequest) -> Result<Psbt, BuildTxError>;

    /// Sign and, when possible, finalize a PSBT in place.
    fn sign(&self, psbt: &mut Psbt) -> Result<SignOutcome, SignError>;

    /// Bring the wallet up to date with `source`, handling reorgs.
    fn sync(&mut self, source: &dyn BlockSource) -> Result<SyncReport, SyncError>;

    /// Public descriptors for both keychains, for creating a watch-only copy.
    /// Returns `None` for watch-only wallets that have no descriptors to share.
    fn descriptors(&self) -> Option<(String, String)> {
        None
    }

    /// Record a signed transaction that has not reached the network yet,
    /// for example one saved while offline.
    ///
    /// Balance, UTXOs and coin selection then treat it as pending, so its
    /// coins are not spent twice. A later [`sync`](Self::sync) drops it again
    /// if the node does not have it, so broadcast it before syncing.
    fn track_pending(&mut self, tx: &Transaction) -> Result<(), TrackTxError> {
        let _ = tx;
        return Err(TrackTxError::Engine(
            "this engine does not support tracking pending transactions".into(),
        ));
    }

    /// Replace an unconfirmed wallet transaction with a higher-fee version
    /// (BIP125 replace-by-fee).
    ///
    /// Returns a new unsigned PSBT spending the same inputs to the same
    /// recipients; the extra fee comes out of change. Sign and broadcast it
    /// to replace the original. Engines without RBF support return
    /// [`BumpFeeError::Engine`].
    fn bump_fee(
        &mut self,
        txid: Txid,
        new_fee_rate: bitcoin::FeeRate,
    ) -> Result<Psbt, BumpFeeError> {
        let _ = (txid, new_fee_rate);
        return Err(BumpFeeError::Engine(
            "this engine does not support fee bumping".into(),
        ));
    }
}
