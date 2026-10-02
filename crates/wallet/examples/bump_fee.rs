//! Speeding up a stuck payment with replace-by-fee (BIP125).
//!
//! A payment sent at 1 sat/vB is replaced by one paying 10 sat/vB. The
//! recipient and amount stay the same; the extra fee comes out of change.
//!
//! ```text
//! cargo run -p wallet --example bump_fee
//! ```

use wallet::bitcoin::{Amount, FeeRate, Network};
use wallet::testkit::MockChain;
use wallet::{Broadcaster, KeySource, Recipient, Wallet};

fn main() -> Result<(), wallet::BoxError> {
    let mut chain = MockChain::new(Network::Regtest);
    let mut wallet = Wallet::builder(Network::Regtest)
        .keys(KeySource::mnemonic(fresh_mnemonic()?.as_str()))
        .create()?;
    let address = wallet.new_address()?.address;
    chain.fund(address.script_pubkey(), Amount::from_sat(200_000));
    wallet.sync(&chain)?;

    let to = Wallet::builder(Network::Regtest)
        .keys(KeySource::mnemonic(fresh_mnemonic()?.as_str()))
        .create()?
        .new_address()?
        .address;
    let pay = Recipient::new(to.into_unchecked(), Amount::from_sat(50_000));

    // Payments signal RBF by default (`TxRequest::enable_rbf`).
    let mut psbt = wallet.build_tx([pay], FeeRate::from_sat_per_vb_u32(1))?;
    wallet.sign(&mut psbt)?;
    let original_fee = psbt.fee()?;
    let original = chain.broadcast(&psbt.extract_tx()?)?;
    wallet.sync(&chain)?;
    println!(
        "sent    {original} at 1 sat/vB, fee {} sat",
        original_fee.to_sat()
    );

    // Fees spiked; bump it to 10 sat/vB.
    let mut bumped = wallet.bump_fee(original, FeeRate::from_sat_per_vb_u32(10))?;
    wallet.sign(&mut bumped)?;
    let new_fee = bumped.fee()?;
    let replacement = chain.broadcast(&bumped.extract_tx()?)?;
    println!(
        "bumped  {replacement} at 10 sat/vB, fee {} sat",
        new_fee.to_sat()
    );
    println!(
        "mempool now holds only the replacement: {:?}",
        chain.mempool_txids()
    );

    chain.mine_empty(1);
    wallet.sync(&chain)?;
    println!("status  {:?}", wallet.tx_status(replacement).expect("ours"));
    println!(
        "history lists the original? {}",
        wallet.transactions().iter().any(|t| t.txid == original)
    );
    Ok(())
}

/// A new random mnemonic for every wallet in this example.
fn fresh_mnemonic() -> Result<String, wallet::BoxError> {
    Ok(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.to_string())
}
