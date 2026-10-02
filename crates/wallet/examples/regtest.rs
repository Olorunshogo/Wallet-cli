//! The full flow against a real Bitcoin Core node.
//!
//! By default this downloads and starts a throwaway regtest `bitcoind`
//! (via `corepc-node`), so it needs no setup:
//!
//! ```text
//! cargo run -p wallet --example regtest
//! ```
//!
//! To use a node you already run, set its URL and credentials. The example
//! then only syncs and reports, since it cannot mine on your node:
//!
//! ```text
//! WALLET_RPC_URL=http://127.0.0.1:18443 \
//! WALLET_RPC_COOKIE=$HOME/.bitcoin/regtest/.cookie \
//! cargo run -p wallet --example regtest
//! ```
//!
//! `WALLET_RPC_USER` and `WALLET_RPC_PASS` work instead of the cookie.

use std::env;

use corepc_node::Node;
use wallet::bitcoin::{Amount, FeeRate, Network};
use wallet::rpc::{RpcAuth, RpcClient};
use wallet::{FeeEstimateError, FeeEstimator, KeySource, Recipient, Wallet};

fn main() -> Result<(), wallet::BoxError> {
    let mut wallet = Wallet::builder(Network::Regtest)
        .keys(KeySource::mnemonic(fresh_mnemonic()?.as_str()))
        .create()?;

    if let Ok(url) = env::var("WALLET_RPC_URL") {
        return existing_node(&url, &mut wallet);
    }

    println!("starting a throwaway regtest bitcoind...");
    // Honours BITCOIND_EXE, then the build-time download, then PATH.
    let node = Node::new(corepc_node::exe_path()?)?;
    let rpc = RpcClient::new(
        &node.rpc_url(),
        RpcAuth::Cookie(node.params.cookie_file.clone()),
    )?;
    println!("node RPC at {}", node.rpc_url());

    // Mine 101 blocks to ourselves so the first coinbase matures.
    let address = wallet.new_address()?.address;
    node.client.generate_to_address(101, &address)?;
    let report = wallet.sync(&rpc)?;
    let balance = wallet.balance();
    println!(
        "synced to {}: {} sat spendable, {} sat immature",
        report.to.height,
        balance.confirmed.to_sat(),
        balance.immature.to_sat()
    );

    // Fresh regtest has no fee history, which is a typed, expected case.
    let fee_rate = match rpc.estimate_fee_rate(6) {
        Ok(rate) => rate,
        Err(FeeEstimateError::Unavailable { .. }) => {
            println!("no fee estimate yet, using 2 sat/vB");
            FeeRate::from_sat_per_vb_u32(2)
        }
        Err(e) => return Err(e.into()),
    };

    let to = Wallet::builder(Network::Regtest)
        .keys(KeySource::mnemonic(fresh_mnemonic()?.as_str()))
        .create()?
        .new_address()?
        .address;
    let txid = wallet.send(
        [Recipient::new(
            to.into_unchecked(),
            Amount::from_sat(1_000_000),
        )],
        fee_rate,
        &rpc,
    )?;
    wallet.sync(&rpc)?;
    println!("sent {txid}: {:?}", wallet.tx_status(txid).expect("ours"));

    node.client.generate_to_address(1, &address)?;
    wallet.sync(&rpc)?;
    println!(
        "after one block: {:?}",
        wallet.tx_status(txid).expect("ours")
    );
    Ok(())
}

fn existing_node(url: &str, wallet: &mut Wallet) -> Result<(), wallet::BoxError> {
    let auth = match (env::var("WALLET_RPC_USER"), env::var("WALLET_RPC_PASS")) {
        (Ok(user), Ok(pass)) => RpcAuth::UserPass { user, pass },
        _ => RpcAuth::Cookie(
            env::var("WALLET_RPC_COOKIE")
                .map_err(|_| "set WALLET_RPC_COOKIE or WALLET_RPC_USER/WALLET_RPC_PASS")?
                .into(),
        ),
    };
    let rpc = RpcClient::new(url, auth)?;
    let report = wallet.sync(&rpc)?;
    println!("synced to height {}", report.to.height);
    println!("balance: {:?}", wallet.balance());
    println!("fund this wallet at {}", wallet.new_address()?.address);
    Ok(())
}

/// A new random mnemonic for every wallet in this example.
fn fresh_mnemonic() -> Result<String, wallet::BoxError> {
    Ok(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.to_string())
}
