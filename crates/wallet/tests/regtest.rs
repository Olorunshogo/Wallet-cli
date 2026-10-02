//! End-to-end tests against a real Bitcoin Core regtest node.
//!
//! `corepc-node` downloads `bitcoind` at build time and starts a fresh node
//! per test, so these run anywhere `cargo test` can reach the network once.

use corepc_node::Node;
use wallet::bitcoin::{Amount, FeeRate, Network};
use wallet::rpc::{RpcAuth, RpcClient};
use wallet::{
    BroadcastError, Broadcaster, FeeEstimateError, FeeEstimator, KeySource, MnemonicLength,
    Recipient, SignOutcome, TxStatus, Wallet, generate_mnemonic,
};

fn start() -> (Node, RpcClient) {
    // Honours BITCOIND_EXE, then the build-time download, then PATH.
    let exe = corepc_node::exe_path().expect("find bitcoind");
    let node = Node::new(exe).expect("start bitcoind");
    let rpc = RpcClient::new(
        &node.rpc_url(),
        RpcAuth::Cookie(node.params.cookie_file.clone()),
    )
    .expect("connect rpc");
    (node, rpc)
}

/// A new in-memory wallet with freshly generated keys. `_role` only makes
/// the tests read better ("alice", "bob"); each call is a different wallet.
fn wallet(_role: &str) -> Wallet {
    let phrase = generate_mnemonic(MnemonicLength::Words12).unwrap();
    Wallet::builder(Network::Regtest)
        .keys(KeySource::mnemonic(phrase.as_str()))
        .create()
        .unwrap()
}

#[test]
fn receive_spend_and_confirm_on_regtest() {
    let (node, rpc) = start();
    let mut alice = wallet("alice");
    let mut bob = wallet("bob");

    // Mine 101 blocks to Alice. A coinbase is spendable once it has 100
    // confirmations (consensus rule), so at height 101 blocks 1 and 2 are.
    let alice_addr = alice.new_address().unwrap().address;
    node.client.generate_to_address(101, &alice_addr).unwrap();

    let report = alice.sync(&rpc).unwrap();
    assert_eq!(report.to.height, 101);
    let balance = alice.balance();
    assert_eq!(balance.confirmed, Amount::from_int_btc(50 * 2));
    assert_eq!(balance.immature, Amount::from_int_btc(50 * 99));

    // Pay Bob and watch the transaction move from mempool to a block.
    let bob_addr = bob.new_address().unwrap().address;
    let fee_rate = FeeRate::from_sat_per_vb_u32(3);
    let amount = Amount::from_sat(1_234_567);
    let mut psbt = alice
        .build_tx(
            [Recipient::new(bob_addr.as_unchecked().clone(), amount)],
            fee_rate,
        )
        .unwrap();
    assert_eq!(alice.sign(&mut psbt).unwrap(), SignOutcome::Finalized);
    let fee = psbt.fee().unwrap();
    let tx = psbt.extract_tx().unwrap();
    let txid = rpc.broadcast(&tx).unwrap();
    assert_eq!(txid, tx.compute_txid());

    bob.sync(&rpc).unwrap();
    assert_eq!(bob.balance().unconfirmed, amount);
    assert_eq!(bob.tx_status(txid), Some(TxStatus::Unconfirmed));

    let miner = wallet("miner").new_address().unwrap().address;
    node.client.generate_to_address(3, &miner).unwrap();

    bob.sync(&rpc).unwrap();
    alice.sync(&rpc).unwrap();
    assert_eq!(bob.balance().confirmed, amount);
    assert_eq!(
        bob.tx_status(txid),
        Some(TxStatus::Confirmed {
            height: 102,
            confirmations: 3
        })
    );
    // Everything Alice mined, minus what she paid Bob and the fee.
    let alice_total = Amount::from_int_btc(50 * 101) - amount - fee;
    assert_eq!(alice.balance().total(), alice_total);
}

#[test]
fn node_rejections_are_typed() {
    let (node, rpc) = start();
    let mut alice = wallet("alice");
    let addr = alice.new_address().unwrap().address;
    let other = wallet("other").new_address().unwrap().address;
    // Exactly one mature coin for Alice: her block, then 100 on top.
    node.client.generate_to_address(1, &addr).unwrap();
    node.client.generate_to_address(100, &other).unwrap();
    alice.sync(&rpc).unwrap();
    assert_eq!(alice.list_utxos().len(), 1);

    // Fresh regtest has no fee history.
    assert!(matches!(
        rpc.estimate_fee_rate(6),
        Err(FeeEstimateError::Unavailable { target_blocks: 6 })
    ));

    // Two payments spending the same coin at the same fee rate: the node
    // accepts the first and refuses to replace it with the second.
    let pay = |w: &mut Wallet, sats| {
        let to = wallet("bob").reveal_next_address().unwrap().address;
        let mut psbt = w
            .build_tx(
                [Recipient::new(
                    to.as_unchecked().clone(),
                    Amount::from_sat(sats),
                )],
                FeeRate::from_sat_per_vb_u32(2),
            )
            .unwrap();
        w.sign(&mut psbt).unwrap();
        psbt.extract_tx().unwrap()
    };
    let first = pay(&mut alice, 10_000);
    let second = pay(&mut alice, 20_000);
    assert_eq!(
        first.input[0].previous_output,
        second.input[0].previous_output
    );

    rpc.broadcast(&first).unwrap();
    match rpc.broadcast(&second) {
        Err(BroadcastError::Rejected { reason }) => assert!(!reason.is_empty()),
        other => panic!("expected Rejected, got {other:?}"),
    }
}

#[test]
fn sync_follows_a_regtest_reorg() {
    let (node, rpc) = start();
    let mut alice = wallet("alice");
    let other = wallet("other").new_address().unwrap().address;
    node.client.generate_to_address(5, &other).unwrap();

    let addr = alice.new_address().unwrap().address;
    let blocks = node.client.generate_to_address(1, &addr).unwrap();
    alice.sync(&rpc).unwrap();
    assert_eq!(alice.balance().immature, Amount::from_int_btc(50));

    // Orphan Alice's coinbase and extend a competing branch.
    let hash = blocks.0[0].parse().unwrap();
    node.client.invalidate_block(hash).unwrap();
    node.client.generate_to_address(2, &other).unwrap();

    let report = alice.sync(&rpc).unwrap();
    assert!(report.reorg_depth >= 1);
    assert_eq!(report.to.height, 7);
    // Coinbases cannot return to the mempool, so the funds are gone entirely.
    assert_eq!(alice.balance().total(), Amount::ZERO);
}

#[test]
fn bitcoin_core_accepts_a_fee_bump() {
    let (node, rpc) = start();
    let mut alice = wallet("alice");
    let addr = alice.new_address().unwrap().address;
    let other = wallet("other").new_address().unwrap().address;
    node.client.generate_to_address(1, &addr).unwrap();
    node.client.generate_to_address(100, &other).unwrap();
    alice.sync(&rpc).unwrap();

    // A low-fee payment that would sit in the mempool.
    let bob = wallet("bob").new_address().unwrap().address;
    let pay = Recipient::new(bob.as_unchecked().clone(), Amount::from_sat(500_000));
    let mut psbt = alice
        .build_tx([pay], FeeRate::from_sat_per_vb_u32(1))
        .unwrap();
    alice.sign(&mut psbt).unwrap();
    let original = rpc.broadcast(&psbt.extract_tx().unwrap()).unwrap();
    alice.sync(&rpc).unwrap();

    // Real BIP125 rules: higher rate and higher absolute fee, or rejection.
    let mut bumped = alice
        .bump_fee(original, FeeRate::from_sat_per_vb_u32(10))
        .unwrap();
    assert_eq!(alice.sign(&mut bumped).unwrap(), SignOutcome::Finalized);
    let replacement = rpc.broadcast(&bumped.extract_tx().unwrap()).unwrap();
    assert_ne!(replacement, original);

    node.client.generate_to_address(1, &other).unwrap();
    alice.sync(&rpc).unwrap();
    assert!(alice.tx_status(replacement).unwrap().is_confirmed());
    let listed: Vec<_> = alice.transactions().iter().map(|t| t.txid).collect();
    assert!(
        !listed.contains(&original),
        "original was replaced, never mined"
    );
}
