//! Public API tests against the in-memory `MockChain`. No node required.

use wallet::bitcoin::address::NetworkUnchecked;
use wallet::bitcoin::hashes::Hash;
use wallet::bitcoin::{Address, Amount, FeeRate, Network};
use wallet::testkit::MockChain;
use wallet::{
    BuildTxError, BumpFeeError, CoinSelection, CreateError, Error, KeySource, Keychain, LoadError,
    MnemonicLength, Recipient, SignError, SignOutcome, SyncError, TxStatus, Wallet,
};

/// The official BIP84 test vector. The only fixed mnemonic in the suite: it
/// proves derivation matches the spec's published addresses. Every other test
/// generates fresh keys.
const BIP84_VECTOR_MNEMONIC: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

/// A brand-new random 12-word mnemonic.
fn fresh_phrase() -> String {
    wallet::generate_mnemonic(MnemonicLength::Words12)
        .unwrap()
        .to_string()
}

/// An in-memory regtest wallet for `phrase`.
fn wallet_from(phrase: &str) -> Wallet {
    Wallet::builder(Network::Regtest)
        .keys(KeySource::mnemonic(phrase))
        .create()
        .expect("create wallet")
}

/// An in-memory regtest wallet with freshly generated keys.
fn regtest_wallet() -> Wallet {
    wallet_from(&fresh_phrase())
}

/// A payment to an address owned by some other wallet.
fn outside(amount: u64) -> Recipient {
    let mut stranger = regtest_wallet();
    let address = stranger.new_address().unwrap().address;
    Recipient::new(address.as_unchecked().clone(), Amount::from_sat(amount))
}

fn rate(sat_vb: u32) -> FeeRate {
    FeeRate::from_sat_per_vb_u32(sat_vb)
}

// === Keys and addresses

#[test]
fn derives_bip84_test_vectors() {
    let mut wallet = Wallet::builder(Network::Bitcoin)
        .keys(KeySource::mnemonic(BIP84_VECTOR_MNEMONIC))
        .create()
        .unwrap();

    let first = wallet.new_address().unwrap();
    assert_eq!(
        first.address.to_string(),
        "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu"
    );
    assert_eq!((first.index, first.keychain), (0, Keychain::External));

    let second = wallet.reveal_next_address().unwrap();
    assert_eq!(
        second.address.to_string(),
        "bc1qnjg0jd8228aq7egyzacy8cys3knf9xvrerkf9g"
    );
    assert_eq!(second.index, 1);
}

#[test]
fn new_address_is_stable_until_used() {
    let mut wallet = regtest_wallet();
    let mut chain = MockChain::new(Network::Regtest);

    let a = wallet.new_address().unwrap();
    assert_eq!(wallet.new_address().unwrap(), a, "unused address is reused");

    chain.fund(a.address.script_pubkey(), Amount::from_sat(10_000));
    wallet.sync(&chain).unwrap();

    let b = wallet.new_address().unwrap();
    assert_ne!(b.address, a.address);
    assert_eq!(b.index, a.index + 1);
}

// === Sync and balance

#[test]
fn sync_reports_confirmed_and_unconfirmed_funds() {
    let mut wallet = regtest_wallet();
    let mut chain = MockChain::new(Network::Regtest);
    let addr = wallet.new_address().unwrap().address;

    let confirmed = chain.fund(addr.script_pubkey(), Amount::from_sat(50_000));
    chain.mine_empty(2);
    let pending = chain.fund_unconfirmed(addr.script_pubkey(), Amount::from_sat(20_000));

    let report = wallet.sync(&chain).unwrap();
    assert_eq!(report.blocks_applied, 3);
    assert_eq!(report.to.height, 3);
    assert_eq!(report.reorg_depth, 0);

    let balance = wallet.balance();
    assert_eq!(balance.confirmed, Amount::from_sat(50_000));
    assert_eq!(balance.unconfirmed, Amount::from_sat(20_000));
    assert_eq!(balance.total(), Amount::from_sat(70_000));
    assert_eq!(wallet.list_utxos().len(), 2);

    assert_eq!(
        wallet.tx_status(confirmed),
        Some(TxStatus::Confirmed {
            height: 1,
            confirmations: 3
        })
    );
    assert_eq!(wallet.tx_status(pending), Some(TxStatus::Unconfirmed));

    let history = wallet.transactions();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].txid, pending, "unconfirmed listed first");
    assert_eq!(history[1].received, Amount::from_sat(50_000));
}

#[test]
fn sync_is_incremental() {
    let mut wallet = regtest_wallet();
    let mut chain = MockChain::new(Network::Regtest);
    chain.mine_empty(5);
    assert_eq!(wallet.sync(&chain).unwrap().blocks_applied, 5);
    assert_eq!(wallet.sync(&chain).unwrap().blocks_applied, 0);
    chain.mine_empty(2);
    assert_eq!(wallet.sync(&chain).unwrap().blocks_applied, 2);
}

#[test]
fn reorg_unconfirms_funds_from_orphaned_blocks() {
    let mut wallet = regtest_wallet();
    let mut chain = MockChain::new(Network::Regtest);
    let addr = wallet.new_address().unwrap().address;

    chain.mine_empty(3);
    let txid = chain.fund(addr.script_pubkey(), Amount::from_sat(40_000));
    wallet.sync(&chain).unwrap();
    assert_eq!(wallet.balance().confirmed, Amount::from_sat(40_000));

    // Replace the funding block with a longer empty branch.
    chain.reorg(1);
    chain.mine_empty(2);
    let report = wallet.sync(&chain).unwrap();

    assert!(report.reorg_depth >= 1);
    assert_eq!(report.to, chain_tip(&chain));
    assert_eq!(wallet.balance().confirmed, Amount::ZERO);
    // Like a real node returning reorged txs to the mempool, BDK keeps the
    // orphaned tx as pending until something conflicts with it.
    assert_eq!(wallet.tx_status(txid), Some(TxStatus::Unconfirmed));
}

#[test]
fn sync_rejects_a_different_chain() {
    let mut wallet = regtest_wallet();
    let chain = MockChain::new(Network::Testnet);
    assert!(matches!(wallet.sync(&chain), Err(SyncError::WrongChain)));
}

fn chain_tip(chain: &MockChain) -> wallet::BlockId {
    use wallet::BlockSource;
    chain.tip().unwrap()
}

// === Spending

#[test]
fn build_sign_broadcast_and_confirm() {
    let mut wallet = regtest_wallet();
    let mut chain = MockChain::new(Network::Regtest);
    let addr = wallet.new_address().unwrap().address;
    chain.fund(addr.script_pubkey(), Amount::from_sat(100_000));
    wallet.sync(&chain).unwrap();

    let mut psbt = wallet.build_tx([outside(30_000)], rate(2)).unwrap();
    assert_eq!(wallet.sign(&mut psbt).unwrap(), SignOutcome::Finalized);

    let fee = psbt.fee().unwrap();
    let tx = psbt.extract_tx().unwrap();
    assert!(
        tx.input.iter().all(|i| !i.witness.is_empty()),
        "inputs are signed"
    );
    // Fee rates are applied to exact weight, so compare in weight units.
    assert!(
        fee >= rate(2).fee_wu(tx.weight()).unwrap(),
        "fee meets the requested rate"
    );

    let txid = wallet::Broadcaster::broadcast(&chain, &tx).unwrap();
    wallet.sync(&chain).unwrap();
    assert_eq!(wallet.tx_status(txid), Some(TxStatus::Unconfirmed));
    let expected = Amount::from_sat(100_000 - 30_000) - fee;
    assert_eq!(wallet.balance().total(), expected);

    chain.mine(Vec::new());
    wallet.sync(&chain).unwrap();
    assert!(wallet.tx_status(txid).unwrap().is_confirmed());
    assert_eq!(wallet.balance().confirmed, expected);

    let sent = wallet
        .transactions()
        .into_iter()
        .find(|t| t.txid == txid)
        .unwrap();
    assert_eq!(sent.fee, Some(fee));
    assert_eq!(sent.sent, Amount::from_sat(100_000));
}

#[test]
fn send_convenience_builds_signs_and_broadcasts() {
    let mut wallet = regtest_wallet();
    let mut chain = MockChain::new(Network::Regtest);
    let addr = wallet.new_address().unwrap().address;
    chain.fund(addr.script_pubkey(), Amount::from_sat(80_000));
    wallet.sync(&chain).unwrap();

    let txid = wallet.send([outside(10_000)], rate(1), &chain).unwrap();
    wallet.sync(&chain).unwrap();
    assert_eq!(wallet.tx_status(txid), Some(TxStatus::Unconfirmed));
}

#[test]
fn every_coin_selection_strategy_builds() {
    for strategy in [
        CoinSelection::BranchAndBound,
        CoinSelection::LargestFirst,
        CoinSelection::OldestFirst,
    ] {
        let mut wallet = Wallet::builder(Network::Regtest)
            .keys(KeySource::mnemonic(fresh_phrase()))
            .coin_selection(strategy)
            .create()
            .unwrap();
        let mut chain = MockChain::new(Network::Regtest);
        for amount in [5_000, 20_000, 60_000] {
            let a = wallet.reveal_next_address().unwrap().address;
            chain.fund(a.script_pubkey(), Amount::from_sat(amount));
        }
        wallet.sync(&chain).unwrap();
        let psbt = wallet.build_tx([outside(50_000)], rate(1)).unwrap();
        assert!(!psbt.inputs.is_empty(), "{strategy:?} selected inputs");
    }
}

#[test]
fn insufficient_funds_reports_amounts() {
    let mut wallet = regtest_wallet();
    let mut chain = MockChain::new(Network::Regtest);
    let addr = wallet.new_address().unwrap().address;
    chain.fund(addr.script_pubkey(), Amount::from_sat(10_000));
    wallet.sync(&chain).unwrap();

    match wallet.build_tx([outside(50_000)], rate(1)) {
        Err(BuildTxError::InsufficientFunds { needed, available }) => {
            assert!(needed > Amount::from_sat(50_000));
            assert_eq!(available, Amount::from_sat(10_000));
        }
        other => panic!("expected InsufficientFunds, got {other:?}"),
    }
}

#[test]
fn wrong_network_address_is_rejected() {
    let mut wallet = regtest_wallet();
    let mainnet: Address<NetworkUnchecked> = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu"
        .parse()
        .unwrap();
    let err = wallet
        .build_tx([Recipient::new(mainnet, Amount::from_sat(1_000))], rate(1))
        .unwrap_err();
    assert!(matches!(
        err,
        BuildTxError::WrongNetwork {
            expected: Network::Regtest,
            ..
        }
    ));
}

#[test]
fn empty_recipient_list_is_rejected() {
    let mut wallet = regtest_wallet();
    let err = wallet.build_tx(Vec::new(), rate(1)).unwrap_err();
    assert!(matches!(err, BuildTxError::NoRecipients));
}

#[test]
fn watch_only_wallet_builds_but_cannot_sign() {
    let phrase = fresh_phrase();
    let (external, internal) = wallet_from(&phrase).public_descriptors();
    let mut watch_only = Wallet::builder(Network::Regtest)
        .keys(KeySource::descriptors(external, internal))
        .create()
        .unwrap();
    assert!(!watch_only.can_sign());

    let mut chain = MockChain::new(Network::Regtest);
    let addr = watch_only.new_address().unwrap().address;
    chain.fund(addr.script_pubkey(), Amount::from_sat(50_000));
    watch_only.sync(&chain).unwrap();

    let mut psbt = watch_only.build_tx([outside(10_000)], rate(1)).unwrap();
    assert!(matches!(
        watch_only.sign(&mut psbt),
        Err(SignError::WatchOnly)
    ));

    // The keyed wallet can sign the watch-only wallet's PSBT.
    let signer = wallet_from(&phrase);
    assert_eq!(signer.sign(&mut psbt).unwrap(), SignOutcome::Finalized);
}

#[test]
fn signing_a_foreign_psbt_reports_nothing_to_sign() {
    let mut wallet = regtest_wallet();
    let mut chain = MockChain::new(Network::Regtest);
    let addr = wallet.new_address().unwrap().address;
    chain.fund(addr.script_pubkey(), Amount::from_sat(50_000));
    wallet.sync(&chain).unwrap();
    let mut psbt = wallet.build_tx([outside(10_000)], rate(1)).unwrap();

    let stranger = regtest_wallet();
    assert!(matches!(
        stranger.sign(&mut psbt),
        Err(SignError::NothingToSign)
    ));
}

// === Persistence

#[test]
fn state_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("wallet.sqlite");
    let mut chain = MockChain::new(Network::Regtest);
    let phrase = fresh_phrase();

    let used = {
        let mut wallet = Wallet::builder(Network::Regtest)
            .keys(KeySource::mnemonic(phrase.as_str()))
            .database(&db)
            .create()
            .unwrap();
        let addr = wallet.new_address().unwrap();
        chain.fund(addr.address.script_pubkey(), Amount::from_sat(25_000));
        wallet.sync(&chain).unwrap();
        addr
    };

    let mut reopened = Wallet::builder(Network::Regtest)
        .keys(KeySource::mnemonic(phrase.as_str()))
        .database(&db)
        .load()
        .unwrap();
    assert!(reopened.can_sign());
    assert_eq!(reopened.balance().confirmed, Amount::from_sat(25_000));
    assert_eq!(reopened.tip().height, 1);
    assert_eq!(reopened.new_address().unwrap().index, used.index + 1);

    // Loading without keys opens the same wallet watch-only.
    drop(reopened);
    let watch_only = Wallet::builder(Network::Regtest)
        .database(&db)
        .load()
        .unwrap();
    assert!(!watch_only.can_sign());
    assert_eq!(watch_only.balance().confirmed, Amount::from_sat(25_000));
}

#[test]
fn lifecycle_errors_are_specific() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("wallet.sqlite");

    let missing = Wallet::builder(Network::Regtest).database(&db).load();
    assert!(matches!(missing, Err(LoadError::NotFound)));

    let phrase = fresh_phrase();
    Wallet::builder(Network::Regtest)
        .keys(KeySource::mnemonic(phrase.as_str()))
        .database(&db)
        .create()
        .unwrap();

    let again = Wallet::builder(Network::Regtest)
        .keys(KeySource::mnemonic(phrase.as_str()))
        .database(&db)
        .create();
    assert!(matches!(again, Err(CreateError::AlreadyExists)));

    let wrong_net = Wallet::builder(Network::Signet).database(&db).load();
    assert!(matches!(
        wrong_net,
        Err(LoadError::NetworkMismatch {
            expected: Network::Signet,
            found: Network::Regtest
        })
    ));

    let other_keys = Wallet::builder(Network::Regtest)
        .keys(KeySource::mnemonic_with_passphrase(
            phrase.as_str(),
            "other",
        ))
        .database(&db)
        .load();
    assert!(matches!(other_keys, Err(LoadError::DescriptorMismatch)));

    let bad_words = Wallet::builder(Network::Regtest)
        .keys(KeySource::mnemonic("these are not bip39 words at all"))
        .create();
    assert!(matches!(bad_words, Err(CreateError::InvalidMnemonic(_))));
}

#[test]
fn create_or_load_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("wallet.sqlite");
    let phrase = fresh_phrase();
    let open = || -> Result<Wallet, Error> {
        Wallet::builder(Network::Regtest)
            .keys(KeySource::mnemonic(phrase.as_str()))
            .database(&db)
            .create_or_load()
    };
    let first = open().unwrap().new_address().unwrap();
    let second = open().unwrap().new_address().unwrap();
    assert_eq!(first, second);
}

// === Fee bumping (RBF)

/// A wallet with one 100k-sat coin and an unconfirmed 30k payment at
/// `rate_vb` sat/vB, already broadcast and synced.
fn stuck_payment(
    rate_vb: u32,
    enable_rbf: bool,
) -> (Wallet, MockChain, wallet::bitcoin::Txid, Amount) {
    let mut wallet = regtest_wallet();
    let mut chain = MockChain::new(Network::Regtest);
    let addr = wallet.new_address().unwrap().address;
    chain.fund(addr.script_pubkey(), Amount::from_sat(100_000));
    wallet.sync(&chain).unwrap();

    let mut request = wallet::TxRequest::new(vec![outside(30_000)], rate(rate_vb));
    request.enable_rbf = enable_rbf;
    let mut psbt = wallet.build_tx_with(&request).unwrap();
    wallet.sign(&mut psbt).unwrap();
    let fee = psbt.fee().unwrap();
    let txid = wallet::Broadcaster::broadcast(&chain, &psbt.extract_tx().unwrap()).unwrap();
    wallet.sync(&chain).unwrap();
    (wallet, chain, txid, fee)
}

#[test]
fn bump_fee_replaces_a_stuck_transaction() {
    let (mut wallet, mut chain, original, original_fee) = stuck_payment(1, true);

    let mut psbt = wallet.bump_fee(original, rate(5)).unwrap();
    assert_eq!(wallet.sign(&mut psbt).unwrap(), SignOutcome::Finalized);
    let new_fee = psbt.fee().unwrap();
    assert!(new_fee > original_fee, "{new_fee} > {original_fee}");
    let replacement = psbt.extract_tx().unwrap();
    let replacement_id = wallet::Broadcaster::broadcast(&chain, &replacement).unwrap();
    assert_eq!(
        chain.mempool_txids(),
        vec![replacement_id],
        "original evicted"
    );

    wallet.sync(&chain).unwrap();
    let txids: Vec<_> = wallet.transactions().iter().map(|t| t.txid).collect();
    assert!(txids.contains(&replacement_id));
    assert!(!txids.contains(&original), "replaced tx no longer listed");
    // The extra fee came out of change; the recipient still gets 30k.
    assert_eq!(
        wallet.balance().total(),
        Amount::from_sat(100_000 - 30_000) - new_fee
    );

    chain.mine(Vec::new());
    wallet.sync(&chain).unwrap();
    assert!(wallet.tx_status(replacement_id).unwrap().is_confirmed());
}

#[test]
fn bump_fee_errors_are_specific() {
    let (mut wallet, mut chain, txid, _) = stuck_payment(2, true);

    // Not higher than the original rate.
    assert!(matches!(
        wallet.bump_fee(txid, rate(1)),
        Err(BumpFeeError::FeeRateTooLow { .. })
    ));

    let unknown = wallet::bitcoin::Txid::from_byte_array([7; 32]);
    assert!(matches!(
        wallet.bump_fee(unknown, rate(10)),
        Err(BumpFeeError::NotFound(t)) if t == unknown
    ));

    chain.mine(Vec::new());
    wallet.sync(&chain).unwrap();
    assert!(matches!(
        wallet.bump_fee(txid, rate(10)),
        Err(BumpFeeError::AlreadyConfirmed(t)) if t == txid
    ));

    let (mut no_rbf, _chain, final_tx, _) = stuck_payment(2, false);
    assert!(matches!(
        no_rbf.bump_fee(final_tx, rate(10)),
        Err(BumpFeeError::NotReplaceable(t)) if t == final_tx
    ));
}

// === Pending (unbroadcast) transactions

/// A wallet with one 100k coin and a signed, unbroadcast 30k payment.
fn signed_unbroadcast() -> (Wallet, MockChain, wallet::bitcoin::Transaction) {
    let mut wallet = regtest_wallet();
    let mut chain = MockChain::new(Network::Regtest);
    let addr = wallet.new_address().unwrap().address;
    chain.fund(addr.script_pubkey(), Amount::from_sat(100_000));
    wallet.sync(&chain).unwrap();
    let mut psbt = wallet.build_tx([outside(30_000)], rate(2)).unwrap();
    wallet.sign(&mut psbt).unwrap();
    (wallet, chain, psbt.extract_tx().unwrap())
}

#[test]
fn tracked_payment_reserves_its_coins() {
    let (mut wallet, _chain, tx) = signed_unbroadcast();
    wallet.track_pending(&tx).unwrap();

    let balance = wallet.balance();
    assert_eq!(balance.confirmed, Amount::ZERO, "the only coin is spent");
    assert!(
        balance.unconfirmed > Amount::from_sat(60_000),
        "change is pending"
    );
    assert_eq!(
        wallet.tx_status(tx.compute_txid()),
        Some(TxStatus::Unconfirmed)
    );

    // The spent coin cannot be picked again, so no double spend.
    match wallet.build_tx([outside(90_000)], rate(1)) {
        Err(BuildTxError::InsufficientFunds { .. }) => {}
        other => panic!("expected InsufficientFunds, got {other:?}"),
    }
}

#[test]
fn sync_drops_a_tracked_payment_the_node_never_saw() {
    let (mut wallet, chain, tx) = signed_unbroadcast();
    wallet.track_pending(&tx).unwrap();
    wallet.sync(&chain).unwrap();
    assert_eq!(wallet.balance().confirmed, Amount::from_sat(100_000));
    assert!(wallet.tx_status(tx.compute_txid()).is_none());
}

#[test]
fn tracked_payment_survives_sync_once_broadcast() {
    let (mut wallet, mut chain, tx) = signed_unbroadcast();
    wallet.track_pending(&tx).unwrap();
    wallet::Broadcaster::broadcast(&chain, &tx).unwrap();
    wallet.sync(&chain).unwrap();
    assert_eq!(
        wallet.tx_status(tx.compute_txid()),
        Some(TxStatus::Unconfirmed)
    );
    chain.mine_empty(1);
    wallet.sync(&chain).unwrap();
    assert!(wallet.tx_status(tx.compute_txid()).unwrap().is_confirmed());
}

#[test]
fn tracking_a_stranger_transaction_is_rejected() {
    let (_wallet, _chain, tx) = signed_unbroadcast();
    let mut stranger = regtest_wallet();
    assert!(matches!(
        stranger.track_pending(&tx),
        Err(wallet::TrackTxError::NotRelevant)
    ));
}
