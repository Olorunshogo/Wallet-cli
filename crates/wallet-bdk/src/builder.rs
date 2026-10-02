use std::path::PathBuf;

use bdk_wallet::bitcoin::Network;
use bdk_wallet::miniscript::descriptor::KeyMap;
use bdk_wallet::rusqlite::Connection;
use bdk_wallet::{CreateWithPersistError, KeychainKind, Wallet};
use wallet_core::{CoinSelection, CreateError, Error, KeySource, LoadError};

use crate::convert;
use crate::engine::BdkEngine;
use crate::keys;

enum Storage {
    Memory,
    File(PathBuf),
}

/// Configures and opens a [`BdkEngine`].
///
/// ```no_run
/// # use wallet_bdk::{BdkEngine, generate_mnemonic};
/// # use wallet_core::{KeySource, MnemonicLength, bitcoin::Network};
/// let mnemonic = generate_mnemonic(MnemonicLength::Words12)?;
/// let engine = BdkEngine::builder(Network::Regtest)
///     .keys(KeySource::mnemonic(mnemonic.as_str()))
///     .database("wallet.sqlite")
///     .create_or_load()?;
/// # Ok::<(), wallet_core::BoxError>(())
/// ```
pub struct BdkEngineBuilder {
    network: Network,
    keys: Option<KeySource>,
    storage: Storage,
    coin_selection: CoinSelection,
    lookahead: Option<u32>,
    birthday: Option<u32>,
}

impl BdkEngineBuilder {
    pub(crate) fn new(network: Network) -> Self {
        Self {
            network,
            keys: None,
            storage: Storage::Memory,
            coin_selection: CoinSelection::default(),
            lookahead: None,
            birthday: None,
        }
    }

    /// Keys for the wallet. Required to create; optional when loading, where
    /// omitting them opens the stored wallet watch-only.
    pub fn keys(mut self, keys: KeySource) -> Self {
        self.keys = Some(keys);
        self
    }

    /// Persist to an SQLite file. Without this the wallet lives in memory.
    pub fn database(mut self, path: impl Into<PathBuf>) -> Self {
        self.storage = Storage::File(path.into());
        self
    }

    /// Keep everything in memory (the default).
    pub fn in_memory(mut self) -> Self {
        self.storage = Storage::Memory;
        self
    }

    /// Default coin selection strategy. Can be overridden per transaction.
    pub fn coin_selection(mut self, strategy: CoinSelection) -> Self {
        self.coin_selection = strategy;
        self
    }

    /// How many addresses past the last used one to watch (the gap limit).
    pub fn lookahead(mut self, lookahead: u32) -> Self {
        self.lookahead = Some(lookahead);
        self
    }

    /// Skip blocks below this height on the first sync. Use the height the
    /// wallet was created at to avoid rescanning the whole chain on restore.
    pub fn birthday(mut self, height: u32) -> Self {
        self.birthday = Some(height);
        self
    }

    /// Create a new wallet. Fails if the database already holds one.
    pub fn create(self) -> Result<BdkEngine, CreateError> {
        let db = self.open().map_err(|e| CreateError::Persist(Box::new(e)))?;
        self.create_with(db)
    }

    /// Load an existing wallet.
    pub fn load(self) -> Result<BdkEngine, LoadError> {
        let db = self.open().map_err(|e| LoadError::Persist(Box::new(e)))?;
        self.load_with(db)
    }

    /// Load the wallet if the database has one, otherwise create it.
    pub fn create_or_load(self) -> Result<BdkEngine, Error> {
        let mut db = self.open().map_err(|e| LoadError::Persist(Box::new(e)))?;
        let exists = Wallet::load()
            .load_wallet(&mut db)
            .map_err(convert::load_error)?
            .is_some();
        if exists {
            Ok(self.load_with(db)?)
        } else {
            Ok(self.create_with(db)?)
        }
    }

    fn open(&self) -> Result<Connection, bdk_wallet::rusqlite::Error> {
        match &self.storage {
            Storage::Memory => Connection::open_in_memory(),
            Storage::File(path) => Connection::open(path),
        }
    }

    fn create_with(self, mut db: Connection) -> Result<BdkEngine, CreateError> {
        let keys = self.keys.as_ref().ok_or_else(|| {
            CreateError::InvalidDescriptor("no keys provided: call .keys(..) before create".into())
        })?;
        let resolved = keys::resolve(keys, self.network).map_err(|e| match keys {
            KeySource::Mnemonic { .. } => CreateError::InvalidMnemonic(e),
            KeySource::Descriptors { .. } => CreateError::InvalidDescriptor(e),
        })?;
        let mut params = Wallet::create(resolved.external, resolved.internal).network(self.network);
        if let Some(n) = self.lookahead {
            params = params.lookahead(n);
        }
        let wallet = params.create_wallet(&mut db).map_err(|e| match e {
            CreateWithPersistError::Persist(e) => CreateError::Persist(Box::new(e)),
            CreateWithPersistError::DataAlreadyExists(_) => CreateError::AlreadyExists,
            CreateWithPersistError::Descriptor(e) => CreateError::InvalidDescriptor(Box::new(e)),
        })?;
        Ok(BdkEngine::new(
            wallet,
            db,
            resolved.keymap,
            self.coin_selection,
            self.birthday,
        ))
    }

    fn load_with(self, mut db: Connection) -> Result<BdkEngine, LoadError> {
        let mut params = Wallet::load().check_network(self.network);
        let mut keymap = KeyMap::new();
        if let Some(keys) = &self.keys {
            let resolved = keys::resolve(keys, self.network).map_err(|e| match keys {
                KeySource::Mnemonic { .. } => LoadError::InvalidMnemonic(e),
                KeySource::Descriptors { .. } => LoadError::InvalidDescriptor(e),
            })?;
            // BDK checks the stored descriptors match the ones derived here.
            params = params
                .descriptor(KeychainKind::External, Some(resolved.external))
                .descriptor(KeychainKind::Internal, Some(resolved.internal));
            keymap = resolved.keymap;
        }
        if let Some(n) = self.lookahead {
            params = params.lookahead(n);
        }
        let wallet = params
            .load_wallet(&mut db)
            .map_err(convert::load_error)?
            .ok_or(LoadError::NotFound)?;
        Ok(BdkEngine::new(
            wallet,
            db,
            keymap,
            self.coin_selection,
            self.birthday,
        ))
    }
}
