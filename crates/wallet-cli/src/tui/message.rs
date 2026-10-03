//! Messages between the UI thread and the wallet worker.
//!
//! The UI never touches the wallet directly: it sends a [`Command`] and later
//! receives a [`WorkerEvent`]. That keeps the interface responsive while a
//! sync or broadcast is in flight, and makes both sides testable alone.

use std::fmt;
use std::path::PathBuf;

use wallet::bitcoin::{Amount, FeeRate, Network, Txid};
use wallet::{
    AddressInfo, Balance, BlockId, CoinSelection, KeySource, Recipient, SyncReport, TxDetails, Utxo,
};
use zeroize::Zeroizing;

use super::format::ErrorView;
use crate::session::NewWallet;

/// How to open the wallet once the chain is ready.
pub enum OpenRequest {
    /// Open the wallet in the data directory; `None` keys means watch-only.
    Existing {
        /// Unlocked keys, if any.
        keys: Option<KeySource>,
    },
    /// Decrypt the stored keys with a password, then open.
    Unlock {
        /// The password the user typed.
        password: Zeroizing<String>,
    },
    /// Create a new wallet in the data directory.
    Create(NewWallet),
}

impl fmt::Debug for OpenRequest {
    // Never print key material.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenRequest::Existing { keys } => f
                .debug_struct("Existing")
                .field("has_keys", &keys.is_some())
                .finish(),
            OpenRequest::Unlock { .. } => f.write_str("Unlock(..)"),
            OpenRequest::Create(_) => f.write_str("Create(..)"),
        }
    }
}

/// What the UI asks the worker to do.
#[derive(Debug)]
pub enum Command {
    /// Open or create the wallet.
    Open(OpenRequest),
    /// Sync with the chain. `manual` controls how loudly the result is shown.
    Sync {
        /// True when the user asked; false for background refreshes.
        manual: bool,
    },
    /// Reveal a fresh receive address.
    RevealAddress,
    /// Ask the node for a fee rate.
    EstimateFee {
        /// Confirmation target in blocks.
        target: u16,
    },
    /// Build and sign a payment, then report its details without sending.
    PreviewSend {
        /// Who to pay.
        recipient: Recipient,
        /// Fee rate to pay.
        fee_rate: FeeRate,
        /// Coin selection strategy; `None` uses the wallet default.
        selection: Option<CoinSelection>,
    },
    /// Broadcast the previewed payment, or save it to the outbox when offline.
    ConfirmSend,
    /// Discard the previewed payment.
    CancelSend,
    /// Replace an unconfirmed transaction with a higher-fee version.
    BumpFee {
        /// Transaction to replace.
        txid: Txid,
        /// New fee rate.
        fee_rate: FeeRate,
    },
    /// Try to reach the node again now.
    Reconnect,
    /// Polar is down: switch to the local node and its wallets.
    UseLocalNode,
    /// List the named wallets.
    ListWallets,
    /// Close this wallet and open (or start creating) the one called `name`.
    SwitchWallet {
        /// Wallet name.
        name: String,
    },
    /// The receive address of another named wallet, to pay it: the first
    /// wallet after `after` by name (wrapping), so repeating cycles through
    /// them.
    PayeeAddress {
        /// The wallet picked last time, if any.
        after: Option<String>,
    },
    /// Broadcast every payment saved in the outbox while offline.
    BroadcastSaved,
    /// Managed node only: mine blocks to a throwaway address (confirms transactions).
    Mine {
        /// How many blocks.
        blocks: u32,
    },
    /// Managed node only: mine 101 blocks to this wallet so it has spendable coins.
    Faucet,
    /// Stop the worker.
    Shutdown,
}

/// Which operation a failure belongs to, so the UI can react in context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Op {
    Wallets,
    Payee,
    Open,
    Unlock,
    Outbox,
    Sync,
    Address,
    FeeEstimate,
    Preview,
    Send,
    Bump,
    Mine,
}

/// Everything the screens show, captured after each change.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// Wallet network.
    pub network: Network,
    /// False for watch-only wallets.
    pub can_sign: bool,
    /// Last synced block.
    pub tip: BlockId,
    /// Current balance.
    pub balance: Balance,
    /// Unspent outputs.
    pub utxos: Vec<Utxo>,
    /// History, unconfirmed first.
    pub txs: Vec<TxDetails>,
    /// Current receive address.
    pub receive: AddressInfo,
}

/// A signed payment awaiting confirmation.
#[derive(Debug, Clone, PartialEq)]
pub struct SendPreview {
    /// Destination as typed.
    pub address: String,
    /// Amount paid to the destination.
    pub amount: Amount,
    /// Total fee.
    pub fee: Amount,
    /// Fee rate actually paid.
    pub fee_rate: FeeRate,
    /// Size in virtual bytes.
    pub vsize: u64,
    /// Number of coins spent.
    pub inputs: usize,
    /// Change returned to the wallet, if any.
    pub change: Option<Amount>,
}

/// What switching to a wallet led to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchKind {
    /// It is being opened; `Opened` follows.
    Opening,
    /// Its recovery words are encrypted; ask for the password.
    Locked,
    /// No wallet by that name yet; run setup to create it.
    New,
}

/// Whether the node can be reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    /// Reachable.
    Online {
        /// Human description of the backend.
        label: String,
        /// Whether `Mine`/`Faucet` work (any regtest node).
        can_mine: bool,
    },
    /// Not reachable; the wallet still works with its stored data.
    Offline {
        /// Why, in a few words.
        reason: String,
        /// False when no node is configured (`--node none`).
        retrying: bool,
    },
}

/// What the worker reports back.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkerEvent {
    /// The node became reachable or unreachable.
    Connection(Connection),
    /// The wallet is open.
    Opened(Snapshot),
    /// The named wallets.
    Wallets {
        /// Every wallet, sorted by name.
        list: Vec<crate::wallets::Entry>,
        /// The one open now.
        current: Option<String>,
    },
    /// Another wallet was selected.
    Switched {
        /// Its name.
        name: String,
        /// What happens next.
        kind: SwitchKind,
    },
    /// A sync began.
    SyncStarted {
        /// Echoes the command.
        manual: bool,
    },
    /// Blocks applied so far out of the expected total.
    SyncProgress {
        /// Blocks fetched.
        done: u32,
        /// Blocks expected.
        total: u32,
    },
    /// A sync finished.
    Synced {
        /// What changed.
        report: SyncReport,
        /// New state.
        snapshot: Snapshot,
        /// Echoes the command.
        manual: bool,
    },
    /// A fresh receive address.
    Address(AddressInfo),
    /// Another wallet's receive address, to pay it.
    PayeeAddress {
        /// The wallet's name.
        name: String,
        /// Its next unused receive address.
        address: String,
    },
    /// A fee rate suggestion.
    FeeEstimate {
        /// The rate.
        rate: FeeRate,
        /// True when the node had no data and the configured fallback is used.
        fallback: bool,
    },
    /// A payment is signed and ready to confirm.
    Preview(SendPreview),
    /// A payment was broadcast.
    Sent {
        /// Its txid.
        txid: Txid,
        /// Fee paid.
        fee: Amount,
        /// New state.
        snapshot: Snapshot,
    },
    /// Offline: a signed payment was saved to the outbox instead of sent.
    Saved {
        /// Its txid.
        txid: Txid,
        /// The file holding the raw transaction.
        path: PathBuf,
        /// New state: the payment counts as pending.
        snapshot: Snapshot,
    },
    /// How many saved payments are waiting in the outbox.
    Outbox {
        /// Number of files.
        count: usize,
    },
    /// Saved payments were broadcast.
    SavedBroadcast {
        /// Sent successfully.
        sent: Vec<Txid>,
        /// Refused by the node (e.g. coins already spent), with the reason.
        rejected: Vec<(String, String)>,
        /// New state.
        snapshot: Snapshot,
    },
    /// A transaction was replaced.
    Bumped {
        /// The replaced transaction.
        original: Txid,
        /// Its replacement.
        replacement: Txid,
        /// New state.
        snapshot: Snapshot,
    },
    /// Demo blocks were mined.
    Mined {
        /// How many.
        blocks: u32,
    },
    /// An operation failed.
    Failed {
        /// Which one.
        op: Op,
        /// What to tell the user.
        error: ErrorView,
    },
}
