//! Keeping keys offline: a watch-only wallet builds, an offline wallet signs.
//!
//! The "hot" machine holds only public descriptors. It can sync, show the
//! balance and build transactions, but it cannot spend. The "cold" machine
//! holds the mnemonic and only ever sees a PSBT. They exchange PSBTs as
//! base64 text (a file, a QR code, a USB stick).
//!
//! ```text
//! cargo run -p wallet --example watch_only_signing
//! ```

use wallet::bitcoin::{Amount, FeeRate, Network, Psbt};
use wallet::testkit::MockChain;
use wallet::{Broadcaster, KeySource, Recipient, SignError, SignOutcome, Wallet};

fn main() -> Result<(), wallet::BoxError> {
    let mut chain = MockChain::new(Network::Regtest);

    // Cold machine: has the keys. Exports public descriptors once, at setup.
    let cold = Wallet::builder(Network::Regtest)
        .keys(KeySource::mnemonic(fresh_mnemonic()?.as_str()))
        .create()?;
    let (external, internal) = cold.public_descriptors();
    println!("cold exports: {external}");

    // Hot machine: watch-only, built from the public descriptors.
    let mut hot = Wallet::builder(Network::Regtest)
        .keys(KeySource::descriptors(external, internal))
        .create()?;
    println!("hot can sign? {}", hot.can_sign());

    let address = hot.new_address()?.address;
    chain.fund(address.script_pubkey(), Amount::from_sat(80_000));
    hot.sync(&chain)?;
    println!("hot sees {} sat", hot.balance().confirmed.to_sat());

    // Hot builds an unsigned PSBT...
    let to = Wallet::builder(Network::Regtest)
        .keys(KeySource::mnemonic(fresh_mnemonic()?.as_str()))
        .create()?
        .new_address()?
        .address;
    let mut unsigned = hot.build_tx(
        [Recipient::new(
            to.into_unchecked(),
            Amount::from_sat(30_000),
        )],
        FeeRate::from_sat_per_vb_u32(2),
    )?;

    // ...and cannot sign it.
    match hot.sign(&mut unsigned) {
        Err(SignError::WatchOnly) => println!("hot tried to sign: refused (watch-only)"),
        other => panic!("unexpected: {other:?}"),
    }

    // Transfer to the cold machine as base64 text.
    let to_cold = unsigned.to_string();
    println!(
        "PSBT to cold ({} chars): {}...",
        to_cold.len(),
        &to_cold[..32]
    );

    // Cold signs and returns base64.
    let mut psbt: Psbt = to_cold.parse()?;
    assert_eq!(cold.sign(&mut psbt)?, SignOutcome::Finalized);
    let to_hot = psbt.to_string();
    println!("cold signed it");

    // Hot extracts and broadcasts. Keys never touched the online machine.
    let signed: Psbt = to_hot.parse()?;
    let txid = chain.broadcast(&signed.extract_tx()?)?;
    hot.sync(&chain)?;
    println!(
        "hot broadcast {txid}: {:?}",
        hot.tx_status(txid).expect("ours")
    );
    Ok(())
}

/// A new random mnemonic for every wallet in this example.
fn fresh_mnemonic() -> Result<String, wallet::BoxError> {
    Ok(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.to_string())
}
