//! Plugging in your own chain backend.
//!
//! The wallet never talks to a node directly. It asks a [`BlockSource`] for
//! blocks and mempool transactions, so anything that can serve blocks can
//! drive a sync: Bitcoin Core, an HTTP API, a block file, a database.
//!
//! This example shows three things:
//! 1. `BlockFile`: a backend written from scratch (a list of blocks in memory,
//!    standing in for blocks read from disk or fetched over HTTP).
//! 2. `Logged<S>`: a decorator that wraps *any* backend to log every call.
//! 3. Choosing a backend at runtime with `Box<dyn BlockSource>`.
//!
//! ```text
//! cargo run -p wallet --example custom_backend
//! ```

use std::cell::Cell;

use wallet::bitcoin::{Amount, Block, BlockHash, Network, Transaction};
use wallet::testkit::MockChain;
use wallet::{BlockId, BlockSource, KeySource, SourceError, Wallet};

// === 1. A backend from scratch

/// Serves blocks from a list. Index = height.
struct BlockFile {
    blocks: Vec<Block>,
}

impl BlockSource for BlockFile {
    fn tip(&self) -> Result<BlockId, SourceError> {
        let height = self.blocks.len() as u32 - 1;
        Ok(BlockId {
            height,
            hash: self.blocks[height as usize].block_hash(),
        })
    }

    fn block_hash(&self, height: u32) -> Result<BlockHash, SourceError> {
        self.blocks
            .get(height as usize)
            .map(Block::block_hash)
            .ok_or(SourceError::HeightNotFound(height))
    }

    fn block(&self, hash: &BlockHash) -> Result<Block, SourceError> {
        self.blocks
            .iter()
            .find(|b| b.block_hash() == *hash)
            .cloned()
            .ok_or(SourceError::BlockNotFound(*hash))
    }

    fn mempool(&self) -> Result<Vec<(Transaction, u64)>, SourceError> {
        // A block file has no mempool. Returning nothing is valid.
        Ok(Vec::new())
    }
}

// === 2. A decorator that works with any backend

/// Wraps a backend and prints each call, counting how many blocks were fetched.
struct Logged<S> {
    inner: S,
    blocks_fetched: Cell<u32>,
}

impl<S: BlockSource> BlockSource for Logged<S> {
    fn tip(&self) -> Result<BlockId, SourceError> {
        let tip = self.inner.tip()?;
        println!("  tip()            -> height {}", tip.height);
        Ok(tip)
    }

    fn block_hash(&self, height: u32) -> Result<BlockHash, SourceError> {
        self.inner.block_hash(height)
    }

    fn block(&self, hash: &BlockHash) -> Result<Block, SourceError> {
        let block = self.inner.block(hash)?;
        self.blocks_fetched.set(self.blocks_fetched.get() + 1);
        println!(
            "  block({:.8}..)  -> {} txs",
            hash.to_string(),
            block.txdata.len()
        );
        Ok(block)
    }

    fn mempool(&self) -> Result<Vec<(Transaction, u64)>, SourceError> {
        let txs = self.inner.mempool()?;
        println!("  mempool()        -> {} txs", txs.len());
        Ok(txs)
    }
}

fn main() -> Result<(), wallet::BoxError> {
    let mut wallet = Wallet::builder(Network::Regtest)
        .keys(KeySource::mnemonic(fresh_mnemonic()?.as_str()))
        .create()?;
    let address = wallet.new_address()?.address;

    // Produce a small chain that pays our address, then copy its blocks into
    // a `BlockFile`, as if they had been read from disk.
    let mut chain = MockChain::new(Network::Regtest);
    chain.mine_empty(2);
    chain.fund(address.script_pubkey(), Amount::from_sat(42_000));
    let tip = chain.tip()?.height;
    let blocks = (0..=tip)
        .map(|h| chain.block(&chain.block_hash(h)?))
        .collect::<Result<Vec<_>, _>>()?;

    // Pick a backend at runtime. Both are `Box<dyn BlockSource>`.
    let use_file = std::env::args().all(|a| a != "--mock");
    let backend: Box<dyn BlockSource> = if use_file {
        Box::new(BlockFile { blocks })
    } else {
        Box::new(chain)
    };

    // Wrap whichever one we picked; the wallet does not know or care.
    let logged = Logged {
        inner: backend,
        blocks_fetched: Cell::new(0),
    };

    println!(
        "syncing through {}:",
        if use_file { "BlockFile" } else { "MockChain" }
    );
    let report = wallet.sync(&logged)?;
    println!(
        "synced to height {} after fetching {} blocks",
        report.to.height,
        logged.blocks_fetched.get()
    );
    println!("balance: {} sat", wallet.balance().confirmed.to_sat());

    println!("\nsyncing again (nothing new, so no blocks fetched):");
    wallet.sync(&logged)?;
    Ok(())
}

/// A new random mnemonic for every wallet in this example.
fn fresh_mnemonic() -> Result<String, wallet::BoxError> {
    Ok(wallet::generate_mnemonic(wallet::MnemonicLength::Words12)?.to_string())
}
