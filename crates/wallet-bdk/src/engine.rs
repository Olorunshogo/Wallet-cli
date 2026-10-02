use std::cmp::Reverse;

use bdk_wallet::bitcoin::psbt::SigningKeys;
use std::time::{SystemTime, UNIX_EPOCH};

use bdk_wallet::bitcoin::{FeeRate, Network, Psbt, ScriptBuf, Sequence, Transaction, Txid};
use bdk_wallet::coin_selection::{
    CoinSelectionAlgorithm, LargestFirstCoinSelection, OldestFirstCoinSelection,
};
use bdk_wallet::error::CreateTxError;
use bdk_wallet::miniscript::descriptor::{KeyMap, KeyMapWrapper};
use bdk_wallet::rusqlite::Connection;
use bdk_wallet::{KeychainKind, PersistedWallet, SignOptions, TxBuilder};
use wallet_core::bitcoin::Amount;
use wallet_core::{
    AddressError, AddressInfo, Balance, BlockId, BlockSource, BuildTxError, BumpFeeError,
    CoinSelection, SignError, SignOutcome, SyncError, SyncReport, TrackTxError, TxDetails,
    TxRequest, TxStatus, Utxo, WalletEngine,
};

use crate::builder::BdkEngineBuilder;
use crate::{convert, sync};

/// [`WalletEngine`] implementation backed by `bdk_wallet` with SQLite storage.
pub struct BdkEngine {
    pub(crate) wallet: PersistedWallet<Connection>,
    pub(crate) db: Connection,
    coin_selection: CoinSelection,
    pub(crate) birthday: Option<u32>,
    /// Private keys, held outside BDK. `None` for watch-only wallets.
    signer: Option<KeyMapWrapper>,
}

impl BdkEngine {
    /// Start configuring an engine for `network`.
    pub fn builder(network: Network) -> BdkEngineBuilder {
        BdkEngineBuilder::new(network)
    }

    pub(crate) fn new(
        wallet: PersistedWallet<Connection>,
        db: Connection,
        keymap: KeyMap,
        coin_selection: CoinSelection,
        birthday: Option<u32>,
    ) -> Self {
        let signer = (!keymap.is_empty()).then(|| KeyMapWrapper::from(keymap));
        Self {
            wallet,
            db,
            coin_selection,
            birthday,
            signer,
        }
    }

    /// Public (xpub) descriptors for both keychains. Safe to share or back up
    /// for a watch-only copy of this wallet.
    pub fn public_descriptors(&self) -> (String, String) {
        (
            self.wallet
                .public_descriptor(KeychainKind::External)
                .to_string(),
            self.wallet
                .public_descriptor(KeychainKind::Internal)
                .to_string(),
        )
    }

    pub(crate) fn persist(&mut self) -> Result<(), bdk_wallet::rusqlite::Error> {
        self.wallet.persist(&mut self.db).map(|_| ())
    }

    fn tip_height(&self) -> u32 {
        self.wallet.latest_checkpoint().height()
    }

    fn reveal(&mut self, next_unused: bool) -> Result<AddressInfo, AddressError> {
        let info = if next_unused {
            self.wallet.next_unused_address(KeychainKind::External)
        } else {
            self.wallet.reveal_next_address(KeychainKind::External)
        };
        self.persist()
            .map_err(|e| AddressError::Persist(Box::new(e)))?;
        Ok(AddressInfo {
            address: info.address,
            index: info.index,
            keychain: convert::keychain(info.keychain),
        })
    }
}

impl WalletEngine for BdkEngine {
    fn network(&self) -> Network {
        self.wallet.network()
    }

    fn can_sign(&self) -> bool {
        self.signer.is_some()
    }

    fn tip(&self) -> BlockId {
        convert::block_id(self.wallet.latest_checkpoint().block_id())
    }

    fn new_address(&mut self) -> Result<AddressInfo, AddressError> {
        let span = tracing::info_span!("wallet.new_address", network = ?self.network());
        let _guard = span.enter();
        return self.reveal(true);
    }

    fn reveal_next_address(&mut self) -> Result<AddressInfo, AddressError> {
        let span = tracing::info_span!("wallet.reveal_next_address", network = ?self.network());
        let _guard = span.enter();
        return self.reveal(false);
    }

    fn balance(&self) -> Balance {
        let b = self.wallet.balance();
        Balance {
            confirmed: b.confirmed,
            unconfirmed: b.trusted_pending + b.untrusted_pending,
            immature: b.immature,
        }
    }

    fn list_utxos(&self) -> Vec<Utxo> {
        let tip = self.tip_height();
        self.wallet
            .list_unspent()
            .map(|o| Utxo {
                outpoint: o.outpoint,
                value: o.txout.value,
                script_pubkey: o.txout.script_pubkey,
                keychain: convert::keychain(o.keychain),
                derivation_index: o.derivation_index,
                status: convert::status(&o.chain_position, tip),
            })
            .collect()
    }

    fn transactions(&self) -> Vec<TxDetails> {
        let tip = self.tip_height();
        let mut txs: Vec<TxDetails> = self
            .wallet
            .transactions()
            .map(|wtx| {
                let tx = &wtx.tx_node.tx;
                let (sent, received) = self.wallet.sent_and_received(tx);
                TxDetails {
                    txid: wtx.tx_node.txid,
                    sent,
                    received,
                    fee: self.wallet.calculate_fee(tx).ok(),
                    status: convert::status(&wtx.chain_position, tip),
                }
            })
            .collect();
        // Unconfirmed first, then newest block first.
        txs.sort_by_key(|t| match t.status {
            TxStatus::Unconfirmed => (0, Reverse(u32::MAX)),
            TxStatus::Confirmed { height, .. } => (1, Reverse(height)),
        });
        txs
    }

    fn tx_status(&self, txid: Txid) -> Option<TxStatus> {
        let tip = self.tip_height();
        self.wallet
            .get_tx(txid)
            .map(|wtx| convert::status(&wtx.chain_position, tip))
    }

    fn build_tx(&mut self, request: &TxRequest) -> Result<Psbt, BuildTxError> {
        let span = tracing::info_span!(
            "wallet.build_tx",
            network = ?self.network(),
            recipients = request.recipients.len(),
            fee_rate = ?request.fee_rate,
        );
        let _guard = span.enter();

        if request.recipients.is_empty() {
            return Err(BuildTxError::NoRecipients);
        }
        let network = self.network();
        let mut outputs: Vec<(ScriptBuf, Amount)> = Vec::with_capacity(request.recipients.len());
        for r in &request.recipients {
            let address = r.address.clone().require_network(network).map_err(|_| {
                BuildTxError::WrongNetwork {
                    address: r.address.assume_checked_ref().to_string(),
                    expected: network,
                }
            })?;
            outputs.push((address.script_pubkey(), r.amount));
        }

        let strategy = request.coin_selection.unwrap_or(self.coin_selection);
        let result = match strategy {
            CoinSelection::BranchAndBound => finish(self.wallet.build_tx(), outputs, request),
            CoinSelection::LargestFirst => finish(
                self.wallet
                    .build_tx()
                    .coin_selection(LargestFirstCoinSelection),
                outputs,
                request,
            ),
            CoinSelection::OldestFirst => finish(
                self.wallet
                    .build_tx()
                    .coin_selection(OldestFirstCoinSelection),
                outputs,
                request,
            ),
        };
        let psbt = result.map_err(convert::build_tx_error)?;
        // Building may reveal a change address; persist so it is not reused.
        self.persist()
            .map_err(|e| BuildTxError::Persist(Box::new(e)))?;
        return Ok(psbt);
    }

    fn sign(&self, psbt: &mut Psbt) -> Result<SignOutcome, SignError> {
        let span = tracing::info_span!("wallet.sign", network = ?self.network());
        let _guard = span.enter();

        let signer = self.signer.as_ref().ok_or(SignError::WatchOnly)?;
        let signed = match psbt.sign(signer, self.wallet.secp_ctx()) {
            Ok(signed) => signed,
            // Some inputs could not be signed. Keep going if we signed any
            // (e.g. a PSBT with foreign inputs), otherwise report why.
            Err((signed, errors)) if signed.is_empty() => {
                return match errors.into_values().next() {
                    Some(e) => Err(SignError::InvalidPsbt(Box::new(e))),
                    None => Err(SignError::NothingToSign),
                };
            }
            Err((signed, _)) => signed,
        };
        let signed_any = signed.values().any(|keys| match keys {
            SigningKeys::Ecdsa(k) => !k.is_empty(),
            SigningKeys::Schnorr(k) => !k.is_empty(),
        });
        if !signed_any {
            return Err(SignError::NothingToSign);
        }
        let finalized = self
            .wallet
            .finalize_psbt(psbt, SignOptions::default())
            .map_err(convert::sign_error)?;
        let outcome = if finalized {
            SignOutcome::Finalized
        } else {
            SignOutcome::Partial
        };
        tracing::debug!(?outcome, "signing complete");
        return Ok(outcome);
    }

    fn sync(&mut self, source: &dyn BlockSource) -> Result<SyncReport, SyncError> {
        sync::run(self, source)
    }

    fn descriptors(&self) -> Option<(String, String)> {
        Some(self.public_descriptors())
    }

    fn track_pending(&mut self, tx: &Transaction) -> Result<(), TrackTxError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        // BDK keeps only transactions that touch the wallet.
        self.wallet.apply_unconfirmed_txs([(tx.clone(), now)]);
        if self.wallet.get_tx(tx.compute_txid()).is_none() {
            return Err(TrackTxError::NotRelevant);
        }
        self.persist()
            .map_err(|e| TrackTxError::Persist(Box::new(e)))?;
        return Ok(());
    }

    fn bump_fee(&mut self, txid: Txid, new_fee_rate: FeeRate) -> Result<Psbt, BumpFeeError> {
        let span =
            tracing::info_span!("wallet.bump_fee", txid = %txid, new_fee_rate = ?new_fee_rate);
        let _guard = span.enter();
        let mut builder = self
            .wallet
            .build_fee_bump(txid)
            .map_err(convert::fee_bump_error)?;
        builder.fee_rate(new_fee_rate);
        let psbt = builder.finish().map_err(convert::bump_finish_error)?;
        // The replacement may reveal a new change index; persist like build_tx.
        self.persist()
            .map_err(|e| BumpFeeError::Persist(Box::new(e)))?;
        return Ok(psbt);
    }
}

fn finish<Cs: CoinSelectionAlgorithm>(
    mut builder: TxBuilder<'_, Cs>,
    outputs: Vec<(ScriptBuf, Amount)>,
    request: &TxRequest,
) -> Result<Psbt, CreateTxError> {
    builder.set_recipients(outputs).fee_rate(request.fee_rate);
    if !request.enable_rbf {
        builder.set_exact_sequence(Sequence::ENABLE_LOCKTIME_NO_RBF);
    }
    builder.finish()
}
