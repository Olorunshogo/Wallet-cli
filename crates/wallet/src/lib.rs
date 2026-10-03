//! A small, modular Bitcoin wallet library.
//!
//! [`Wallet`] is the entry point. It is generic over a [`WalletEngine`]; v1
//! ships [`BdkEngine`], which wraps `bdk_wallet`. Chain access goes through the
//! [`BlockSource`], [`Broadcaster`] and [`FeeEstimator`] traits, with a Bitcoin
//! Core implementation in [`rpc`] (feature `rpc`, on by default).
//!
//! ```no_run
//! use std::env;
//!
//! use wallet::bitcoin::{Amount, FeeRate, Network};
//! use wallet::rpc::{RpcAuth, RpcClient};
//! use wallet::{KeySource, Recipient, Wallet};
//!
//! let url = env::var("WALLET_RPC_URL")?;
//! let cookie = env::var("WALLET_RPC_COOKIE")?;
//! let node = RpcClient::new(&url, RpcAuth::Cookie(cookie.into()))?;
//! // A fresh random mnemonic; store it safely, it is the wallet's backup.
//! let mnemonic = wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?;
//! let mut wallet = Wallet::builder(Network::Regtest)
//!     .keys(KeySource::mnemonic(mnemonic.as_str()))
//!     .database("wallet.sqlite")
//!     .create_or_load()?;
//!
//! let receive = wallet.new_address()?;
//! wallet.sync(&node)?;
//! println!("{} -> {}", receive.address, wallet.balance().confirmed);
//!
//! let to = "bcrt1q...".parse()?;
//! let fee_rate = FeeRate::from_sat_per_vb(2).expect("valid rate");
//! let txid = wallet.send([Recipient::new(to, Amount::from_sat(10_000))], fee_rate, &node)?;
//! # Ok::<(), wallet::BoxError>(())
//! ```

#![warn(missing_docs)]
// Explicit returns are a project convention; clippy's needless_return lint
// disagrees, so allow it crate-wide.
#![allow(clippy::needless_return)]

pub use wallet_bdk::{BdkEngine, BdkEngineBuilder, generate_mnemonic, validate_mnemonic};
pub use wallet_core::bitcoin;
pub use wallet_core::{
    AddressError, AddressInfo, Balance, BlockId, BlockSource, BoxError, BroadcastError,
    Broadcaster, BuildTxError, BumpFeeError, CoinSelection, CreateError, Error, FeeEstimateError,
    FeeEstimator, KeySource, Keychain, LoadError, MnemonicLength, Recipient, SignError,
    SignOutcome, SourceError, SyncError, SyncReport, TrackTxError, TxDetails, TxRequest, TxStatus,
    Utxo, WalletEngine,
};

/// An in-memory chain for testing code built on this library, no node needed.
///
/// [`MockChain`](testkit::MockChain) implements [`BlockSource`],
/// [`Broadcaster`] and [`FeeEstimator`], so it can stand in for a real node.
pub mod testkit {
    pub use wallet_core::testkit::MockChain;
}

/// Bitcoin Core JSON-RPC backend.
#[cfg(feature = "rpc")]
pub mod rpc {
    pub use wallet_rpc::{DEFAULT_TIMEOUT, RpcAuth, RpcClient};
}

use std::path::PathBuf;

use bitcoin::{FeeRate, Network, Psbt, Transaction, Txid};

/// A wallet driven by engine `E`.
///
/// The methods here are thin: each one delegates to the engine, except the
/// `send` convenience, which chains build, sign and broadcast.
pub struct Wallet<E: WalletEngine = BdkEngine> {
    engine: E,
}

impl Wallet<BdkEngine> {
    /// Start configuring a BDK-backed wallet.
    pub fn builder(network: Network) -> WalletBuilder {
        WalletBuilder {
            inner: BdkEngine::builder(network),
        }
    }

    /// Public descriptors for both keychains, for creating a watch-only copy.
    pub fn public_descriptors(&self) -> (String, String) {
        self.engine.public_descriptors()
    }
}

impl<E: WalletEngine> Wallet<E> {
    /// Wrap any engine, e.g. a custom one or the future native engine.
    pub fn from_engine(engine: E) -> Self {
        Self { engine }
    }

    /// The engine behind this wallet, for engine-specific features.
    pub fn engine(&self) -> &E {
        &self.engine
    }

    /// Unwrap into the engine.
    pub fn into_engine(self) -> E {
        self.engine
    }

    /// Network the wallet was created for.
    pub fn network(&self) -> Network {
        self.engine.network()
    }

    /// Whether the wallet holds private keys (false when watch-only).
    pub fn can_sign(&self) -> bool {
        self.engine.can_sign()
    }

    /// The latest block the wallet has synced to.
    pub fn tip(&self) -> BlockId {
        self.engine.tip()
    }

    /// Next unused receive address.
    ///
    /// Returns the same address until it receives a payment, so calling it
    /// repeatedly (e.g. every time a screen opens) does not waste addresses.
    ///
    /// ```
    /// # use wallet::bitcoin::{Amount, Network};
    /// # use wallet::testkit::MockChain;
    /// # use wallet::{KeySource, Wallet};
    /// # let mut wallet = Wallet::builder(Network::Regtest)
    /// #     .keys(KeySource::mnemonic(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.as_str()))
    /// #     .create()?;
    /// # let mut chain = MockChain::new(Network::Regtest);
    /// let first = wallet.new_address()?;
    /// assert_eq!(wallet.new_address()?, first);
    /// # Ok::<(), wallet::BoxError>(())
    /// ```
    pub fn new_address(&mut self) -> Result<AddressInfo, AddressError> {
        self.engine.new_address()
    }

    /// Always reveal a fresh receive address, even if earlier ones are unused.
    ///
    /// ```
    /// # use wallet::bitcoin::{Amount, Network};
    /// # use wallet::testkit::MockChain;
    /// # use wallet::{KeySource, Wallet};
    /// # let mut wallet = Wallet::builder(Network::Regtest)
    /// #     .keys(KeySource::mnemonic(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.as_str()))
    /// #     .create()?;
    /// # let mut chain = MockChain::new(Network::Regtest);
    /// let a = wallet.reveal_next_address()?;
    /// let b = wallet.reveal_next_address()?;
    /// assert_eq!(b.index, a.index + 1);
    /// # Ok::<(), wallet::BoxError>(())
    /// ```
    pub fn reveal_next_address(&mut self) -> Result<AddressInfo, AddressError> {
        self.engine.reveal_next_address()
    }

    /// Balance as of the last [`sync`](Self::sync).
    ///
    /// ```
    /// # use wallet::bitcoin::{Amount, Network};
    /// # use wallet::testkit::MockChain;
    /// # use wallet::{KeySource, Wallet};
    /// # let mut wallet = Wallet::builder(Network::Regtest)
    /// #     .keys(KeySource::mnemonic(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.as_str()))
    /// #     .create()?;
    /// # let mut chain = MockChain::new(Network::Regtest);
    /// # let a = wallet.new_address()?.address;
    /// # chain.fund(a.script_pubkey(), Amount::from_sat(100_000));
    /// # wallet.sync(&chain)?;
    /// let balance = wallet.balance();
    /// assert_eq!(balance.confirmed, Amount::from_sat(100_000));
    /// assert_eq!(balance.total(), Amount::from_sat(100_000));
    /// # Ok::<(), wallet::BoxError>(())
    /// ```
    pub fn balance(&self) -> Balance {
        self.engine.balance()
    }

    /// Unspent outputs the wallet can spend, confirmed or not.
    ///
    /// ```
    /// # use wallet::bitcoin::{Amount, Network};
    /// # use wallet::testkit::MockChain;
    /// # use wallet::{KeySource, Wallet};
    /// # let mut wallet = Wallet::builder(Network::Regtest)
    /// #     .keys(KeySource::mnemonic(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.as_str()))
    /// #     .create()?;
    /// # let mut chain = MockChain::new(Network::Regtest);
    /// # let a = wallet.new_address()?.address;
    /// # chain.fund(a.script_pubkey(), Amount::from_sat(100_000));
    /// # wallet.sync(&chain)?;
    /// let utxos = wallet.list_utxos();
    /// assert_eq!(utxos.len(), 1);
    /// assert_eq!(utxos[0].value, Amount::from_sat(100_000));
    /// # Ok::<(), wallet::BoxError>(())
    /// ```
    pub fn list_utxos(&self) -> Vec<Utxo> {
        self.engine.list_utxos()
    }

    /// Wallet transactions: unconfirmed first, then newest block first.
    ///
    /// ```
    /// # use wallet::bitcoin::{Amount, Network};
    /// # use wallet::testkit::MockChain;
    /// # use wallet::{KeySource, Wallet};
    /// # let mut wallet = Wallet::builder(Network::Regtest)
    /// #     .keys(KeySource::mnemonic(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.as_str()))
    /// #     .create()?;
    /// # let mut chain = MockChain::new(Network::Regtest);
    /// # let a = wallet.new_address()?.address;
    /// # chain.fund(a.script_pubkey(), Amount::from_sat(100_000));
    /// # wallet.sync(&chain)?;
    /// let history = wallet.transactions();
    /// assert_eq!(history[0].received, Amount::from_sat(100_000));
    /// # Ok::<(), wallet::BoxError>(())
    /// ```
    pub fn transactions(&self) -> Vec<TxDetails> {
        self.engine.transactions()
    }

    /// Confirmation state of a wallet transaction, or `None` if the wallet
    /// has not seen it.
    ///
    /// ```
    /// # use wallet::bitcoin::{Amount, Network};
    /// # use wallet::testkit::MockChain;
    /// # use wallet::{KeySource, Wallet};
    /// # let mut wallet = Wallet::builder(Network::Regtest)
    /// #     .keys(KeySource::mnemonic(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.as_str()))
    /// #     .create()?;
    /// # let mut chain = MockChain::new(Network::Regtest);
    /// # let a = wallet.new_address()?.address;
    /// # chain.fund(a.script_pubkey(), Amount::from_sat(100_000));
    /// # wallet.sync(&chain)?;
    /// let txid = wallet.transactions()[0].txid;
    /// assert!(wallet.tx_status(txid).unwrap().is_confirmed());
    /// # Ok::<(), wallet::BoxError>(())
    /// ```
    pub fn tx_status(&self, txid: Txid) -> Option<TxStatus> {
        self.engine.tx_status(txid)
    }

    /// Bring the wallet up to date with a chain backend.
    ///
    /// Fetches blocks since the last sync (handling reorgs) plus the mempool,
    /// then saves the result. Any [`BlockSource`] works: `rpc::RpcClient` for
    /// Bitcoin Core, [`testkit::MockChain`] for tests, or your own.
    ///
    /// ```
    /// # use wallet::bitcoin::{Amount, Network};
    /// # use wallet::testkit::MockChain;
    /// # use wallet::{KeySource, Wallet};
    /// # let mut wallet = Wallet::builder(Network::Regtest)
    /// #     .keys(KeySource::mnemonic(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.as_str()))
    /// #     .create()?;
    /// # let mut chain = MockChain::new(Network::Regtest);
    /// let address = wallet.new_address()?.address;
    /// chain.fund(address.script_pubkey(), Amount::from_sat(50_000));
    ///
    /// let report = wallet.sync(&chain)?;
    /// assert_eq!(report.blocks_applied, 1);
    /// assert_eq!(wallet.balance().confirmed, Amount::from_sat(50_000));
    /// # Ok::<(), wallet::BoxError>(())
    /// ```
    pub fn sync(&mut self, source: &impl BlockSource) -> Result<SyncReport, SyncError> {
        self.engine.sync(source)
    }

    /// Build an unsigned PSBT paying `recipients` at `fee_rate`.
    ///
    /// Coins are chosen with the wallet's default strategy and change goes to a
    /// fresh change address. Addresses are checked against the wallet network.
    ///
    /// ```
    /// # use wallet::bitcoin::{Amount, Network};
    /// # use wallet::testkit::MockChain;
    /// # use wallet::{KeySource, Wallet};
    /// # let mut wallet = Wallet::builder(Network::Regtest)
    /// #     .keys(KeySource::mnemonic(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.as_str()))
    /// #     .create()?;
    /// # let mut chain = MockChain::new(Network::Regtest);
    /// # let a = wallet.new_address()?.address;
    /// # chain.fund(a.script_pubkey(), Amount::from_sat(100_000));
    /// # wallet.sync(&chain)?;
    /// # let mut other = Wallet::builder(Network::Regtest)
    /// #     .keys(KeySource::mnemonic(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.as_str()))
    /// #     .create()?;
    /// # let to = other.new_address()?.address.into_unchecked();
    /// use wallet::Recipient;
    /// use wallet::bitcoin::FeeRate;
    ///
    /// let psbt = wallet.build_tx(
    ///     [Recipient::new(to, Amount::from_sat(20_000))],
    ///     FeeRate::from_sat_per_vb_u32(2),
    /// )?;
    /// assert_eq!(psbt.unsigned_tx.output.len(), 2); // payment + change
    /// # Ok::<(), wallet::BoxError>(())
    /// ```
    pub fn build_tx(
        &mut self,
        recipients: impl IntoIterator<Item = Recipient>,
        fee_rate: FeeRate,
    ) -> Result<Psbt, BuildTxError> {
        let request = TxRequest::new(recipients.into_iter().collect(), fee_rate);
        self.engine.build_tx(&request)
    }

    /// Build with full control over coin selection and RBF.
    pub fn build_tx_with(&mut self, request: &TxRequest) -> Result<Psbt, BuildTxError> {
        self.engine.build_tx(request)
    }

    /// Sign every input the wallet owns and finalize when complete.
    ///
    /// ```
    /// # use wallet::bitcoin::{Amount, Network};
    /// # use wallet::testkit::MockChain;
    /// # use wallet::{KeySource, Wallet};
    /// # let mut wallet = Wallet::builder(Network::Regtest)
    /// #     .keys(KeySource::mnemonic(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.as_str()))
    /// #     .create()?;
    /// # let mut chain = MockChain::new(Network::Regtest);
    /// # let a = wallet.new_address()?.address;
    /// # chain.fund(a.script_pubkey(), Amount::from_sat(100_000));
    /// # wallet.sync(&chain)?;
    /// # let mut other = Wallet::builder(Network::Regtest)
    /// #     .keys(KeySource::mnemonic(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.as_str()))
    /// #     .create()?;
    /// # let to = other.new_address()?.address.into_unchecked();
    /// use wallet::{Recipient, SignOutcome};
    /// use wallet::bitcoin::FeeRate;
    ///
    /// let mut psbt = wallet.build_tx(
    ///     [Recipient::new(to, Amount::from_sat(20_000))],
    ///     FeeRate::from_sat_per_vb_u32(2),
    /// )?;
    /// assert_eq!(wallet.sign(&mut psbt)?, SignOutcome::Finalized);
    /// let tx = psbt.extract_tx()?;
    /// # Ok::<(), wallet::BoxError>(())
    /// ```
    pub fn sign(&self, psbt: &mut Psbt) -> Result<SignOutcome, SignError> {
        self.engine.sign(psbt)
    }

    /// Build, sign and broadcast in one call.
    ///
    /// ```
    /// # use wallet::bitcoin::{Amount, Network};
    /// # use wallet::testkit::MockChain;
    /// # use wallet::{KeySource, Wallet};
    /// # let mut wallet = Wallet::builder(Network::Regtest)
    /// #     .keys(KeySource::mnemonic(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.as_str()))
    /// #     .create()?;
    /// # let mut chain = MockChain::new(Network::Regtest);
    /// # let a = wallet.new_address()?.address;
    /// # chain.fund(a.script_pubkey(), Amount::from_sat(100_000));
    /// # wallet.sync(&chain)?;
    /// # let mut other = Wallet::builder(Network::Regtest)
    /// #     .keys(KeySource::mnemonic(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.as_str()))
    /// #     .create()?;
    /// # let to = other.new_address()?.address.into_unchecked();
    /// use wallet::{Recipient, TxStatus};
    /// use wallet::bitcoin::FeeRate;
    ///
    /// let txid = wallet.send(
    ///     [Recipient::new(to, Amount::from_sat(20_000))],
    ///     FeeRate::from_sat_per_vb_u32(2),
    ///     &chain,
    /// )?;
    /// wallet.sync(&chain)?;
    /// assert_eq!(wallet.tx_status(txid), Some(TxStatus::Unconfirmed));
    /// # Ok::<(), wallet::BoxError>(())
    /// ```
    pub fn send(
        &mut self,
        recipients: impl IntoIterator<Item = Recipient>,
        fee_rate: FeeRate,
        broadcaster: &impl Broadcaster,
    ) -> Result<Txid, Error> {
        let mut psbt = self.build_tx(recipients, fee_rate)?;
        if self.sign(&mut psbt)? != SignOutcome::Finalized {
            return Err(Error::NotFinalized);
        }
        let tx = psbt.extract_tx().map_err(|e| Error::Extract(Box::new(e)))?;
        Ok(broadcaster.broadcast(&tx)?)
    }

    /// Build, sign and broadcast with full control over coin selection and RBF.
    pub fn send_with(
        &mut self,
        request: &TxRequest,
        broadcaster: &impl Broadcaster,
    ) -> Result<Txid, Error> {
        let mut psbt = self.build_tx_with(request)?;
        if self.sign(&mut psbt)? != SignOutcome::Finalized {
            return Err(Error::NotFinalized);
        }
        let tx = psbt.extract_tx().map_err(|e| Error::Extract(Box::new(e)))?;
        Ok(broadcaster.broadcast(&tx)?)
    }

    /// Record a signed transaction that has not been broadcast yet (for
    /// example while offline), so balance and coin selection account for it.
    ///
    /// A later [`sync`](Self::sync) drops it again if the node does not have
    /// it, so broadcast it before syncing.
    ///
    /// ```
    /// # use wallet::bitcoin::{Amount, Network};
    /// # use wallet::testkit::MockChain;
    /// # use wallet::{KeySource, Wallet};
    /// # let mut wallet = Wallet::builder(Network::Regtest)
    /// #     .keys(KeySource::mnemonic(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.as_str()))
    /// #     .create()?;
    /// # let mut chain = MockChain::new(Network::Regtest);
    /// # let a = wallet.new_address()?.address;
    /// # chain.fund(a.script_pubkey(), Amount::from_sat(100_000));
    /// # wallet.sync(&chain)?;
    /// # let mut other = Wallet::builder(Network::Regtest)
    /// #     .keys(KeySource::mnemonic(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.as_str()))
    /// #     .create()?;
    /// # let to = other.new_address()?.address.into_unchecked();
    /// use wallet::Recipient;
    /// use wallet::bitcoin::FeeRate;
    ///
    /// let mut psbt = wallet.build_tx(
    ///     [Recipient::new(to, Amount::from_sat(20_000))],
    ///     FeeRate::from_sat_per_vb_u32(2),
    /// )?;
    /// wallet.sign(&mut psbt)?;
    /// let tx = psbt.extract_tx()?;
    ///
    /// wallet.track_pending(&tx)?; // e.g. saved to send later
    /// assert_eq!(wallet.balance().confirmed, Amount::ZERO, "the coin is spent");
    /// assert!(wallet.balance().unconfirmed > Amount::ZERO, "change is pending");
    /// # Ok::<(), wallet::BoxError>(())
    /// ```
    pub fn track_pending(&mut self, tx: &Transaction) -> Result<(), TrackTxError> {
        self.engine.track_pending(tx)
    }

    /// Public descriptors for both keychains, for creating a watch-only copy.
    /// `None` if the engine cannot export them.
    pub fn descriptors(&self) -> Option<(String, String)> {
        self.engine.descriptors()
    }

    /// Replace an unconfirmed transaction with a higher-fee version (RBF).
    ///
    /// Returns an unsigned PSBT paying the same recipients; the extra fee comes
    /// out of change. Sign and broadcast it like any other.
    ///
    /// ```
    /// # use wallet::bitcoin::{Amount, Network};
    /// # use wallet::testkit::MockChain;
    /// # use wallet::{KeySource, Wallet};
    /// # let mut wallet = Wallet::builder(Network::Regtest)
    /// #     .keys(KeySource::mnemonic(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.as_str()))
    /// #     .create()?;
    /// # let mut chain = MockChain::new(Network::Regtest);
    /// # let a = wallet.new_address()?.address;
    /// # chain.fund(a.script_pubkey(), Amount::from_sat(100_000));
    /// # wallet.sync(&chain)?;
    /// # let mut other = Wallet::builder(Network::Regtest)
    /// #     .keys(KeySource::mnemonic(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.as_str()))
    /// #     .create()?;
    /// # let to = other.new_address()?.address.into_unchecked();
    /// use wallet::Recipient;
    /// use wallet::bitcoin::FeeRate;
    ///
    /// let stuck = wallet.send(
    ///     [Recipient::new(to, Amount::from_sat(20_000))],
    ///     FeeRate::from_sat_per_vb_u32(1),
    ///     &chain,
    /// )?;
    /// wallet.sync(&chain)?;
    ///
    /// let mut faster = wallet.bump_fee(stuck, FeeRate::from_sat_per_vb_u32(10))?;
    /// wallet.sign(&mut faster)?;
    /// assert!(faster.fee()? > Amount::from_sat(1_000));
    /// # Ok::<(), wallet::BoxError>(())
    /// ```
    pub fn bump_fee(&mut self, txid: Txid, new_fee_rate: FeeRate) -> Result<Psbt, BumpFeeError> {
        self.engine.bump_fee(txid, new_fee_rate)
    }
}

/// Configures and opens a BDK-backed [`Wallet`]. See [`BdkEngineBuilder`].
pub struct WalletBuilder {
    inner: BdkEngineBuilder,
}

impl WalletBuilder {
    /// Keys for the wallet: a mnemonic, or descriptors. Required to create;
    /// when loading, leaving them out opens the stored wallet watch-only.
    pub fn keys(self, keys: KeySource) -> Self {
        Self {
            inner: self.inner.keys(keys),
        }
    }

    /// Persist to this SQLite file. Without it the wallet lives in memory.
    pub fn database(self, path: impl Into<PathBuf>) -> Self {
        Self {
            inner: self.inner.database(path),
        }
    }

    /// Keep everything in memory (the default); nothing survives a restart.
    pub fn in_memory(self) -> Self {
        Self {
            inner: self.inner.in_memory(),
        }
    }

    /// Default coin selection strategy (branch and bound unless set).
    pub fn coin_selection(self, strategy: CoinSelection) -> Self {
        Self {
            inner: self.inner.coin_selection(strategy),
        }
    }

    /// How many unused addresses past the last used one to watch.
    pub fn lookahead(self, lookahead: u32) -> Self {
        Self {
            inner: self.inner.lookahead(lookahead),
        }
    }

    /// Skip blocks below this height on the first sync. Use the height the
    /// wallet was created at so a restore does not rescan the whole chain.
    pub fn birthday(self, height: u32) -> Self {
        Self {
            inner: self.inner.birthday(height),
        }
    }

    /// Create a new wallet. Fails with [`CreateError::AlreadyExists`] if the
    /// database already has one.
    pub fn create(self) -> Result<Wallet, CreateError> {
        self.inner.create().map(Wallet::from_engine)
    }

    /// Open an existing wallet. Fails with [`LoadError::NotFound`] if there
    /// is none.
    pub fn load(self) -> Result<Wallet, LoadError> {
        self.inner.load().map(Wallet::from_engine)
    }

    /// Open the wallet if the database has one, otherwise create it.
    pub fn create_or_load(self) -> Result<Wallet, Error> {
        self.inner.create_or_load().map(Wallet::from_engine)
    }
}
