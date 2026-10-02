//! Every operation returns its own error enum, so callers can react to each
//! failure precisely instead of parsing messages.
//!
//! ```text
//! cargo run -p wallet --example error_handling
//! ```

use wallet::bitcoin::{Amount, FeeRate, Network, Txid, hashes::Hash};
use wallet::testkit::MockChain;
use wallet::{
    BuildTxError, BumpFeeError, KeySource, LoadError, Recipient, SignError, SyncError, Wallet,
};

fn main() -> Result<(), wallet::BoxError> {
    let mut chain = MockChain::new(Network::Regtest);
    let mut wallet = Wallet::builder(Network::Regtest)
        .keys(KeySource::mnemonic(fresh_mnemonic()?.as_str()))
        .create()?;
    let address = wallet.new_address()?.address;
    chain.fund(address.script_pubkey(), Amount::from_sat(10_000));
    wallet.sync(&chain)?;
    let rate = FeeRate::from_sat_per_vb_u32(1);
    let own = address.clone().into_unchecked();

    // Spending more than we have: the error carries both amounts.
    match wallet.build_tx(
        [Recipient::new(own.clone(), Amount::from_sat(50_000))],
        rate,
    ) {
        Err(BuildTxError::InsufficientFunds { needed, available }) => println!(
            "insufficient funds: short by {} sat",
            (needed - available).to_sat()
        ),
        other => panic!("unexpected: {other:?}"),
    }

    // A mainnet address on a regtest wallet is caught before anything is built.
    let mainnet = "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu".parse()?;
    match wallet.build_tx([Recipient::new(mainnet, Amount::from_sat(1_000))], rate) {
        Err(BuildTxError::WrongNetwork { address, expected }) => {
            println!("wrong network: {address} is not a {expected} address")
        }
        other => panic!("unexpected: {other:?}"),
    }

    // Dust outputs are rejected with the index of the offending output.
    match wallet.build_tx([Recipient::new(own, Amount::from_sat(100))], rate) {
        Err(BuildTxError::OutputBelowDust { index }) => println!("output #{index} is dust"),
        other => panic!("unexpected: {other:?}"),
    }

    // A watch-only wallet can build but not sign.
    let (external, internal) = wallet.public_descriptors();
    let mut watch_only = Wallet::builder(Network::Regtest)
        .keys(KeySource::descriptors(external, internal))
        .create()?;
    watch_only.sync(&chain)?;
    let to = watch_only.reveal_next_address()?.address.into_unchecked();
    let mut psbt = watch_only.build_tx([Recipient::new(to, Amount::from_sat(1_000))], rate)?;
    if let Err(SignError::WatchOnly) = watch_only.sign(&mut psbt) {
        println!("watch-only: build works, signing is refused");
    }

    // Bumping a transaction the wallet has never seen.
    let unknown = Txid::from_byte_array([1; 32]);
    if let Err(BumpFeeError::NotFound(txid)) = wallet.bump_fee(unknown, rate) {
        println!("bump fee: {txid} is not ours");
    }

    // Syncing against the wrong network's chain.
    let testnet = MockChain::new(Network::Testnet);
    if let Err(SyncError::WrongChain) = wallet.sync(&testnet) {
        println!("sync: backend is on a different chain");
    }

    // Opening a database that has no wallet.
    let dir = std::env::temp_dir().join("wallet-example-missing.sqlite");
    let _ = std::fs::remove_file(&dir);
    if let Err(LoadError::NotFound) = Wallet::builder(Network::Regtest).database(&dir).load() {
        println!("load: no wallet in {}", dir.display());
    }
    let _ = std::fs::remove_file(&dir);

    // Callers who do not need detail can use `?` with `wallet::Error`.
    let result: Result<(), wallet::Error> = (|| {
        wallet.sync(&testnet)?;
        Ok(())
    })();
    println!("as wallet::Error: {}", result.unwrap_err());
    Ok(())
}

/// A new random mnemonic for every wallet in this example.
fn fresh_mnemonic() -> Result<String, wallet::BoxError> {
    Ok(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.to_string())
}
