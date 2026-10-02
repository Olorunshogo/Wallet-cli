use bitcoin::address::NetworkUnchecked;
use bitcoin::{Address, Amount, BlockHash, FeeRate, OutPoint, ScriptBuf, Txid};

// === Keychains and addresses

/// Which derivation branch an address or output belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum Keychain {
    /// Receive addresses (`.../0/*`).
    External,
    /// Change addresses (`.../1/*`).
    Internal,
}

/// An address together with where it was derived from.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct AddressInfo {
    /// The address, already checked against the wallet's network.
    pub address: Address,
    /// Derivation index within the keychain.
    pub index: u32,
    /// Which keychain it came from.
    pub keychain: Keychain,
}

// === Chain position

/// A block identified by height and hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BlockId {
    /// Height above genesis (genesis is 0).
    pub height: u32,
    /// Block hash.
    pub hash: BlockHash,
}

/// Confirmation state of a transaction or output relative to the wallet's tip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(tag = "state", rename_all = "lowercase"))]
pub enum TxStatus {
    /// In the mempool (or orphaned by a reorg and not yet re-mined).
    Unconfirmed,
    /// Mined into the wallet's best chain.
    Confirmed {
        /// Height of the block that contains it.
        height: u32,
        /// 1 when it is in the tip block, 2 one block later, and so on.
        confirmations: u32,
    },
}

impl TxStatus {
    /// True when the transaction is in a block.
    pub fn is_confirmed(&self) -> bool {
        matches!(self, TxStatus::Confirmed { .. })
    }
}

// === Balances and outputs

/// Wallet balance split by spendability.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Balance {
    /// Confirmed and immediately spendable.
    #[cfg_attr(feature = "serde", serde(with = "bitcoin::amount::serde::as_sat"))]
    pub confirmed: Amount,
    /// In the mempool, not yet mined.
    #[cfg_attr(feature = "serde", serde(with = "bitcoin::amount::serde::as_sat"))]
    pub unconfirmed: Amount,
    /// Coinbase outputs that have not reached maturity.
    #[cfg_attr(feature = "serde", serde(with = "bitcoin::amount::serde::as_sat"))]
    pub immature: Amount,
}

impl Balance {
    /// Sum of all three buckets.
    pub fn total(&self) -> Amount {
        self.confirmed + self.unconfirmed + self.immature
    }
}

/// An unspent output owned by the wallet.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Utxo {
    /// The transaction output being referenced (`txid:vout`).
    pub outpoint: OutPoint,
    /// Value of the output.
    #[cfg_attr(feature = "serde", serde(with = "bitcoin::amount::serde::as_sat"))]
    pub value: Amount,
    /// The locking script, which pays one of the wallet's addresses.
    pub script_pubkey: ScriptBuf,
    /// Whether it was received or is change.
    pub keychain: Keychain,
    /// Derivation index of the address it pays.
    pub derivation_index: u32,
    /// Confirmation state of the transaction that created it.
    pub status: TxStatus,
}

/// A wallet-relevant transaction, summarised from the wallet's point of view.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct TxDetails {
    /// Transaction id.
    pub txid: Txid,
    /// Total value of wallet-owned inputs spent by this transaction.
    #[cfg_attr(feature = "serde", serde(with = "bitcoin::amount::serde::as_sat"))]
    pub sent: Amount,
    /// Total value of outputs paying to the wallet.
    #[cfg_attr(feature = "serde", serde(with = "bitcoin::amount::serde::as_sat"))]
    pub received: Amount,
    /// Known only when every input's previous output is known to the wallet.
    #[cfg_attr(feature = "serde", serde(with = "bitcoin::amount::serde::as_sat::opt"))]
    pub fee: Option<Amount>,
    /// Confirmation state.
    pub status: TxStatus,
}

// === Spending

/// Coin selection strategy used when building transactions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "kebab-case"))]
pub enum CoinSelection {
    /// Branch and bound, searching for a changeless solution first.
    #[default]
    BranchAndBound,
    /// Spend the biggest coins first; fewest inputs, may create change.
    LargestFirst,
    /// Spend the oldest coins first.
    OldestFirst,
}

/// A payment output. The address is validated against the wallet network when
/// the transaction is built, so callers can pass user input straight through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    /// Destination, not yet checked against a network.
    pub address: Address<NetworkUnchecked>,
    /// Amount to pay.
    pub amount: Amount,
}

impl Recipient {
    /// A payment of `amount` to `address`.
    pub fn new(address: Address<NetworkUnchecked>, amount: Amount) -> Self {
        Self { address, amount }
    }
}

/// Everything needed to build an unsigned transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxRequest {
    /// Outputs to pay. Change is added automatically.
    pub recipients: Vec<Recipient>,
    /// Target fee rate for the whole transaction.
    pub fee_rate: FeeRate,
    /// Overrides the wallet's default strategy for this transaction only.
    pub coin_selection: Option<CoinSelection>,
    /// Signal BIP125 replace-by-fee. BDK enables it by default.
    pub enable_rbf: bool,
}

impl TxRequest {
    /// A request with the wallet's default coin selection and RBF enabled.
    pub fn new(recipients: Vec<Recipient>, fee_rate: FeeRate) -> Self {
        Self {
            recipients,
            fee_rate,
            coin_selection: None,
            enable_rbf: true,
        }
    }
}

/// Result of a signing attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignOutcome {
    /// All inputs are signed and finalized; the transaction can be extracted.
    Finalized,
    /// Some signatures were added but more are needed (e.g. multisig).
    Partial,
}

// === Sync

/// Summary of a completed sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct SyncReport {
    /// Wallet tip before the sync.
    pub from: BlockId,
    /// Wallet tip after the sync.
    pub to: BlockId,
    /// Blocks downloaded and applied in this sync.
    pub blocks_applied: u32,
    /// Number of local blocks that were replaced because of a reorg.
    pub reorg_depth: u32,
    /// Mempool transactions the backend returned (relevant or not).
    pub mempool_txs: usize,
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn balance_total_is_sum(
            confirmed in any::<u64>(),
            unconfirmed in any::<u64>(),
            immature in any::<u64>(),
        ) {
            let b = Balance {
                confirmed: Amount::from_sat(confirmed),
                unconfirmed: Amount::from_sat(unconfirmed),
                immature: Amount::from_sat(immature),
            };
            let expected = confirmed
                .checked_add(unconfirmed)
                .and_then(|sum| sum.checked_add(immature));
            if let Some(expected) = expected {
                prop_assert_eq!(b.total(), Amount::from_sat(expected));
            }
        }

        #[test]
        fn tx_status_is_confirmed_iff_has_height(
            height in any::<u32>(),
            confirmations in any::<u32>(),
        ) {
            let status = TxStatus::Confirmed {
                height,
                confirmations,
            };
            prop_assert!(status.is_confirmed());
            prop_assert!(!TxStatus::Unconfirmed.is_confirmed());
        }
    }
}
