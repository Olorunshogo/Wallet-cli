# v2: native engine

## Why v1 wraps BDK

v1 is a solo MVP. Wrapping `bdk_wallet` delivered a working, tested wallet quickly with little risk in the fiddly parts (descriptor handling, change, fee math, chain canonicalization). The time went into what the project is judged on: API design, error handling, modularity and tests.

| | Wrap `bdk_wallet` (v1) | Build on `bitcoin` + `miniscript` + `bip39` (v2) |
|---|---|---|
| Speed | Fast | Slower |
| What you learn | Mostly BDK's API | UTXO tracking, coin selection, fees, PSBT flow |
| What you can present | "We wrapped a library" | "We designed a wallet engine" |
| Risk | Low | Medium, but manageable if scope stays tight |

## Goal

Add a `wallet-native` crate that implements `wallet_core::WalletEngine` using only `bitcoin`, `miniscript` and `bip39`. **Callers do not change**, because they only see `wallet-core` types:

```rust
// v1
let wallet: Wallet<BdkEngine> = Wallet::builder(network).keys(keys).create()?;
// v2
let wallet: Wallet<NativeEngine> = Wallet::from_engine(NativeEngine::builder(network).keys(keys).create()?);
```

`wallet-rpc`, `MockChain`, the CLI and the whole public API test suite are reused unchanged.

## Replacement map

| BDK component used in v1 | v2 native replacement |
|---|---|
| `Bip84` templates, descriptor derivation | `miniscript::Descriptor::at_derivation_index`, `derived_descriptor` |
| Keychain index and lookahead (`KeychainTxOutIndex`) | Own `AddressIndex`: next index per keychain, used set, script-to-index map with a gap limit |
| `LocalChain` checkpoints | Own checkpoint list (height, hash), reorg rewind to the agreement point |
| `TxGraph` + canonicalization | Own tx store: txs by id, anchors (height, hash), last seen; spent-by map for UTXO derivation |
| Balance and `list_unspent` | Derived from the tx store against the current tip, including coinbase maturity |
| Coin selection | `CoinSelector` trait with LargestFirst and BranchAndBound (own, or `bdk_coin_select` as a dependency) |
| `TxBuilder` | Own builder; input weight from `Descriptor::max_weight_to_satisfy`, dust check, change output on the internal keychain, RBF sequence |
| Signing and finalizing | `Psbt::sign` with the key map (already done in v1), then `miniscript::psbt::PsbtExt::finalize_mut` |
| SQLite persistence (`bdk_chain` rusqlite) | `WalletStore` trait with a `ChangeSet` of deltas; JSON file and SQLite implementations |

v1 already does two of these without BDK: signing (our key map plus `bitcoin::Psbt::sign`) and block fetching (`BlockSource` instead of `bdk_bitcoind_rpc::Emitter`).

## Milestones

1. `AddressIndex` and derivation, passing the BIP84 vectors.
2. Tx store, checkpoints and `sync` from a `BlockSource`, including reorg rewind.
3. Balance, UTXOs, history and status.
4. Coin selection trait and two strategies, with property tests: inputs always cover outputs plus fee, change is never dust.
5. Tx builder and finalization.
6. `WalletStore` and persistence.
7. Run `crates/wallet/tests/wallet.rs` and `regtest.rs` against both engines. **Acceptance: the same suite passes on both.**

## Multiple wallets (done in v1)

Named wallets, the `w` switcher in the TUI and a shared local regtest node were planned here and have shipped in v1; see [CLI.md](../CLI.md#named-wallets) and [TUI.md](../TUI.md#switching-wallets). The library needed no change: each `Wallet` owns its own database, so storage layout stays a concern of the apps.

## Open questions

- Keep SQLite, or start with a JSON store and add SQLite later?
- Handle reorged-out transactions like BDK (stay pending until conflicted), or drop them immediately when absent from the mempool?
- Taproot (BIP86) support in v2, or keep BIP84 only?
