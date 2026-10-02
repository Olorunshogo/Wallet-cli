//! The background thread that owns the wallet.
//!
//! It receives [`Command`]s, calls the `wallet` library's public API, and
//! reports [`WorkerEvent`]s. [`Worker::handle`] is synchronous so tests can
//! drive it step by step; [`WorkerHandle`] runs it on a thread for the UI.
//!
//! The wallet works without a node. When the node cannot be reached the
//! worker reports [`Connection::Offline`], keeps serving stored data, saves
//! signed payments to an outbox, and reconnects when asked.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use wallet::bitcoin::consensus::encode::{deserialize_hex, serialize_hex};
use wallet::bitcoin::{Address, Amount, FeeRate, Network, Psbt, Transaction, Txid};
use wallet::{
    BroadcastError, CoinSelection, Error, FeeEstimateError, KeySource, MnemonicLength, Recipient,
    SignOutcome, TxRequest, Wallet, generate_mnemonic,
};

use super::chain::{Chain, ChainFactory, Progress};
use super::format::{self, ErrorView};
use super::message::{
    Command, Connection, Op, OpenRequest, SendPreview, Snapshot, SwitchKind, WorkerEvent,
};
use crate::config::DataDir;
use crate::session;
use crate::wallets::Wallets;

/// Settings the worker needs, taken from the TUI config.
#[derive(Debug, Clone, Copy)]
pub struct WorkerConfig {
    /// Used when the node has no fee estimate, or is offline.
    pub fallback_fee_rate: FeeRate,
    /// How often to report sync progress.
    pub progress_every: Duration,
}

/// Everything needed to start a worker.
pub struct WorkerSpec {
    /// Wallet network.
    pub network: Network,
    /// Where the wallet lives.
    pub dir: DataDir,
    /// Name of the wallet in `dir`, for named wallets.
    pub name: Option<String>,
    /// All named wallets, for switching; `None` disables switching.
    pub wallets: Option<Wallets>,
    /// Worker settings.
    pub config: WorkerConfig,
    /// Connects to the chain; `None` means work offline (`--node none`).
    pub connect: Option<ChainFactory>,
}

/// The wallet side of the TUI.
pub struct Worker {
    chain: Option<Chain>,
    connect: Option<ChainFactory>,
    online: bool,
    dir: DataDir,
    network: Network,
    config: WorkerConfig,
    events: Sender<WorkerEvent>,
    wallet: Option<Wallet>,
    /// Signed payment waiting for the user to confirm.
    pending: Option<Psbt>,
    /// Managed node only: where "mine a block" sends rewards, so they do not
    /// land in the user's wallet.
    burn_address: Option<Address>,
    /// Named wallets, for switching.
    wallets: Option<Wallets>,
    /// Name of the open wallet.
    name: Option<String>,
}

impl Worker {
    /// A worker with an optional ready chain and an optional way to
    /// (re)connect. Call [`Worker::reconnect`] to report the first state.
    pub fn new(
        chain: Option<Chain>,
        connect: Option<ChainFactory>,
        dir: DataDir,
        network: Network,
        config: WorkerConfig,
        events: Sender<WorkerEvent>,
    ) -> Self {
        Self {
            chain,
            connect,
            online: false,
            dir,
            network,
            config,
            events,
            wallet: None,
            pending: None,
            burn_address: None,
            wallets: None,
            name: None,
        }
    }

    /// Enable switching between named wallets.
    pub fn with_wallets(mut self, wallets: Option<Wallets>, name: Option<String>) -> Self {
        self.wallets = wallets;
        self.name = name;
        self
    }

    fn emit(&self, event: WorkerEvent) {
        // The UI hung up; nothing left to do with the event.
        let _ = self.events.send(event);
    }

    fn fail(&self, op: Op, error: ErrorView) {
        self.emit(WorkerEvent::Failed { op, error });
    }

    /// Report a failure; an unreachable node also flips us offline.
    fn fail_with(&mut self, op: Op, err: impl Into<Error>) {
        let err = err.into();
        if format::is_connection(&err) {
            self.set_offline(format::chain(&err));
        }
        self.fail(op, format::describe(&err));
    }

    fn set_offline(&mut self, reason: String) {
        let changed = self.online;
        self.online = false;
        if changed {
            self.emit(WorkerEvent::Connection(Connection::Offline {
                reason,
                retrying: self.can_retry(),
            }));
        }
    }

    fn can_retry(&self) -> bool {
        self.chain.is_some() || self.connect.is_some()
    }

    /// Try to reach the node and report the result.
    pub fn reconnect(&mut self) {
        if self.chain.is_none()
            && let Some(connect) = self.connect.as_mut()
        {
            match connect() {
                Ok(chain) => self.chain = Some(chain),
                Err(e) => return self.report_offline(format::chain(e.as_ref())),
            }
        }
        let Some(chain) = &self.chain else {
            return self.report_offline("no node configured (--node none)".into());
        };
        match chain.backend.tip() {
            Ok(_) => {
                self.online = true;
                self.emit(WorkerEvent::Connection(Connection::Online {
                    label: chain.label.clone(),
                    can_mine: chain.miner.is_some(),
                }));
                self.flush_outbox();
            }
            Err(e) => self.report_offline(format::chain(&e)),
        }
    }

    fn report_offline(&mut self, reason: String) {
        self.online = false;
        self.emit(WorkerEvent::Connection(Connection::Offline {
            reason,
            retrying: self.can_retry(),
        }));
    }

    /// The chain, only while online.
    fn live(&self) -> Option<&Chain> {
        self.chain.as_ref().filter(|_| self.online)
    }

    /// Handle one command. Returns false when the worker should stop.
    pub fn handle(&mut self, command: Command) -> bool {
        match command {
            Command::Shutdown => return false,
            Command::Reconnect => self.reconnect(),
            Command::Open(request) => self.open(request),
            Command::ListWallets => self.list_wallets(),
            Command::SwitchWallet { name } => self.switch(name),
            command if self.wallet.is_none() => {
                tracing::warn!(?command, "ignored: wallet not open");
                self.fail(
                    Op::Open,
                    ErrorView::new("Wallet not open", "Open a wallet first."),
                );
            }
            Command::Sync { manual } => self.sync(manual),
            Command::RevealAddress => self.reveal_address(),
            Command::EstimateFee { target } => self.estimate_fee(target),
            Command::PreviewSend {
                recipient,
                fee_rate,
                selection,
            } => self.preview(recipient, fee_rate, selection),
            Command::ConfirmSend => self.confirm_send(),
            Command::CancelSend => self.pending = None,
            Command::BumpFee { txid, fee_rate } => self.bump(txid, fee_rate),
            Command::BroadcastSaved => self.broadcast_saved(),
            Command::Mine { blocks } => self.mine(blocks, false),
            Command::Faucet => self.mine(101, true),
        }
        true
    }

    fn wallet(&mut self) -> &mut Wallet {
        self.wallet.as_mut().expect("checked in handle")
    }

    fn snapshot(&mut self) -> Result<Snapshot, Error> {
        let wallet = self.wallet();
        let receive = wallet.new_address()?;
        let (total, height) = (wallet.balance().total().to_sat(), wallet.tip().height);
        // Remembered for wallet lists; losing it only costs a stale number.
        let _ = self.dir.record_sync(total, height);
        let wallet = self.wallet();
        Ok(Snapshot {
            network: wallet.network(),
            can_sign: wallet.can_sign(),
            tip: wallet.tip(),
            balance: wallet.balance(),
            utxos: wallet.list_utxos(),
            txs: wallet.transactions(),
            receive,
        })
    }

    fn offline_error(&self) -> ErrorView {
        ErrorView::new(
            "Offline",
            if self.can_retry() {
                "The node cannot be reached; showing stored data. Retrying in the background."
            } else {
                "No node configured; showing stored data."
            },
        )
    }

    // === Named wallets

    fn list_wallets(&mut self) {
        match &self.wallets {
            Some(wallets) => self.emit(WorkerEvent::Wallets {
                list: wallets.list(),
                current: self.name.clone(),
            }),
            None => self.fail(
                Op::Wallets,
                ErrorView::new(
                    "Switching unavailable",
                    "This wallet was opened by path (--datadir) or is the demo.",
                ),
            ),
        }
    }

    fn switch(&mut self, name: String) {
        let Some(wallets) = self.wallets.clone() else {
            return self.list_wallets();
        };
        if let Err(e) = crate::wallets::validate_name(&name) {
            return self.fail(Op::Wallets, ErrorView::new("Invalid wallet name", e));
        }
        // Close the current wallet; the chain connection stays.
        self.wallet = None;
        self.pending = None;
        self.dir = wallets.dir(&name);
        self.name = Some(name.clone());
        if !wallets.exists(&name) {
            return self.emit(WorkerEvent::Switched {
                name,
                kind: SwitchKind::New,
            });
        }
        match session::stored_keys(&self.dir) {
            Ok(session::StoredKeys::Encrypted) => self.emit(WorkerEvent::Switched {
                name,
                kind: SwitchKind::Locked,
            }),
            Ok(stored) => {
                self.emit(WorkerEvent::Switched {
                    name,
                    kind: SwitchKind::Opening,
                });
                let keys = match stored {
                    session::StoredKeys::Plain(keys) => Some(keys),
                    _ => None,
                };
                self.open(OpenRequest::Existing { keys });
            }
            Err(e) => self.fail(
                Op::Open,
                ErrorView::new("Could not read the wallet", format::chain(e.as_ref())),
            ),
        }
    }

    // === Commands

    fn open(&mut self, request: OpenRequest) {
        let result = match request {
            OpenRequest::Existing { keys } => session::open(&self.dir, self.network, keys),
            OpenRequest::Unlock { password } => match session::unlock(&self.dir, &password) {
                Ok(keys) => session::open(&self.dir, self.network, Some(keys)),
                Err(e) => {
                    return self.fail(
                        Op::Unlock,
                        ErrorView::new("Wrong password", format::chain(e.as_ref())),
                    );
                }
            },
            OpenRequest::Create(new) => session::create(&self.dir, self.network, new),
        };
        match result {
            Ok(wallet) => {
                self.wallet = Some(wallet);
                // Payments saved in an earlier session stay reserved, even
                // if the app stopped before the wallet was saved.
                for (_, tx) in self.saved() {
                    let _ = self.wallet().track_pending(&tx);
                }
                match self.snapshot() {
                    Ok(snapshot) => self.emit(WorkerEvent::Opened(snapshot)),
                    Err(e) => self.fail_with(Op::Open, e),
                }
                self.emit_outbox();
                self.flush_outbox();
            }
            Err(e) => self.fail(
                Op::Open,
                ErrorView::new("Could not open the wallet", format::chain(e.as_ref())),
            ),
        }
    }

    fn sync(&mut self, manual: bool) {
        let Some(chain) = self.live() else {
            return self.fail(Op::Sync, self.offline_error());
        };
        self.emit(WorkerEvent::SyncStarted { manual });
        let local = self
            .wallet
            .as_ref()
            .expect("checked in handle")
            .tip()
            .height;
        let total = match chain.backend.tip() {
            Ok(tip) => tip.height.saturating_sub(local),
            Err(e) => return self.fail_with(Op::Sync, e),
        };
        let events = self.events.clone();
        let chain = self.chain.as_ref().expect("live");
        let progress = Progress::new(chain.backend.as_ref(), self.config.progress_every, |done| {
            let _ = events.send(WorkerEvent::SyncProgress { done, total });
        });
        let result = self
            .wallet
            .as_mut()
            .expect("checked in handle")
            .sync(&progress);
        match result {
            Ok(report) => match self.snapshot() {
                Ok(snapshot) => self.emit(WorkerEvent::Synced {
                    report,
                    snapshot,
                    manual,
                }),
                Err(e) => self.fail_with(Op::Sync, e),
            },
            Err(e) => self.fail_with(Op::Sync, e),
        }
    }

    fn reveal_address(&mut self) {
        match self.wallet().reveal_next_address() {
            Ok(info) => self.emit(WorkerEvent::Address(info)),
            Err(e) => self.fail_with(Op::Address, e),
        }
    }

    fn estimate_fee(&mut self, target: u16) {
        let fallback = WorkerEvent::FeeEstimate {
            rate: self.config.fallback_fee_rate,
            fallback: true,
        };
        let Some(chain) = self.live() else {
            return self.emit(fallback);
        };
        match chain.backend.estimate_fee_rate(target) {
            Ok(rate) => self.emit(WorkerEvent::FeeEstimate {
                rate,
                fallback: false,
            }),
            Err(FeeEstimateError::Unavailable { .. }) => self.emit(fallback),
            Err(e) => {
                self.fail_with(Op::FeeEstimate, e);
                self.emit(fallback);
            }
        }
    }

    fn preview(
        &mut self,
        recipient: Recipient,
        fee_rate: FeeRate,
        selection: Option<CoinSelection>,
    ) {
        self.pending = None;
        let address = recipient.address.assume_checked_ref().to_string();
        let script = recipient.address.assume_checked_ref().script_pubkey();
        let amount = recipient.amount;
        let mut request = TxRequest::new(vec![recipient], fee_rate);
        request.coin_selection = selection;
        let wallet = self.wallet();
        let mut psbt = match wallet.build_tx_with(&request) {
            Ok(psbt) => psbt,
            Err(e) => return self.fail_with(Op::Preview, e),
        };
        // Sign now so the preview shows the real size and fee rate, and a
        // watch-only wallet is told before it gets to "confirm".
        match wallet.sign(&mut psbt) {
            Ok(SignOutcome::Finalized) => {}
            Ok(SignOutcome::Partial) => return self.fail_with(Op::Preview, Error::NotFinalized),
            Err(e) => return self.fail_with(Op::Preview, e),
        }
        let fee = psbt.fee().unwrap_or(Amount::ZERO);
        let tx = match psbt.clone().extract_tx() {
            Ok(tx) => tx,
            Err(e) => return self.fail_with(Op::Preview, Error::Extract(Box::new(e))),
        };
        let change: Amount = tx
            .output
            .iter()
            .filter(|o| o.script_pubkey != script)
            .map(|o| o.value)
            .sum();
        let preview = SendPreview {
            address,
            amount,
            fee,
            fee_rate: FeeRate::from_sat_per_kwu(fee.to_sat() * 1_000 / tx.weight().to_wu().max(1)),
            vsize: tx.vsize() as u64,
            inputs: tx.input.len(),
            change: (change > Amount::ZERO).then_some(change),
        };
        self.pending = Some(psbt);
        self.emit(WorkerEvent::Preview(preview));
    }

    fn confirm_send(&mut self) {
        let Some(psbt) = self.pending.take() else {
            return self.fail(
                Op::Send,
                ErrorView::new("Nothing to send", "Review a payment first."),
            );
        };
        let fee = psbt.fee().unwrap_or(Amount::ZERO);
        let tx = match psbt.extract_tx() {
            Ok(tx) => tx,
            Err(e) => return self.fail_with(Op::Send, Error::Extract(Box::new(e))),
        };
        let Some(chain) = self.live() else {
            return self.save(&tx);
        };
        match chain.backend.broadcast(&tx) {
            Ok(txid) => match self.refresh() {
                Ok(snapshot) => self.emit(WorkerEvent::Sent {
                    txid,
                    fee,
                    snapshot,
                }),
                Err(e) => self.fail_with(Op::Send, e),
            },
            // Lost the node mid-send: keep the signed payment instead of
            // making the user start over.
            Err(BroadcastError::Connection(e)) => {
                self.set_offline(format::chain(e.as_ref()));
                self.save(&tx);
            }
            Err(e) => self.fail_with(Op::Send, e),
        }
    }

    // === Outbox: payments signed while offline

    fn outbox(&self) -> PathBuf {
        self.dir.path().join("outbox")
    }

    fn save(&mut self, tx: &Transaction) {
        let txid = tx.compute_txid();
        // Named by save time so they are broadcast in order: a later payment
        // may spend the change of an earlier one.
        let saved_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let path = self.outbox().join(format!("{saved_at:020}-{txid}.hex"));
        let written = std::fs::create_dir_all(self.outbox())
            .and_then(|()| std::fs::write(&path, serialize_hex(tx)));
        match written {
            Ok(()) => {
                // Reserve its coins so the next payment cannot reuse them.
                if let Err(e) = self.wallet().track_pending(tx) {
                    return self.fail_with(Op::Outbox, e);
                }
                match self.snapshot() {
                    Ok(snapshot) => self.emit(WorkerEvent::Saved {
                        txid,
                        path,
                        snapshot,
                    }),
                    Err(e) => self.fail_with(Op::Outbox, e),
                }
                self.emit_outbox();
            }
            Err(e) => self.fail(
                Op::Outbox,
                ErrorView::new("Could not save the payment", e.to_string()),
            ),
        }
    }

    fn saved_files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(self.outbox())
            .map(|dir| {
                dir.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|x| x == "hex"))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        files
    }

    /// Saved payments that can be read back, with their files.
    fn saved(&self) -> Vec<(PathBuf, Transaction)> {
        self.saved_files()
            .into_iter()
            .filter_map(|path| {
                let hex = std::fs::read_to_string(&path).ok()?;
                let tx = deserialize_hex(hex.trim()).ok()?;
                Some((path, tx))
            })
            .collect()
    }

    /// Send saved payments as soon as we are online with a wallet open.
    /// They were already confirmed by the user, and a sync would otherwise
    /// drop them as unknown to the node.
    fn flush_outbox(&mut self) {
        if self.online && self.wallet.is_some() && !self.saved_files().is_empty() {
            self.broadcast_saved();
        }
    }

    fn emit_outbox(&self) {
        self.emit(WorkerEvent::Outbox {
            count: self.saved_files().len(),
        });
    }

    fn broadcast_saved(&mut self) {
        if self.live().is_none() {
            return self.fail(Op::Outbox, self.offline_error());
        }
        let mut sent = Vec::new();
        let mut rejected = Vec::new();
        for path in self.saved_files() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let tx: Transaction = match std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|hex| deserialize_hex(hex.trim()).map_err(|e| e.to_string()))
            {
                Ok(tx) => tx,
                Err(e) => {
                    rejected.push((name, format!("unreadable: {e}")));
                    let _ = move_to(&path, "failed");
                    continue;
                }
            };
            let chain = self.chain.as_ref().expect("live");
            match chain.backend.broadcast(&tx) {
                Ok(txid) => {
                    sent.push(txid);
                    let _ = move_to(&path, "sent");
                }
                Err(BroadcastError::Rejected { reason }) => {
                    rejected.push((name, reason));
                    let _ = move_to(&path, "failed");
                }
                Err(e) => return self.fail_with(Op::Outbox, e),
            }
        }
        match self.refresh() {
            Ok(snapshot) => self.emit(WorkerEvent::SavedBroadcast {
                sent,
                rejected,
                snapshot,
            }),
            Err(e) => self.fail_with(Op::Outbox, e),
        }
        self.emit_outbox();
    }

    fn bump(&mut self, original: Txid, fee_rate: FeeRate) {
        if self.live().is_none() {
            return self.fail(Op::Bump, self.offline_error());
        }
        let wallet = self.wallet();
        let mut psbt = match wallet.bump_fee(original, fee_rate) {
            Ok(psbt) => psbt,
            Err(e) => return self.fail_with(Op::Bump, e),
        };
        match wallet.sign(&mut psbt) {
            Ok(SignOutcome::Finalized) => {}
            Ok(SignOutcome::Partial) => return self.fail_with(Op::Bump, Error::NotFinalized),
            Err(e) => return self.fail_with(Op::Bump, e),
        }
        let tx = match psbt.extract_tx() {
            Ok(tx) => tx,
            Err(e) => return self.fail_with(Op::Bump, Error::Extract(Box::new(e))),
        };
        let chain = self.chain.as_ref().expect("live");
        let replacement = match chain.backend.broadcast(&tx) {
            Ok(txid) => txid,
            Err(e) => return self.fail_with(Op::Bump, e),
        };
        match self.refresh() {
            Ok(snapshot) => self.emit(WorkerEvent::Bumped {
                original,
                replacement,
                snapshot,
            }),
            Err(e) => self.fail_with(Op::Bump, e),
        }
    }

    fn mine(&mut self, blocks: u32, to_wallet: bool) {
        if self.live().is_none_or(|c| c.miner.is_none()) {
            return self.fail(
                Op::Mine,
                ErrorView::new(
                    "Mining unavailable",
                    "Mining needs the managed regtest node (--node local or --demo) to be running.",
                ),
            );
        }
        let to = if to_wallet {
            match self.wallet().new_address() {
                Ok(info) => info.address,
                Err(e) => return self.fail_with(Op::Mine, e),
            }
        } else {
            match self.burn_address() {
                Ok(address) => address,
                Err(error) => return self.fail(Op::Mine, error),
            }
        };
        let miner = self
            .chain
            .as_ref()
            .and_then(|c| c.miner.as_ref())
            .expect("checked above");
        if let Err(e) = miner.mine(blocks, &to) {
            return self.fail(
                Op::Mine,
                ErrorView::new("Mining failed", format::chain(e.as_ref())),
            );
        }
        self.emit(WorkerEvent::Mined { blocks });
        self.sync(false);
    }

    /// An address from a throwaway in-memory wallet with fresh keys.
    fn burn_address(&mut self) -> Result<Address, ErrorView> {
        if let Some(address) = &self.burn_address {
            return Ok(address.clone());
        }
        let make = || -> Result<Address, wallet::BoxError> {
            let phrase = generate_mnemonic(MnemonicLength::Words12)?;
            let mut miner = Wallet::builder(self.network)
                .keys(KeySource::mnemonic(phrase.as_str()))
                .create()?;
            Ok(miner.new_address()?.address)
        };
        let address = make().map_err(|e| ErrorView::new("Mining failed", e.to_string()))?;
        self.burn_address = Some(address.clone());
        Ok(address)
    }

    /// Sync quietly after a broadcast so the new transaction shows up.
    fn refresh(&mut self) -> Result<Snapshot, Error> {
        let backend = self.chain.as_ref().expect("online").backend.as_ref();
        let wallet = self.wallet.as_mut().expect("checked in handle");
        wallet.sync(&backend)?;
        self.snapshot()
    }
}

/// Move `path` into a sibling folder (`sent`, `failed`) of its directory.
fn move_to(path: &Path, folder: &str) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    let target = parent.join(folder);
    std::fs::create_dir_all(&target)?;
    std::fs::rename(path, target.join(path.file_name().unwrap_or_default()))
}

/// A worker running on its own thread.
pub struct WorkerHandle {
    commands: Sender<Command>,
    events: Receiver<WorkerEvent>,
    thread: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    /// Start the thread. It tries the node first and reports the result as a
    /// [`WorkerEvent::Connection`].
    pub fn spawn(spec: WorkerSpec) -> Self {
        let (command_tx, command_rx) = mpsc::channel::<Command>();
        let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>();
        let thread = std::thread::Builder::new()
            .name("wallet-worker".into())
            .spawn(move || run(spec, command_rx, event_tx))
            .expect("spawn worker thread");
        Self {
            commands: command_tx,
            events: event_rx,
            thread: Some(thread),
        }
    }

    /// Queue a command.
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    /// The next event, if one is waiting.
    pub fn try_recv(&self) -> Option<WorkerEvent> {
        self.events.try_recv().ok()
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(spec: WorkerSpec, commands: Receiver<Command>, events: Sender<WorkerEvent>) {
    let mut worker = Worker::new(
        None,
        spec.connect,
        spec.dir,
        spec.network,
        spec.config,
        events,
    )
    .with_wallets(spec.wallets, spec.name);
    worker.reconnect();
    for command in commands {
        if !worker.handle(command) {
            break;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use wallet::bitcoin::Block;
    use wallet::testkit::MockChain;
    use wallet::{BlockId, BlockSource, BoxError, BroadcastError, Broadcaster, FeeEstimator};
    use wallet::{FeeEstimateError, SourceError, TxStatus};
    use zeroize::Zeroizing;

    use super::*;
    use crate::session::NewWallet;
    use crate::tui::chain::Miner;

    /// A `MockChain` shared between the worker and the test, with a switch
    /// to make it unreachable.
    #[derive(Clone)]
    pub(crate) struct Shared(pub Rc<RefCell<MockChain>>, pub Rc<Cell<bool>>);

    impl Shared {
        fn down(&self) -> Option<SourceError> {
            (!self.1.get()).then(|| SourceError::Connection("connection refused".into()))
        }
    }

    impl BlockSource for Shared {
        fn tip(&self) -> Result<BlockId, SourceError> {
            if let Some(e) = self.down() {
                return Err(e);
            }
            self.0.borrow().tip()
        }
        fn block_hash(&self, h: u32) -> Result<wallet::bitcoin::BlockHash, SourceError> {
            if let Some(e) = self.down() {
                return Err(e);
            }
            self.0.borrow().block_hash(h)
        }
        fn block(&self, hash: &wallet::bitcoin::BlockHash) -> Result<Block, SourceError> {
            if let Some(e) = self.down() {
                return Err(e);
            }
            self.0.borrow().block(hash)
        }
        fn mempool(&self) -> Result<Vec<(wallet::bitcoin::Transaction, u64)>, SourceError> {
            if let Some(e) = self.down() {
                return Err(e);
            }
            self.0.borrow().mempool()
        }
    }

    impl Broadcaster for Shared {
        fn broadcast(&self, tx: &wallet::bitcoin::Transaction) -> Result<Txid, BroadcastError> {
            if self.down().is_some() {
                return Err(BroadcastError::Connection("connection refused".into()));
            }
            self.0.borrow().broadcast(tx)
        }
    }

    impl FeeEstimator for Shared {
        fn estimate_fee_rate(&self, target: u16) -> Result<FeeRate, FeeEstimateError> {
            if self.down().is_some() {
                return Err(FeeEstimateError::Connection("connection refused".into()));
            }
            self.0.borrow().estimate_fee_rate(target)
        }
    }

    impl Miner for Shared {
        // The mock pays rewards as a normal output (no coinbase maturity).
        fn mine(&self, blocks: u32, to: &Address) -> Result<(), BoxError> {
            let mut chain = self.0.borrow_mut();
            chain.fund(to.script_pubkey(), Amount::from_int_btc(50));
            chain.mine_empty(blocks.saturating_sub(1));
            Ok(())
        }
    }

    pub(crate) struct Harness {
        pub worker: Worker,
        pub events: Receiver<WorkerEvent>,
        pub chain: Shared,
        pub _dir: tempfile::TempDir,
    }

    impl Harness {
        pub fn new(with_miner: bool) -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let dir = DataDir::at(tmp.path().join("w"));
            let shared = Shared(
                Rc::new(RefCell::new(MockChain::new(Network::Regtest))),
                Rc::new(Cell::new(true)),
            );
            let chain = Chain {
                backend: Box::new(shared.clone()),
                miner: with_miner.then(|| Box::new(shared.clone()) as Box<dyn Miner>),
                label: "mock".into(),
                _keepalive: None,
            };
            let (tx, rx) = mpsc::channel();
            let config = WorkerConfig {
                fallback_fee_rate: FeeRate::from_sat_per_vb_u32(3),
                progress_every: Duration::ZERO,
            };
            let mut worker = Worker::new(Some(chain), None, dir, Network::Regtest, config, tx);
            worker.reconnect();
            let h = Self {
                worker,
                events: rx,
                chain: shared,
                _dir: tmp,
            };
            h.events.try_iter().for_each(drop);
            h
        }

        pub fn opened(with_miner: bool) -> Self {
            let mut h = Self::new(with_miner);
            let phrase = generate_mnemonic(MnemonicLength::Words12).unwrap();
            h.run(Command::Open(OpenRequest::Create(NewWallet {
                keys: KeySource::mnemonic(phrase.as_str()),
                birthday: None,
                password: None,
            })));
            h
        }

        /// Handle a command and return the events it produced.
        pub fn run(&mut self, command: Command) -> Vec<WorkerEvent> {
            self.worker.handle(command);
            self.events.try_iter().collect()
        }

        pub fn fund(&mut self, sats: u64) {
            let address = self.worker.wallet().new_address().unwrap().address;
            self.chain
                .0
                .borrow_mut()
                .fund(address.script_pubkey(), Amount::from_sat(sats));
        }
    }

    fn last_snapshot(events: &[WorkerEvent]) -> &Snapshot {
        events
            .iter()
            .rev()
            .find_map(|e| match e {
                WorkerEvent::Opened(s)
                | WorkerEvent::Synced { snapshot: s, .. }
                | WorkerEvent::Sent { snapshot: s, .. }
                | WorkerEvent::Bumped { snapshot: s, .. } => Some(s),
                _ => None,
            })
            .expect("a snapshot")
    }

    fn failure(events: &[WorkerEvent]) -> (Op, &ErrorView) {
        events
            .iter()
            .find_map(|e| match e {
                WorkerEvent::Failed { op, error } => Some((*op, error)),
                _ => None,
            })
            .expect("a failure")
    }

    fn outside_address() -> Recipient {
        let phrase = generate_mnemonic(MnemonicLength::Words12).unwrap();
        let mut other = Wallet::builder(Network::Regtest)
            .keys(KeySource::mnemonic(phrase.as_str()))
            .create()
            .unwrap();
        let address = other.new_address().unwrap().address.into_unchecked();
        Recipient::new(address, Amount::from_sat(30_000))
    }

    #[test]
    fn commands_before_open_are_refused() {
        let mut h = Harness::new(false);
        let events = h.run(Command::Sync { manual: true });
        assert_eq!(failure(&events).0, Op::Open);
    }

    #[test]
    fn create_then_sync_reports_progress_and_balance() {
        let mut h = Harness::opened(false);
        h.fund(80_000);
        h.chain.0.borrow_mut().mine_empty(2);
        let events = h.run(Command::Sync { manual: true });

        assert_eq!(events[0], WorkerEvent::SyncStarted { manual: true });
        let progress: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                WorkerEvent::SyncProgress { done, total } => Some((*done, *total)),
                _ => None,
            })
            .collect();
        assert_eq!(progress.last(), Some(&(3, 3)));
        let snapshot = last_snapshot(&events);
        assert_eq!(snapshot.balance.confirmed, Amount::from_sat(80_000));
        assert_eq!(snapshot.tip.height, 3);
        assert!(snapshot.can_sign);
    }

    #[test]
    fn preview_then_confirm_sends_and_updates() {
        let mut h = Harness::opened(false);
        h.fund(100_000);
        h.run(Command::Sync { manual: false });

        let events = h.run(Command::PreviewSend {
            recipient: outside_address(),
            fee_rate: FeeRate::from_sat_per_vb_u32(2),
            selection: Some(CoinSelection::LargestFirst),
        });
        let WorkerEvent::Preview(preview) = &events[0] else {
            panic!("expected preview, got {events:?}");
        };
        assert_eq!(preview.amount, Amount::from_sat(30_000));
        assert_eq!(preview.inputs, 1);
        assert_eq!(
            preview.change,
            Some(Amount::from_sat(100_000 - 30_000) - preview.fee)
        );
        assert!(preview.fee_rate >= FeeRate::from_sat_per_vb_u32(2));

        let events = h.run(Command::ConfirmSend);
        let WorkerEvent::Sent { txid, snapshot, .. } = &events[0] else {
            panic!("expected sent, got {events:?}");
        };
        let tx = snapshot.txs.iter().find(|t| t.txid == *txid).unwrap();
        assert_eq!(tx.status, TxStatus::Unconfirmed);

        // The pending payment is consumed.
        let again = h.run(Command::ConfirmSend);
        assert_eq!(failure(&again).0, Op::Send);
    }

    #[test]
    fn typed_errors_reach_the_ui_as_friendly_messages() {
        let mut h = Harness::opened(false);
        h.fund(10_000);
        h.run(Command::Sync { manual: false });
        let events = h.run(Command::PreviewSend {
            recipient: outside_address(),
            fee_rate: FeeRate::from_sat_per_vb_u32(2),
            selection: None,
        });
        let (op, error) = failure(&events);
        assert_eq!(op, Op::Preview);
        assert_eq!(error.title, "Insufficient funds");
    }

    #[test]
    fn bump_replaces_a_pending_payment() {
        let mut h = Harness::opened(false);
        h.fund(100_000);
        h.run(Command::Sync { manual: false });
        h.run(Command::PreviewSend {
            recipient: outside_address(),
            fee_rate: FeeRate::from_sat_per_vb_u32(1),
            selection: None,
        });
        let sent = h.run(Command::ConfirmSend);
        let WorkerEvent::Sent { txid, .. } = sent[0] else {
            panic!("{sent:?}");
        };
        let events = h.run(Command::BumpFee {
            txid,
            fee_rate: FeeRate::from_sat_per_vb_u32(8),
        });
        let WorkerEvent::Bumped {
            original,
            replacement,
            snapshot,
        } = &events[0]
        else {
            panic!("{events:?}");
        };
        assert_eq!(*original, txid);
        assert!(snapshot.txs.iter().any(|t| t.txid == *replacement));
        assert!(!snapshot.txs.iter().any(|t| t.txid == txid));
    }

    #[test]
    fn fee_estimate_falls_back_when_the_node_has_none() {
        let mut h = Harness::opened(false);
        assert_eq!(
            h.run(Command::EstimateFee { target: 6 }),
            vec![WorkerEvent::FeeEstimate {
                rate: FeeRate::from_sat_per_vb_u32(2),
                fallback: false
            }]
        );
        h.chain.0.borrow_mut().set_fee_rate(None);
        assert_eq!(
            h.run(Command::EstimateFee { target: 6 }),
            vec![WorkerEvent::FeeEstimate {
                rate: FeeRate::from_sat_per_vb_u32(3),
                fallback: true
            }]
        );
    }

    #[test]
    fn faucet_funds_the_wallet_and_mine_confirms() {
        let mut h = Harness::opened(true);
        let events = h.run(Command::Faucet);
        assert!(events.contains(&WorkerEvent::Mined { blocks: 101 }));
        assert_eq!(
            last_snapshot(&events).balance.confirmed,
            Amount::from_int_btc(50)
        );

        let events = h.run(Command::Mine { blocks: 1 });
        let snapshot = last_snapshot(&events);
        assert_eq!(
            snapshot.balance.confirmed,
            Amount::from_int_btc(50),
            "mined rewards go to a throwaway address"
        );
    }

    #[test]
    fn mining_without_a_demo_node_is_refused() {
        let mut h = Harness::opened(false);
        let events = h.run(Command::Faucet);
        assert_eq!(failure(&events).0, Op::Mine);
    }

    #[test]
    fn encrypted_wallet_unlocks_only_with_the_right_password() {
        let mut h = Harness::new(false);
        let phrase = generate_mnemonic(MnemonicLength::Words12).unwrap();
        h.run(Command::Open(OpenRequest::Create(NewWallet {
            keys: KeySource::mnemonic(phrase.as_str()),
            birthday: None,
            password: Some(Zeroizing::new("correct horse".into())),
        })));

        let wrong = h.run(Command::Open(OpenRequest::Unlock {
            password: Zeroizing::new("nope".into()),
        }));
        let (op, error) = failure(&wrong);
        assert_eq!((op, error.title.as_str()), (Op::Unlock, "Wrong password"));

        let right = h.run(Command::Open(OpenRequest::Unlock {
            password: Zeroizing::new("correct horse".into()),
        }));
        assert!(last_snapshot(&right).can_sign);

        let watch_only = h.run(Command::Open(OpenRequest::Existing { keys: None }));
        assert!(!last_snapshot(&watch_only).can_sign);
    }

    fn set_online(h: &Harness, up: bool) {
        h.chain.1.set(up);
    }

    fn connection(events: &[WorkerEvent]) -> Option<&Connection> {
        events.iter().find_map(|e| match e {
            WorkerEvent::Connection(c) => Some(c),
            _ => None,
        })
    }

    #[test]
    fn losing_the_node_flips_offline_and_sync_explains() {
        let mut h = Harness::opened(false);
        set_online(&h, false);
        let events = h.run(Command::Sync { manual: true });
        assert!(matches!(
            connection(&events),
            Some(Connection::Offline { retrying: true, .. })
        ));
        // While offline, sync does not even try.
        let events = h.run(Command::Sync { manual: true });
        assert_eq!(failure(&events).1.title, "Offline");
        assert!(connection(&events).is_none(), "no repeated offline event");
    }

    #[test]
    fn offline_payments_are_reserved_saved_and_sent_on_reconnect() {
        let mut h = Harness::opened(false);
        h.fund(100_000);
        h.run(Command::Sync { manual: false });
        set_online(&h, false);
        h.run(Command::Sync { manual: false });

        // Fees fall back to the configured rate while offline.
        assert_eq!(
            h.run(Command::EstimateFee { target: 6 }),
            vec![WorkerEvent::FeeEstimate {
                rate: FeeRate::from_sat_per_vb_u32(3),
                fallback: true
            }]
        );

        h.run(Command::PreviewSend {
            recipient: outside_address(),
            fee_rate: FeeRate::from_sat_per_vb_u32(2),
            selection: None,
        });
        let events = h.run(Command::ConfirmSend);
        let WorkerEvent::Saved {
            txid,
            path,
            snapshot,
        } = &events[0]
        else {
            panic!("{events:?}");
        };
        let (txid, path) = (*txid, path.clone());
        assert!(path.exists());
        assert!(events.contains(&WorkerEvent::Outbox { count: 1 }));
        // The saved payment counts: its coin is spent, the change is pending.
        assert_eq!(snapshot.balance.confirmed, Amount::ZERO);
        assert!(snapshot.balance.unconfirmed > Amount::ZERO);

        // A second offline payment spends the first one's pending change
        // (never the already-spent coin), so the two chain cleanly.
        h.run(Command::PreviewSend {
            recipient: outside_address(),
            fee_rate: FeeRate::from_sat_per_vb_u32(2),
            selection: None,
        });
        let events = h.run(Command::ConfirmSend);
        let WorkerEvent::Saved { txid: child, .. } = events[0] else {
            panic!("{events:?}");
        };
        assert!(events.contains(&WorkerEvent::Outbox { count: 2 }));
        // Nothing left to spend: both coins are reserved.
        let events = h.run(Command::PreviewSend {
            recipient: Recipient::new(outside_address().address, Amount::from_sat(60_000)),
            fee_rate: FeeRate::from_sat_per_vb_u32(2),
            selection: None,
        });
        assert_eq!(failure(&events).1.title, "Insufficient funds");

        // Broadcasting needs the node.
        let events = h.run(Command::BroadcastSaved);
        assert_eq!(failure(&events).0, Op::Outbox);

        // Back online: saved payments go out by themselves, before any sync.
        set_online(&h, true);
        let events = h.run(Command::Reconnect);
        assert!(matches!(
            connection(&events),
            Some(Connection::Online { .. })
        ));
        let broadcast = events
            .iter()
            .find_map(|e| match e {
                WorkerEvent::SavedBroadcast {
                    sent,
                    rejected,
                    snapshot,
                } => Some((sent, rejected, snapshot)),
                _ => None,
            })
            .expect("automatic broadcast on reconnect");
        assert_eq!(broadcast.0, &vec![txid, child], "parent first, then child");
        assert!(broadcast.1.is_empty());
        assert!(
            broadcast.2.txs.iter().any(|t| t.txid == txid),
            "still listed after the sync"
        );
        assert!(broadcast.2.txs.iter().any(|t| t.txid == child));
        assert!(events.contains(&WorkerEvent::Outbox { count: 0 }));
        assert!(
            path.parent()
                .unwrap()
                .join("sent")
                .join(path.file_name().unwrap())
                .exists()
        );

        // A later sync keeps it, because the node now has it.
        let events = h.run(Command::Sync { manual: false });
        let synced = events.iter().find_map(|e| match e {
            WorkerEvent::Synced { snapshot, .. } => Some(snapshot),
            _ => None,
        });
        assert!(synced.unwrap().txs.iter().any(|t| t.txid == txid));
    }

    #[test]
    fn losing_the_node_during_send_keeps_the_signed_payment() {
        let mut h = Harness::opened(false);
        h.fund(100_000);
        h.run(Command::Sync { manual: false });
        h.run(Command::PreviewSend {
            recipient: outside_address(),
            fee_rate: FeeRate::from_sat_per_vb_u32(2),
            selection: None,
        });
        set_online(&h, false);
        let events = h.run(Command::ConfirmSend);
        assert!(matches!(
            connection(&events),
            Some(Connection::Offline { .. })
        ));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WorkerEvent::Saved { .. }))
        );
    }

    #[test]
    fn without_a_node_the_wallet_still_opens() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = DataDir::at(tmp.path().join("w"));
        let (tx, rx) = mpsc::channel();
        let config = WorkerConfig {
            fallback_fee_rate: FeeRate::from_sat_per_vb_u32(3),
            progress_every: Duration::ZERO,
        };
        let mut worker = Worker::new(None, None, dir, Network::Regtest, config, tx);
        worker.reconnect();
        let events: Vec<_> = rx.try_iter().collect();
        assert!(matches!(
            connection(&events),
            Some(Connection::Offline {
                retrying: false,
                ..
            })
        ));

        let phrase = generate_mnemonic(MnemonicLength::Words12).unwrap();
        worker.handle(Command::Open(OpenRequest::Create(NewWallet {
            keys: KeySource::mnemonic(phrase.as_str()),
            birthday: None,
            password: None,
        })));
        worker.handle(Command::RevealAddress);
        let events: Vec<_> = rx.try_iter().collect();
        assert!(events.iter().any(|e| matches!(e, WorkerEvent::Opened(_))));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, WorkerEvent::Address(a) if a.index == 1))
        );
    }

    #[test]
    fn switching_wallets_creates_and_reopens_by_name() {
        let mut h = Harness::new(false);
        let root = h._dir.path().join("home");
        let wallets = Wallets::new(Some(root), Network::Regtest);
        h.worker.wallets = Some(wallets.clone());
        h.worker.name = None;

        // A new name: setup runs, then the wallet is created there.
        let events = h.run(Command::SwitchWallet {
            name: "alice".into(),
        });
        assert_eq!(
            events,
            vec![WorkerEvent::Switched {
                name: "alice".into(),
                kind: SwitchKind::New
            }]
        );
        let phrase = generate_mnemonic(MnemonicLength::Words12).unwrap();
        h.run(Command::Open(OpenRequest::Create(NewWallet {
            keys: KeySource::mnemonic(phrase.as_str()),
            birthday: None,
            password: None,
        })));
        assert!(wallets.exists("alice"));

        h.run(Command::SwitchWallet { name: "bob".into() });
        let events = h.run(Command::SwitchWallet {
            name: "alice".into(),
        });
        assert_eq!(
            events[0],
            WorkerEvent::Switched {
                name: "alice".into(),
                kind: SwitchKind::Opening
            }
        );
        assert!(
            matches!(events[1], WorkerEvent::Opened(_)),
            "plain keys open directly"
        );

        let events = h.run(Command::ListWallets);
        let WorkerEvent::Wallets { list, current } = &events[0] else {
            panic!("{events:?}");
        };
        assert_eq!(current.as_deref(), Some("alice"));
        assert_eq!(list.len(), 1, "bob was never created");
        assert!(
            list[0].last_balance_sat.is_some(),
            "balance recorded on open"
        );
    }

    #[test]
    fn shutdown_stops_the_loop() {
        let mut h = Harness::new(false);
        assert!(!h.worker.handle(Command::Shutdown));
    }
}
