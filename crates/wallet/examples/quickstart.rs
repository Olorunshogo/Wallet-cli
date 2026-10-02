//! Quickstart: create a wallet, receive, sync, check the balance, send.
//!
//! Runs anywhere with no setup. `MockChain` stands in for a Bitcoin node; swap
//! it for `wallet::rpc::RpcClient` to use a real one (see `regtest.rs`).
//!
//! ```text
//! cargo run -p wallet --example quickstart
//! ```

use wallet::bitcoin::{Amount, FeeRate, Network};
use wallet::testkit::MockChain;
use wallet::{Broadcaster, KeySource, MnemonicLength, Recipient, Wallet, generate_mnemonic};

fn main() -> Result<(), wallet::BoxError> {
    let mut chain = MockChain::new(Network::Regtest);

    // 1. Create a wallet from a fresh 12-word mnemonic. Without `.database(..)`
    //    the wallet lives in memory; add it to persist between runs.
    let mnemonic = generate_mnemonic(MnemonicLength::Words12)?;
    let mut wallet = Wallet::builder(Network::Regtest)
        .keys(KeySource::mnemonic(mnemonic.as_str()))
        .create()?;
    println!("mnemonic:  {}", mnemonic.as_str());

    // 2. Get an address to receive on (BIP84, native SegWit).
    let receive = wallet.new_address()?;
    println!("receive:   {} (index {})", receive.address, receive.index);

    // 3. Someone pays us, and the payment is mined.
    chain.fund(receive.address.script_pubkey(), Amount::from_sat(100_000));

    // 4. Sync: read new blocks and the mempool to discover the payment.
    let report = wallet.sync(&chain)?;
    println!(
        "synced:    height {} ({} new blocks)",
        report.to.height, report.blocks_applied
    );
    println!(
        "balance:   {} sat confirmed",
        wallet.balance().confirmed.to_sat()
    );

    // 5. Pay a friend 25,000 sat at 2 sat/vB.
    let mut friend = Wallet::builder(Network::Regtest)
        .keys(KeySource::mnemonic(
            generate_mnemonic(MnemonicLength::Words12)?.as_str(),
        ))
        .create()?;
    let friend_address = friend.new_address()?.address;
    let payment = Recipient::new(friend_address.into_unchecked(), Amount::from_sat(25_000));

    let mut psbt = wallet.build_tx([payment], FeeRate::from_sat_per_vb_u32(2))?;
    wallet.sign(&mut psbt)?;
    let fee = psbt.fee()?;
    let txid = chain.broadcast(&psbt.extract_tx()?)?;
    println!("sent:      {txid} (fee {} sat)", fee.to_sat());

    // 6. Mine it and watch both wallets update.
    chain.mine_empty(1);
    wallet.sync(&chain)?;
    friend.sync(&chain)?;
    println!("status:    {:?}", wallet.tx_status(txid).expect("our tx"));
    println!("ours:      {} sat", wallet.balance().confirmed.to_sat());
    println!("friend's:  {} sat", friend.balance().confirmed.to_sat());
    Ok(())
}
