# Wallet Library: Complete Review & Bitcoin Lectures

## Table of Contents

1. [Session Recap](#1-session-recap)
2. [Crate-by-Crate Analysis](#2-crate-by-crate-analysis)
3. [Lecture: Bitcoin Wallet Creation](#3-lecture-bitcoin-wallet-creation)
4. [Lecture: PSBT (Partially Signed Bitcoin Transaction)](#4-lecture-psbt-partially-signed-bitcoin-transaction)
5. [What We Did Right](#5-what-we-did-right)
6. [What We Can Do Better](#6-what-we-can-do-better)
7. [Polar / Lightning: Do We Need It?](#7-polar--lightning-do-we-need-it)
8. [Architecture Diagrams](#8-architecture-diagrams)
9. [Data Flow Deep-Dives](#9-data-flow-deep-dives)
10. [Dependency Graph & Version Matrix](#10-dependency-graph--version-matrix)

---

## 1. Session Recap

### 1.1 Project context

This is **Project 7: Wallet Library** from the Rust for Bitcoin cohort capstone. The goal is a reusable Rust crate that provides wallet functionality through a clean API. Think of it as a small, focused BDK. It encapsulates key management, address derivation, UTXO tracking, coin selection, transaction building and signing behind a well-designed public API, so that apps like the Bitcoin Wallet or Bitcoin CLI can use it without reimplementing wallet logic.

### 1.2 What exists right now

The workspace is a Cargo workspace (edition 2024, resolver 3) with five crates:

| Layer | Crate | What it is | Lines of code |
|---|---|---|---|
| **Core** | `wallet-core` | Types, traits, errors, MockChain: no IO, no BDK | ~700 |
| **Engine** | `wallet-bdk` | `BdkEngine`: wraps `bdk_wallet` + SQLite | ~600 |
| **Backend** | `wallet-rpc` | `RpcClient`: Bitcoin Core JSON-RPC | ~150 |
| **Facade** | `wallet` | `Wallet<E>`: the public entry point | ~210 |
| **Binary** | `wallet-cli` | Demo CLI using the facade | ~440 |
| **Docs** | `docs/architecture.md`, `docs/v2/README.md` | Design docs + v2 plan | ~160 |

### 1.3 Key technical decisions already made

- **Wrap BDK, don't build from primitives (v1).** BDK is a private implementation detail: no `bdk_*` type appears in any public API.
- **Block-based chain backend.** The `BlockSource` trait hands full blocks to the engine, which filters what's relevant. No `txindex` or address index needed on the node.
- **Keymap held outside BDK.** BDK only receives public descriptors. Private keys stay in the engine for signing.
- **One error enum per operation.** Each public method returns the narrowest `thiserror` enum describing what can actually go wrong.
- **Generic facade.** `Wallet<E: WalletEngine = BdkEngine>`: the default makes it easy to use, the generic makes it easy to swap.

### 1.4 Git state

The repo has no commits yet (fresh `main` branch). The codebase is fully built and tested but uncommitted.

---

## 2. Crate-by-Crate Analysis

### 2.1 `wallet-core`: The vocabulary layer

**Purpose:** Defines everything shared between engines and backends. It performs no IO and does not depend on BDK, so a future native engine can implement the same traits.

**Files:**

| File | Contents |
|---|---|
| `lib.rs` | Module declarations and re-exports |
| `types.rs` | All domain types (see below) |
| `error.rs` | One `thiserror` enum per operation |
| `chain.rs` | `BlockSource`, `Broadcaster`, `FeeEstimator` traits |
| `engine.rs` | `WalletEngine` trait |
| `keys.rs` | `KeySource` enum with `Zeroizing` secrets |
| `testkit.rs` | `MockChain`: in-memory chain for tests |

**Domain types in detail:**

```
Keychain         : External (receive, .../0/*) or Internal (change, .../1/*)
AddressInfo      : address + derivation index + keychain
BlockId          : height + hash
TxStatus         : Unconfirmed | Confirmed { height, confirmations }
Balance          : confirmed + unconfirmed + immature (coinbase)
Utxo             : outpoint + value + script_pubkey + keychain + index + status
TxDetails        : txid + sent + received + fee + status
CoinSelection    : BranchAndBound | LargestFirst | OldestFirst
Recipient        : address (NetworkUnchecked) + amount
TxRequest        : recipients + fee_rate + coin_selection + enable_rbf
SignOutcome      : Finalized | Partial
SyncReport       : from + to + blocks_applied + reorg_depth + mempool_txs
```

**The three chain traits:**

```rust
pub trait BlockSource {
    fn tip(&self) -> Result<BlockId, SourceError>;
    fn block_hash(&self, height: u32) -> Result<BlockHash, SourceError>;
    fn block(&self, hash: &BlockHash) -> Result<Block, SourceError>;
    fn mempool(&self) -> Result<Vec<(Transaction, u64)>, SourceError>;
}

pub trait Broadcaster {
    fn broadcast(&self, tx: &Transaction) -> Result<Txid, BroadcastError>;
}

pub trait FeeEstimator {
    fn estimate_fee_rate(&self, target_blocks: u16) -> Result<FeeRate, FeeEstimateError>;
}
```

These are **object-safe** and have blanket impls for `&T` and `Box<T>`, so a backend can be chosen at runtime as `Box<dyn BlockSource>`.

**The engine trait:**

```rust
pub trait WalletEngine {
    fn network(&self) -> Network;
    fn can_sign(&self) -> bool;
    fn tip(&self) -> BlockId;
    fn new_address(&mut self) -> Result<AddressInfo, AddressError>;
    fn reveal_next_address(&mut self) -> Result<AddressInfo, AddressError>;
    fn balance(&self) -> Balance;
    fn list_utxos(&self) -> Vec<Utxo>;
    fn transactions(&self) -> Vec<TxDetails>;
    fn tx_status(&self, txid: Txid) -> Option<TxStatus>;
    fn build_tx(&mut self, request: &TxRequest) -> Result<Psbt, BuildTxError>;
    fn sign(&self, psbt: &mut Psbt) -> Result<SignOutcome, SignError>;
    fn sync(&mut self, source: &dyn BlockSource) -> Result<SyncReport, SyncError>;
}
```

**Error enums (one per operation):**

| Operation | Error enum | Notable variants |
|---|---|---|
| create | `CreateError` | `InvalidMnemonic`, `InvalidDescriptor`, `AlreadyExists` |
| load | `LoadError` | `NotFound`, `NetworkMismatch { expected, found }`, `DescriptorMismatch` |
| address | `AddressError` | `Persist` |
| chain read | `SourceError` | `HeightNotFound`, `BlockNotFound`, `Connection`, `InvalidResponse` |
| sync | `SyncError` | `WrongChain`, `ApplyBlock { height, .. }`, `Source(SourceError)` |
| broadcast | `BroadcastError` | `Rejected { reason }`, `Connection` |
| fee estimate | `FeeEstimateError` | `Unavailable { target_blocks }`, `Connection` |
| build tx | `BuildTxError` | `InsufficientFunds { needed, available }`, `WrongNetwork`, `OutputBelowDust { index }`, `FeeTooLow` |
| sign | `SignError` | `WatchOnly`, `NothingToSign`, `InvalidPsbt` |
| umbrella | `Error` | `#[from]` all of the above + `NotFinalized`, `Extract` |

All enums are `#[non_exhaustive]` so new variants can be added without breaking callers.

**KeySource, the secret holder:**

```rust
pub enum KeySource {
    Mnemonic { phrase: Zeroizing<String>, passphrase: Option<Zeroizing<String>> },
    Descriptors { external: Zeroizing<String>, internal: Zeroizing<String> },
}
```

- Secrets are held in `Zeroizing` buffers (wiped on drop).
- The `Debug` impl prints `<redacted>`: covered by a test.
- `MnemonicLength` enum: `Words12` (default) or `Words24`.

**MockChain, the test chain:**

Implements all three chain traits in memory:
- `new(network)`: chain with only the genesis block
- `mine(txs)` / `mine_empty(count)`: mine blocks with valid merkle roots and BIP34 coinbases
- `fund(script, amount)`: pay a script in a new block
- `fund_unconfirmed(script, amount)`: pay a script into the mempool only
- `reorg(depth)`: drop the top N blocks
- `set_fee_rate(rate)`: control fee estimation

Blocks are structurally valid (linked headers, correct merkle roots, BIP34 coinbases) but have no proof of work: wallet engines don't check PoW.

**Do we need this crate?**

**Yes: this is the most important crate.** It's the "pure Rust" heart the project description calls for. It has zero IO, zero BDK, and the compiler enforces that (no dependencies to import). The v2 native engine plan depends entirely on this crate staying stable. Without it, every adapter would redefine the same types.

**Necessity: 10/10 (irreplaceable).**

---

### 2.2 `wallet-bdk`: The v1 engine

**Purpose:** Implements `WalletEngine` by wrapping `bdk_wallet` with SQLite persistence.

**Files:**

| File | Contents |
|---|---|
| `lib.rs` | Module declarations and re-exports |
| `builder.rs` | `BdkEngineBuilder`: fluent configuration |
| `engine.rs` | `BdkEngine`: the `WalletEngine` impl |
| `keys.rs` | `resolve()`: KeySource → descriptors + keymap; `generate_mnemonic()` |
| `convert.rs` | BDK types → wallet-core types (anti-corruption layer) |
| `sync.rs` | Block-based sync with reorg handling |

**BdkEngine struct:**

```rust
pub struct BdkEngine {
    pub(crate) wallet: PersistedWallet<Connection>,  // BDK's wallet
    pub(crate) db: Connection,                        // SQLite connection
    coin_selection: CoinSelection,                    // default strategy
    pub(crate) birthday: Option<u32>,                 // scan-from height
    signer: Option<KeyMapWrapper>,                   // private keys (None = watch-only)
}
```

**The builder pattern:**

```rust
let engine = BdkEngine::builder(Network::Regtest)
    .keys(KeySource::mnemonic("abandon ... about"))  // required for create
    .database("wallet.sqlite")                        // or .in_memory()
    .coin_selection(CoinSelection::BranchAndBound)    // default
    .lookahead(20)                                    // gap limit
    .birthday(100)                                    // skip blocks below this
    .create()?;                                       // or .load() / .create_or_load()
```

**Key resolution (`keys.rs`):**

The `resolve()` function turns a `KeySource` into BIP84 descriptors plus a keymap:

1. Parse the BIP39 mnemonic (`Mnemonic::parse_in`)
2. Derive the seed with `mnemonic.to_seed(passphrase)` (PBKDF2-HMAC-SHA512, 2048 iterations, 512-bit output)
3. Create the master xprv: `Xpriv::new_master(network, seed)`
4. Use BDK's `Bip84(xprv, KeychainKind::External)` template to produce: `wpkh(m/84'/coin'/0'/0/*)`
5. The keymap holds the private keys for signing

Only the **public** descriptors are handed to BDK. The keymap stays in the engine.

**The anti-corruption layer (`convert.rs`):**

Every BDK type is translated to a wallet-core type in one place:

```rust
pub(crate) fn keychain(k: KeychainKind) -> Keychain { ... }
pub(crate) fn block_id(id: bdk_wallet::chain::BlockId) -> BlockId { ... }
pub(crate) fn status(pos: &ChainPosition<...>, tip_height: u32) -> TxStatus { ... }
pub(crate) fn load_error(err: LoadWithPersistError<...>) -> LoadError { ... }
pub(crate) fn build_tx_error(err: CreateTxError) -> BuildTxError { ... }
pub(crate) fn sign_error(err: SignerError) -> SignError { ... }
```

This is what stops BDK types leaking into the public API.

**Sync (`sync.rs`):**

Block-based sync with reorg handling:

1. Check the backend's genesis hash matches the wallet network (`SyncError::WrongChain` otherwise)
2. Walk the wallet's checkpoints down until one matches the backend's chain: everything above that point was reorged out
3. Fetch each block from the agreement point (or the birthday height) to the tip and apply it with `apply_block_connected_to`
4. Apply mempool transactions, and mark unconfirmed wallet transactions the node no longer has as evicted
5. Persist every 500 blocks (resumable long sync) and at the end
6. Return a `SyncReport`

**Do we need this crate?**

**Yes for v1.** It's the only working engine. The modularity is real: `Wallet<E: WalletEngine>` is generic, and `Wallet::from_engine()` accepts any implementor. The swap is a `Cargo.toml` change + one line at the call site.

**Necessity: 9/10 (needed now, replaceable by v2).**

---

### 2.3 `wallet-rpc`: The chain backend

**Purpose:** Implements `BlockSource`, `Broadcaster` and `FeeEstimator` over Bitcoin Core JSON-RPC.

**The RpcClient:**

```rust
pub struct RpcClient {
    client: bitcoincore_rpc::Client,
}

impl RpcClient {
    pub fn new(url: &str, auth: RpcAuth) -> Result<Self, SourceError>;
}

pub enum RpcAuth {
    Cookie(PathBuf),
    UserPass { user: String, pass: String },
}
```

**BlockSource implementation:**

| Method | RPC call | Notes |
|---|---|---|
| `tip()` | `getblockchaininfo` | Returns height + best block hash |
| `block_hash(height)` | `getblockhash` | Translates `-8` (invalid parameter) → `HeightNotFound` |
| `block(hash)` | `getblock` (verbosity 0) | Raw hex, decoded locally: much faster than verbose JSON |
| `mempool()` | `getrawmempool` + `getrawtransaction` per tx | Skips txs mined/evicted between calls |

**Broadcaster implementation:**

`sendrawtransaction`: translates RPC error codes:
- `-25` (verify error), `-26` (verify rejected), `-27` (already in chain) → `BroadcastError::Rejected { reason }`
- Everything else → `BroadcastError::Connection`

**FeeEstimator implementation:**

`estimatesmartfee`: converts BTC/kvB to sat/kwu (1 vB = 4 wu, so `per_kvb.to_sat() / 4`).

**Do we need this crate?**

**Yes for v1.** It's the only real chain backend. The `BlockSource` trait is block-based on purpose: the engine receives full blocks and filters, so no `txindex` or address index is needed on the node. This is what makes the same backend work for BDK today and a native engine tomorrow.

**Necessity: 8/10 (needed for real chains, but Esplora could be a second impl).**

---

### 2.4 `wallet`: The facade

**Purpose:** The public entry point. This is the crate consumers actually depend on.

**The Wallet struct:**

```rust
pub struct Wallet<E: WalletEngine = BdkEngine> {
    engine: E,
}
```

The default type parameter means `Wallet` (without turbofish) is `Wallet<BdkEngine>`, so it is easy to use out of the box. The generic means `Wallet<NativeEngine>` works too, so engines are easy to swap.

**Two impl blocks:**

```rust
impl Wallet<BdkEngine> {
    pub fn builder(network: Network) -> WalletBuilder { ... }
    pub fn public_descriptors(&self) -> (String, String) { ... }
}

impl<E: WalletEngine> Wallet<E> {
    pub fn from_engine(engine: E) -> Self { ... }
    pub fn engine(&self) -> &E { ... }
    pub fn into_engine(self) -> E { ... }
    // ... all the delegated methods
}
```

**The `send` convenience method:**

```rust
pub fn send(
    &mut self,
    recipients: impl IntoIterator<Item = Recipient>,
    fee_rate: FeeRate,
    broadcaster: &impl Broadcaster,
) -> Result<Txid, Error> {
    let mut psbt = self.build_tx(recipients, fee_rate)?;
    if self.sign(&mut psbt)? != SignOutcome::Finalized {
        return Err(Error::NotFinalized);
    }
    let tx = psbt.extract_tx().map_err(|e| Error::Extract(Box::new(e)))?;
    Ok(broadcaster.broadcast(&tx)?)
}
```

This is the cleanest demonstration of the whole flow in 6 lines: build → sign → extract → broadcast.

**Feature flags:**

| Feature | Default | Effect |
|---|---|---|
| `rpc` | yes | Includes `wallet-rpc` as a dependency, exposes `wallet::rpc` module |
| `serde` | no | Enables `Serialize`/`Deserialize` on all public types |

**Do we need this crate?**

**Yes.** This is the "small, focused BDK" the project description asks for. It's the crate that gets published and depended on.

**Necessity: 10/10 (this is the product).**

---

### 2.5 `wallet-cli`: The demo binary

**Purpose:** A `clap`-based CLI that exercises every feature of the library.

**Commands:**

| Command | What it does |
|---|---|
| `init [--words 12\|24]` | Create a new wallet with a freshly generated mnemonic |
| `restore --mnemonic "..." [--passphrase "..."] [--birthday N]` | Restore from an existing mnemonic |
| `address [--new]` | Show the next unused (or a fresh) receive address |
| `sync` | Update the wallet from the node |
| `balance` | Show confirmed, unconfirmed, immature balance |
| `utxos` | List unspent outputs |
| `history` | List wallet transactions |
| `send <address> <amount> [--fee-rate N] [--target N] [--selection bnb\|largest\|oldest] [--dry-run]` | Pay an address |
| `status <txid>` | Show confirmation status |
| `descriptors` | Print public descriptors (for a watch-only copy) |

**Global options:**
- `--network <network>` (default: regtest, env: `WALLET_NETWORK`)
- `--datadir <path>` (default: `./.wallet/<network>`, env: `WALLET_DATADIR`)
- `--json`: machine-readable output
- `--rpc-url`, `--rpc-cookie`, `--rpc-user`, `--rpc-pass`: node connection

**Data directory layout:**

```
.wallet/regtest/
├── wallet.sqlite    # wallet state (public data only: no private keys)
├── mnemonic         # BIP39 mnemonic (mode 0600)
├── passphrase       # BIP39 passphrase (mode 0600, optional)
└── meta.json        # CLI settings such as the birthday height
```

**Output formatting:**

Every command prints through `Output` which has two modes:
- Human-readable text (default)
- JSON (`--json` flag)

The `emit()` method takes a `serde_json::Value` and a closure producing the text version, so the two formats stay in sync.

**Do we need this crate?**

**Yes for demonstration, no for the library.** It's the proof that the library works end-to-end. It's also the only crate that uses `anyhow` (appropriate for a binary). The CLI stores the mnemonic unencrypted (mode 0600): fine for regtest, documented as such.

**Necessity: 7/10 (essential for demo/testing, not part of the library proper).**

---

### 2.6 Summary: Do we need each crate?

| Crate | Need? | Why | Necessity |
|---|---|---|---|
| `wallet-core` | **Absolutely** | The shared vocabulary; v2 depends on it | 10/10 |
| `wallet-bdk` | **Yes (v1)** | The only engine today; swap point for v2 | 9/10 |
| `wallet-rpc` | **Yes** | The only real chain backend | 8/10 |
| `wallet` | **Absolutely** | The public API consumers depend on | 10/10 |
| `wallet-cli` | **Yes (demo)** | Proof of end-to-end functionality | 7/10 |

**Could we collapse any?**

You could merge `wallet-bdk` into `wallet` and `wallet-rpc` into `wallet`, but you'd lose the modularity that's the whole point of the project. The 5-crate split is correct: it mirrors the seam structure (engine / backend / facade / binary) and lets each piece be tested and replaced independently.

---

## 3. Lecture: Bitcoin Wallet Creation

### 3.1 The cryptographic hierarchy

Bitcoin wallets are built on a hierarchy of cryptographic derivations. Each layer builds on the one below:

```
┌─────────────────────────────────────────────────────────────┐
│  BIP39 Mnemonic (12 or 24 words)                            │
│  Human-readable backup of the wallet's root entropy          │
└──────────────────────────┬──────────────────────────────────┘
                           │  PBKDF2-HMAC-SHA512
                           │  password: "mnemonic" + passphrase
                           │  iterations: 2048
                           │  output: 512 bits
┌──────────────────────────▼──────────────────────────────────┐
│  BIP32 Seed (512 bits)                                      │
│  The root of the deterministic key tree                     │
└──────────────────────────┬──────────────────────────────────┘
                           │  HMAC-SHA512("Bitcoin seed", seed)
                           │  → 256-bit IL (private key) + 256-bit IR (chain code)
┌──────────────────────────▼──────────────────────────────────┐
│  Master Extended Private Key (xprv) at path m/              │
│  The root key from which all child keys are derived         │
└──────────────────────────┬──────────────────────────────────┘
                           │  CKDpriv (Child Key Derivation, private)
                           │  hardened: m/84'  (BIP84 purpose)
┌──────────────────────────▼──────────────────────────────────┐
│  Purpose Key at path m/84'                                  │
│  84' = native SegWit (bech32 addresses)                     │
└──────────────────────────┬──────────────────────────────────┘
                           │  hardened: m/84'/coin'
                           │  coin: 0' = mainnet, 1' = testnet
┌──────────────────────────▼──────────────────────────────────┐
│  Account Key at path m/84'/coin'/0'                         │
│  Standard account (BIP44/BIP49/BIP84 all use account 0)     │
└──────────────────────────┬──────────────────────────────────┘
                           │  non-hardened: m/84'/coin'/0'/0
                           │  0 = external keychain (receive)
                           │  1 = internal keychain (change)
┌──────────────────────────▼──────────────────────────────────┐
│  Keychain Key at path m/84'/coin'/0'/0                      │
│  All receive addresses are derived from this branch         │
└──────────────────────────┬──────────────────────────────────┘
                           │  non-hardened: m/84'/coin'/0'/0/index
                           │  index: 0, 1, 2, ... (sequential)
┌──────────────────────────▼──────────────────────────────────┐
│  Address Key at path m/84'/coin'/0'/0/0                     │
│  The specific key for a single receive address              │
└──────────────────────────┬──────────────────────────────────┘
                           │  hash160 (SHA256 → RIPEMD160)
                           │  of the compressed public key (33 bytes)
┌──────────────────────────▼──────────────────────────────────┐
│  Witness Program (20 bytes)                                 │
│  The scriptPubKey for P2WPKH                                │
└──────────────────────────┬──────────────────────────────────┘
                           │  bech32 encode
                           │  HRP: "bc" (mainnet), "bcrt" (regtest)
                           │  witness version: 0
┌──────────────────────────▼──────────────────────────────────┐
│  bc1q... address                                            │
│  The final bech32-encoded address                           │
└─────────────────────────────────────────────────────────────┘
```

### 3.2 BIP39: Mnemonic generation

A BIP39 mnemonic encodes entropy as words from a 2048-word list:

| Words | Entropy bits | Checksum bits | Total bits |
|---|---|---|---|
| 12 | 128 | 4 | 132 |
| 24 | 256 | 8 | 264 |

The checksum is the first `entropy_bits/32` bits of SHA256(entropy).

**In your code** (`wallet-bdk/src/keys.rs`):

```rust
pub fn generate_mnemonic(length: MnemonicLength) -> Result<Zeroizing<String>, BoxError> {
    let words = match length {
        MnemonicLength::Words12 => WordCount::Words12,
        MnemonicLength::Words24 => WordCount::Words24,
    };
    let generated: GeneratedKey<Mnemonic, Segwitv0> =
        Mnemonic::generate((words, Language::English))?;
    Ok(Zeroizing::new(generated.into_key().to_string()))
}
```

### 3.3 BIP32: Hierarchical Deterministic Keys

BIP32 defines how to derive child keys from a parent key. Each extended key has:
- A private key (256 bits)
- A chain code (256 bits): used as HMAC key for child derivation
- A depth (1 byte)
- A parent fingerprint (4 bytes)
- A child number (4 bytes)

**Two derivation types:**

| Type | Symbol | Can derive from public key? | Used for |
|---|---|---|---|
| Normal | `m/0` | Yes | Address keys (so a watch-only wallet can derive addresses) |
| Hardened | `m/0'` | No | Purpose, coin, account (so a leaked child key can't compromise siblings) |

**Child key derivation (CKDpriv):**

```
I = HMAC-SHA512(chain_code, data)
  where data = serP(point(k_par)) || ser32(i)   for normal
  where data = 0x00 || ser256(k_par) || ser32(i) for hardened

IL = child private key = (IL + k_par) mod n
IR = child chain code
```

### 3.4 BIP84: Native SegWit descriptors

BIP84 defines the derivation scheme for native SegWit (bech32) wallets:

```
m / purpose' / coin' / account' / change / address_index
  84'         0' or 1'  0'         0 or 1    0, 1, 2, ...
```

The descriptor for a BIP84 wallet is:

```
wpkh([fingerprint/84'/coin'/0']xpub/0/*)
```

Where:
- `wpkh(...)` = pay-to-witness-pubkey-hash (P2WPKH)
- `[fingerprint/84'/coin'/0']` = key origin info
- `xpub` = extended public key at the account level
- `/0/*` = external keychain, any index

### 3.5 What your code does step by step

In `wallet-bdk/src/keys.rs`, the `resolve()` function:

```rust
pub(crate) fn resolve(keys: &KeySource, network: Network) -> Result<Resolved, BoxError> {
    let secp = Secp256k1::new();
    let kind = NetworkKind::from(network);
    let ((external, mut keymap), (internal, internal_keys)) = match keys {
        KeySource::Mnemonic { phrase, passphrase } => {
            // 1. Parse the mnemonic
            let mnemonic = Mnemonic::parse_in(Language::English, phrase.as_str())?;
            // 2. Derive the seed (PBKDF2, 2048 rounds)
            let passphrase = passphrase.as_ref().map(|p| p.as_str()).unwrap_or("");
            let seed = Zeroizing::new(mnemonic.to_seed(passphrase));
            // 3. Create master xprv
            let xprv = Xpriv::new_master(network, seed.as_slice())?;
            // 4. Derive BIP84 descriptors
            (
                Bip84(xprv, KeychainKind::External).into_wallet_descriptor(&secp, kind)?,
                Bip84(xprv, KeychainKind::Internal).into_wallet_descriptor(&secp, kind)?,
            )
        }
        KeySource::Descriptors { external, internal } => (
            external.as_str().into_wallet_descriptor(&secp, kind)?,
            internal.as_str().into_wallet_descriptor(&secp, kind)?,
        ),
    };
    keymap.extend(internal_keys);
    Ok(Resolved { external, internal, keymap })
}
```

### 3.6 Key concepts glossary

| Concept | Explanation |
|---|---|
| **Mnemonic** | Human-readable backup. 12 words = 128 bits entropy, 24 words = 256 bits. |
| **Passphrase** | BIP39 "256th word": optional extra secret. Same mnemonic + different passphrase = completely different wallet. |
| **Derivation path** | The "address" of a key in the tree. `m/84'/0'/0'/0/0` = first receive address on mainnet. |
| **Hardened derivation** (`'`) | Child can't be derived from parent public key. Used at purpose, coin, and account levels. |
| **Gap limit** | How many unused addresses to watch. BDK defaults to 20. Your `lookahead` configures this. |
| **Descriptor** | A script expression plus the keys needed to satisfy it. `wpkh(...)` = pay-to-witness-pubkey-hash. |
| **xpub/xprv** | Extended public/private key: includes the chain code so children can be derived. |
| **P2WPKH** | Pay-to-witness-pubkey-hash, the SegWit output type. Script: `OP_0 <20-byte-hash>`. |
| **bech32** | The address encoding for SegWit. HRP "bc" (mainnet), "bcrt" (regtest), "tb" (testnet). |

---

## 4. Lecture: PSBT (Partially Signed Bitcoin Transaction)

### 4.1 What problem does PSBT solve?

Before PSBT (BIP174), there was no standard way to move a transaction between parties who each hold different keys. If Alice and Bob have a 2-of-2 multisig, who builds the transaction? How does the other party add their signature? How does a hardware wallet sign a transaction built by software?

PSBT solves this by defining a **portable container** for unsigned and partially-signed transactions. It separates the transaction lifecycle into distinct roles:

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│  Creator    │────▶│  Signer     │────▶│  Finalizer  │────▶│  Extractor  │
│  (builds)   │     │  (signs)    │     │  (combines) │     │  (extracts) │
└─────────────┘     └─────────────┘     └─────────────┘     └─────────────┘
```

- **Creator**: Builds the PSBT with inputs (including UTXO info + derivation paths) and outputs. Does not need private keys.
- **Signer**: Adds signatures (witness data) for the inputs it can sign. May be a hardware wallet, a remote signer, or the wallet itself.
- **Finalizer**: Combines all partial signatures into the final witness/scriptSig. Verifies the transaction is complete.
- **Extractor**: Produces the raw transaction bytes for broadcast.

### 4.2 PSBT data structure

A PSBT is a map of key-value pairs. The structure is:

```
PSBT Global:
  ├─ PSBT_GLOBAL_UNSIGNED_TX    : the unsigned transaction (inputs + outputs, no scripts/witnesses)
  ├─ PSBT_GLOBAL_VERSION        : PSBT version (0 for v1)
  ├─ PSBT_GLOBAL_XPUB           : extended public keys + derivation paths (for multisig)
  ├─ PSBT_GLOBAL_TX_VERSION     : transaction version
  ├─ PSBT_GLOBAL_LOCKTIME       : transaction locktime
  └─ PSBT_GLOBAL_PROPRIETARY    : proprietary key-value pairs

PSBT Input (per input):
  ├─ PSBT_IN_NON_WITNESS_UTXO   : full previous transaction (for non-SegWit inputs)
  ├─ PSBT_IN_WITNESS_UTXO       : value + scriptPubKey (for SegWit inputs)
  ├─ PSBT_IN_PARTIAL_SIG        : (pubkey → signature) pairs
  ├─ PSBT_IN_SIGHASH_TYPE       : sighash type for this input
  ├─ PSBT_IN_REDEEM_SCRIPT      : redeem script (for P2SH)
  ├─ PSBT_IN_WITNESS_SCRIPT     : witness script (for P2WSH)
  ├─ PSBT_IN_BIP32_DERIVATION   : (pubkey → fingerprint + path) pairs
  ├─ PSBT_IN_FINAL_SCRIPSIG     : final scriptSig (after finalization)
  ├─ PSBT_IN_FINAL_WITNESS      : final witness (after finalization)
  ├─ PSBT_IN_RIPEMD160          : RIPEMD160 image (for hash preimage signing)
  ├─ PSBT_IN_SHA256             : SHA256 image
  ├─ PSBT_IN_HASH160            : HASH160 image
  ├─ PSBT_IN_HASH256            : HASH256 image
  └─ PSBT_IN_PROPRIETARY        : proprietary key-value pairs

PSBT Output (per output):
  ├─ PSBT_OUT_REDEEM_SCRIPT     : redeem script
  ├─ PSBT_OUT_WITNESS_SCRIPT    : witness script
  ├─ PSBT_OUT_BIP32_DERIVATION  : (pubkey → fingerprint + path) pairs
  └─ PSBT_OUT_PROPRIETARY       : proprietary key-value pairs
```

### 4.3 The PSBT lifecycle in detail

#### Step 1: Creation

The creator builds an unsigned transaction and wraps it in a PSBT:

```rust
// In your code (wallet-bdk/src/engine.rs, build_tx):
let mut builder = self.wallet.build_tx()
    .set_recipients(outputs)           // add outputs
    .fee_rate(request.fee_rate);       // set fee rate
let psbt = builder.finish()?;          // coin selection + change → unsigned PSBT
```

At this point:
- Each input has the witness UTXO (value + scriptPubKey) and BIP32 derivation paths
- Each output has the scriptPubKey and value
- No signatures exist yet

#### Step 2: Signing

The signer adds partial signatures for the inputs it can sign:

```rust
// In your code (wallet-bdk/src/engine.rs, sign):
let signer = self.signer.as_ref().ok_or(SignError::WatchOnly)?;
let signed = psbt.sign(signer, self.wallet.secp_ctx())?;
```

At this point:
- Each input has partial signatures (pubkey → signature)
- The signatures are NOT yet combined into the final witness

#### Step 3: Finalization

The finalizer combines all partial signatures into the final witness:

```rust
// In your code (wallet-bdk/src/engine.rs, sign):
let finalized = self.wallet.finalize_psbt(psbt, SignOptions::default())?;
```

At this point:
- Each input has the final witness (stack of signatures + witness script)
- The transaction is complete and valid

#### Step 4: Extraction

The extractor produces the raw transaction:

```rust
// In your code (wallet/src/lib.rs, send):
let tx = psbt.extract_tx().map_err(|e| Error::Extract(Box::new(e)))?;
```

### 4.4 Why PSBT matters for your architecture

PSBT is the **interoperability standard**. Because your `sign()` method takes `&mut Psbt` and returns `SignOutcome`, you can:

| Use case | How PSBT enables it |
|---|---|
| Hardware wallet signing | Build PSBT in software → pass to hardware wallet → hardware wallet signs → return PSBT |
| Multisig | Multiple signers each add their partial signatures to the same PSBT |
| Watch-only inspection | A watch-only wallet can build a PSBT and inspect it without being able to sign |
| Cross-engine signing | A PSBT built by BDK can be signed by a native engine (and vice versa) |
| Air-gapped signing | Export PSBT to a USB drive → sign on an offline machine → import back |

This is why the `WalletEngine::sign` trait method uses `bitcoin::Psbt`: it's the universal transaction format.

### 4.5 Fee calculation

```
fee = sum(input values) - sum(output values)
```

The fee rate is sat/vB (satoshis per virtual byte). SegWit transactions have virtual size (vsize):

```
vsize = ceil(weight / 4)
weight = base_size * 3 + total_size
```

Where:
- `base_size` = size without witness data
- `total_size` = size with witness data

Your code uses `FeeRate::from_sat_per_vb_u32(2)` = 2 sat/vB. The fee is calculated by BDK's `TxBuilder` which:
1. Selects UTXOs (coin selection)
2. Adds a change output if needed
3. Calculates the fee based on the estimated transaction weight
4. Adjusts the change output to account for the fee

### 4.6 Sighash types

When signing, the signer must commit to which parts of the transaction are being signed. This is the sighash type:

| Type | Value | Commits to | Use case |
|---|---|---|---|
| `ALL` | 0x01 | All inputs and outputs | Standard |
| `NONE` | 0x02 | All inputs, no outputs | Blank check |
| `SINGLE` | 0x03 | All inputs, one output | Pay-to-pubkey |
| `ALL\|ANYONECANPAY` | 0x81 | One input, all outputs | CoinJoin |
| `NONE\|ANYONECANPAY` | 0x82 | One input, no outputs | Rarely used |
| `SINGLE\|ANYONECANPAY` | 0x83 | One input, one output | Rarely used |

Your code uses `SignOptions::default()` which uses `SighashType::All`: the standard choice.

---

## 5. What We Did Right

### 5.1 Architecture

**The seam structure is correct.** Engine, backend, facade and binary each have one reason to change. The compiler enforces the dependency direction: `wallet-core` has no dependencies on adapters, databases, network clients, or BDK.

**BDK is a private implementation detail.** No `bdk_*` type appears in any public API. The `convert.rs` anti-corruption layer is the right pattern: it's a single place where BDK types are translated, so if BDK changes its API, only one file needs updating.

**Block-based `BlockSource`.** No `txindex` needed, works for any engine, no address index required. The engine receives full blocks and filters what's relevant itself.

**Generic `Wallet<E: WalletEngine = BdkEngine>`.** The default makes it easy to use, the generic makes it easy to swap. This is the key design decision that makes the v2 native engine possible without breaking changes.

### 5.2 Error handling

**One enum per operation, `#[non_exhaustive]`.** Callers can match exhaustively on the cases they care about, and you can add variants without breaking them. This is the right approach for a library.

**Structured data in variants.** `InsufficientFunds { needed, available }`, `WrongNetwork { address, expected }`, `NetworkMismatch { expected, found }`: not strings. This lets callers programmatically react to errors.

**Translation in one place.** `convert.rs` for BDK, `source_error()` for RPC. The chain is never lost (`source()`). Anything without a stable meaning is kept as `source()` inside an `Engine` or `Connection` variant.

**`wallet::Error` umbrella enum.** For callers who just want `?`. The `#[from]` impls make conversion automatic.

### 5.3 Security

**BDK never sees private keys.** Only public descriptors. The keymap stays in the engine. This follows BDK 3's direction of moving key storage out of `Wallet`, and means a hardware or remote signer is a change inside the engine only.

**`Zeroizing` for secrets.** Mnemonic and passphrase are wiped on drop. The seed derived from the mnemonic is also in a `Zeroizing` buffer.

**Redacted `Debug`.** `KeySource`'s `Debug` impl prints `<redacted>`, with a test proving it. This prevents accidental secret leakage through logging.

**SQLite holds no private keys.** Only public descriptors and derived data. Loading without keys opens the wallet watch-only.

**CLI file permissions.** Mnemonic/passphrase written with mode `0600` (owner read/write only).

### 5.4 Testing

**Three layers:**

| Layer | Where | Needs a node | What it covers |
|---|---|---|---|
| Unit | `wallet-core`, `wallet-bdk` | No | Key generation, mnemonic validation, type conversions |
| Public API | `crates/wallet/tests/wallet.rs` against `MockChain` | No | BIP84 vectors, reorgs, coin selection, watch-only, persistence, lifecycle errors |
| End to end | `crates/wallet/tests/regtest.rs` against real `bitcoind` | Downloaded automatically | Receive, spend, confirm, coinbase maturity, typed node rejections, fee estimation, real reorg |

**BIP84 test vectors.** The first address matches the known vector: `bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu`.

**Reorg handling.** Both in MockChain (`reorg()` + `mine_empty()`) and real regtest (`invalidateblock` + `generate_to_address`).

**Watch-only flow.** Build but can't sign; keyed wallet can sign the watch-only wallet's PSBT. This demonstrates the PSBT interoperability.

**Every coin selection strategy.** BnB, LargestFirst, OldestFirst all tested.

**Lifecycle errors.** NotFound, AlreadyExists, NetworkMismatch, DescriptorMismatch, InvalidMnemonic: all tested with `matches!`.

### 5.5 Code quality

**Doc comments everywhere.** Every public type and method has a doc comment explaining what it does and when to use it.

**Feature flags.** `rpc` and `serde` are optional; `testkit` is dev-only. Consumers only pull in what they need.

**Workspace dependencies.** Single version of `bitcoin` across all crates. No version conflicts.

**Consistent naming.** `new_address` vs `reveal_next_address`: the distinction (next unused vs always fresh) is clear from the name.

---

## 6. What We Can Do Better

### 6.1 API design

**1. `build_tx` takes `impl IntoIterator<Item = Recipient>`.**
This is convenient but means the caller can't easily reuse a pre-built recipient list. Consider accepting `&[Recipient]` or `impl AsRef<[Recipient]>` for flexibility:

```rust
// Current
pub fn build_tx(&mut self, recipients: impl IntoIterator<Item = Recipient>, fee_rate: FeeRate) -> Result<Psbt, BuildTxError>

// More flexible
pub fn build_tx(&mut self, recipients: impl IntoIterator<Item = Recipient>, fee_rate: FeeRate) -> Result<Psbt, BuildTxError>
// Callers can do: wallet.build_tx(&recipients, fee_rate) if recipients: Vec<Recipient>
```

Actually, `impl IntoIterator` already accepts `Vec<Recipient>` by value. The issue is more that it consumes the collection. A `&[Recipient]` overload would allow reuse.

**2. `sign` takes `&self` not `&mut self`.**
This is correct (signing doesn't mutate wallet state), but it's inconsistent with `build_tx` which takes `&mut self`. The inconsistency is justified (build_tx may reveal a change address), but it should be documented why.

**3. `sync` takes `&impl BlockSource`.**
This is good for static dispatch, but means you can't easily store a `Box<dyn BlockSource>` in a struct. Consider adding a `sync_boxed` or making the trait object-safe with a blanket impl for `Box<dyn BlockSource>`.

### 6.2 Error handling

**4. `BoxError = Box<dyn Error + Send + Sync>`.**
This is fine for an MVP, but it means callers can't programmatically inspect engine/backend errors. Consider a `source()` method or a `kind()` enum for the boxed variants.

**5. `Error` umbrella enum loses specific variant data.**
Callers who want `InsufficientFunds { needed, available }` have to match on `Error::BuildTx(BuildTxError::InsufficientFunds { .. })`. This is acceptable but could be improved with a method like `Error::as_build_tx()`.

### 6.3 Engine

**6. Keymap is held outside BDK.**
This is a security win, but it means BDK's `finalize_psbt` needs the keymap passed in. The current code does this correctly, but it's a pattern that could be fragile across BDK upgrades.

**7. `public_descriptors()` is on `BdkEngine` not `WalletEngine`.**
This means watch-only creation is engine-specific. Consider adding `descriptors()` to the `WalletEngine` trait (returning `Option<(String, String)>`, with `None` for watch-only).

**8. `birthday` is not persisted by the library.**
The CLI stores it in `meta.json`, but the library should own this. Consider adding it to the engine's persisted state.

### 6.4 Backend

**9. Mempool scanning fetches every transaction.**
Fine for regtest, but mainnet would need a filtered source. Consider a `BlockSource::mempool_for(scripts: &[ScriptBuf])` method or a separate `MempoolSource` trait.

**10. `estimate_fee_rate` divides by 4.**
`per_kvb.to_sat() / 4` converts BTC/kvB to sat/kwu. This is correct but should be documented more clearly (the comment says "1 vB = 4 wu" which is right, but the math could be clearer).

### 6.5 Testing

**11. No property-based tests.**
The v2 plan mentions property tests for coin selection. Consider adding `proptest` for:
- Inputs always cover outputs + fee
- Change is never dust
- Sum of inputs >= sum of outputs + fee
- Coin selection never selects more inputs than necessary (for BnB)

**12. No fuzzing.**
PSBT parsing is a common source of vulnerabilities. Consider `cargo-fuzz` for the signing path.

**13. Regtest tests download bitcoind.**
This is convenient but makes CI slower. Consider a feature flag `regtest` that's off by default.

### 6.6 Documentation

**14. No rustdoc examples on all methods.**
Some methods have them, but not all. Every public method should have a runnable example.

**15. No architecture decision records (ADRs).**
The `docs/architecture.md` is good, but ADRs for key decisions (why BDK, why block-based, why keymap outside BDK) would help future contributors.

### 6.7 CLI

**16. Mnemonic stored unencrypted.**
Documented as a regtest-only convenience, but consider adding a `--encrypt` flag using a passphrase-derived key (e.g., AES-256-GCM with Argon2).

**17. No `wallet-cli` commands for PSBT export/import.**
Consider `export-psbt` and `sign-psbt` commands to demonstrate the PSBT flow.

---

## 7. Polar / Lightning: Do We Need It?

### 7.1 What Polar is

Polar is a tool for running a **local Lightning Network** on regtest. It spins up:
- A `bitcoind` node
- One or more Lightning nodes (LND, Core Lightning, or Eclair)
- A web UI for managing channels

### 7.2 Do we need it for this project?

**No: not for the wallet library (Project 7).**

| Aspect | Wallet Library (P7) | Lightning (P5) |
|---|---|---|
| Layer | Base layer (L1) | Lightning (L2) |
| What it does | Key management, UTXO tracking, tx building | Payment channels, HTLCs, routing |
| Chain backend | Bitcoin Core RPC | Bitcoin Core + Lightning node |
| PSBT relevance | Core to the design | Used for channel open/close |
| Polar needed? | **No** | **Yes** |

The wallet library is purely L1. It creates, signs, and broadcasts Bitcoin transactions. Lightning is a separate protocol built on top of Bitcoin.

### 7.3 When would you need Polar?

You'd need Polar if you were building:
- A Lightning wallet (Project 5)
- A Lightning node integration
- A service that opens/closes channels

### 7.4 How to connect Polar (for future reference)

If you do need Polar for a Lightning project:

```bash
# 1. Install Polar
npm install -g polar-nodes

# 2. Start a network
polar network create --bitcoin-node-count 1 --lightning-node-count 2

# 3. Get the connection details
polar network ls
```

The Polar UI runs at `http://localhost:3000`. Each Lightning node exposes:
- REST API (port varies, shown in UI)
- gRPC (port varies)
- Macaroon for auth

To connect from Rust, you'd use a Lightning client crate like `tonic` (for gRPC) or `reqwest` (for REST), with the macaroon as an auth header.

### 7.5 The one place Lightning touches your wallet library

If you later build a Lightning wallet, the **funding transaction** (opening a channel) is a regular Bitcoin transaction, and that's where your wallet library would be used:

```rust
// Open a channel = build a tx to a 2-of-2 multisig
let channel_address = lightning_node.new_funding_address();
let psbt = wallet.build_tx([Recipient::new(channel_address, channel_amount)], fee_rate)?;
wallet.sign(&mut psbt)?;
let txid = broadcaster.broadcast(&psbt.extract_tx()?)?;
// Lightning node watches for the funding tx and completes the channel open
```

So the wallet library is a **prerequisite** for a Lightning wallet, but Polar itself is not needed until you build the Lightning layer.

---

## 8. Architecture Diagrams

### 8.1 Crate dependency graph

```
┌─────────────────────────────────────────────────────────────────┐
│                         wallet-cli                              │
│  (clap binary: config, output, main)                            │
└──────────────────────────┬──────────────────────────────────────┘
                           │ depends on
                           ▼
┌─────────────────────────────────────────────────────────────────┐
│                          wallet                                 │
│  (facade: Wallet<E>, WalletBuilder, send())                     │
│  Features: rpc (default), serde                                 │
└──────┬──────────────────────┬──────────────────────┬────────────┘
       │                      │                      │
       │ depends on           │ depends on           │ depends on (feature rpc)
       ▼                      ▼                      ▼
┌──────────────┐    ┌──────────────┐    ┌──────────────────────┐
│ wallet-core  │◄───│ wallet-bdk   │    │     wallet-rpc       │
│ (types,      │    │ (BdkEngine)  │    │ (RpcClient)          │
│  traits,     │    │              │    │                      │
│  errors,     │    │              │    │                      │
│  testkit)    │    │              │    │                      │
└──────┬───────┘    └──────┬───────┘    └──────────┬───────────┘
       │                   │                       │
       │ depends on        │ depends on            │ depends on
       ▼                   ▼                       ▼
┌──────────────┐    ┌──────────────┐    ┌──────────────────────┐
│   bitcoin    │    │  bdk_wallet  │    │  bitcoincore-rpc     │
│  (0.32.x)    │    │   (3.2)      │    │     (0.19)           │
└──────────────┘    └──────────────┘    └──────────────────────┘
```

### 8.2 The seam structure

```
┌─────────────────────────────────────────────────────────────────┐
│                        Wallet<E>                                │
│  (wallet crate: public API)                                     │
└──────────────────────────┬──────────────────────────────────────┘
                           │
           ┌───────────────┼───────────────┐
           │               │               │
           ▼               ▼               ▼
    ┌─────────────┐ ┌─────────────┐ ┌─────────────┐
    │ WalletEngine│ │ BlockSource │ │ Broadcaster │
    │  (trait)    │ │  (trait)    │ │  (trait)    │
    └──────┬──────┘ └──────┬──────┘ └──────┬──────┘
           │               │               │
     ┌─────┴─────┐   ┌─────┴─────┐   ┌─────┴─────┐
     │           │   │           │   │           │
     ▼           ▼   ▼           ▼   ▼           ▼
  BdkEngine  Native  RpcClient  MockChain  RpcClient  MockChain
  (v1)       (v2)    (real)     (tests)    (real)     (tests)
```

### 8.3 Data flow: Sync

```
┌──────────┐                    ┌──────────┐                    ┌──────────┐
│  Wallet  │                    │  Engine  │                    │  Chain   │
│          │                    │          │                    │  Backend │
│  sync()  │─────sync(source)──▶│  sync()  │                    │          │
│          │                    │          │───tip()───────────▶│          │
│          │                    │          │◀──BlockId──────────│          │
│          │                    │          │                    │          │
│          │                    │          │───block_hash(h)───▶│          │
│          │                    │          │◀──BlockHash────────│          │
│          │                    │          │                    │          │
│          │                    │          │───block(hash)─────▶│          │
│          │                    │          │◀──Block────────────│          │
│          │                    │          │                    │          │
│          │                    │  apply_block_connected_to()   │          │
│          │                    │  (repeat for each block)     │          │
│          │                    │          │                    │          │
│          │                    │          │───mempool()────────▶│          │
│          │                    │          │◀──Vec<(Tx, u64)─────│          │
│          │                    │          │                    │          │
│          │                    │  apply_unconfirmed_txs()      │          │
│          │                    │  apply_evicted_txs()          │          │
│          │                    │  persist()                    │          │
│          │                    │          │                    │          │
│          │◀──SyncReport───────│          │                    │          │
└──────────┘                    └──────────┘                    └──────────┘
```

### 8.4 Data flow: Send

```
┌──────────┐                    ┌──────────┐                    ┌──────────┐
│  Caller  │                    │  Wallet  │                    │  Engine  │
│          │                    │          │                    │          │
│  send()  │──recipients+rate──▶│  send()  │                    │          │
│          │                    │          │──build_tx()────────▶│          │
│          │                    │          │                    │          │
│          │                    │          │  TxBuilder:         │          │
│          │                    │          │  - set_recipients() │          │
│          │                    │          │  - fee_rate()       │          │
│          │                    │          │  - coin selection   │          │
│          │                    │          │  - change output    │          │
│          │                    │          │  - finish()         │          │
│          │                    │          │                    │          │
│          │                    │          │◀──unsigned Psbt─────│          │
│          │                    │          │                    │          │
│          │                    │          │──sign(&mut psbt)───▶│          │
│          │                    │          │                    │          │
│          │                    │          │  Psbt::sign(keymap) │          │
│          │                    │          │  finalize_psbt()    │          │
│          │                    │          │                    │          │
│          │                    │          │◀──SignOutcome───────│          │
│          │                    │          │                    │          │
│          │                    │  psbt.extract_tx()             │          │
│          │                    │          │                    │          │
│          │                    │──broadcast(&tx)──────────────▶│  Backend │
│          │                    │          │                    │          │
│          │                    │◀──Txid─────────────────────────│          │
│          │◀──Txid─────────────│          │                    │          │
└──────────┘                    └──────────┘                    └──────────┘
```

---

## 9. Data Flow Deep-Dives

### 9.1 Address derivation flow

```
Wallet::new_address()
  │
  ▼
WalletEngine::new_address()
  │
  ▼
BdkEngine::reveal(next_unused=true)
  │
  ▼
BDK Wallet::next_unused_address(KeychainKind::External)
  │
  ▼
BDK KeychainTxOutIndex::next_unused_index()
  │  (returns the next index that hasn't received funds)
  ▼
Descriptor::at_derivation_index(index)
  │
  ▼
miniscript::Descriptor::address(network)
  │
  ▼
bech32::encode(hrp, witness_version, witness_program)
  │
  ▼
AddressInfo { address, index, keychain: External }
```

### 9.2 Coin selection flow

```
Wallet::build_tx(recipients, fee_rate)
  │
  ▼
WalletEngine::build_tx(&TxRequest)
  │
  ▼
BdkEngine::build_tx(request)
  │
  ├── Validate recipients (non-empty, correct network)
  ├── Determine strategy (request override or engine default)
  │
  ▼
BDK TxBuilder
  │
  ├── .set_recipients(outputs)
  ├── .fee_rate(request.fee_rate)
  ├── .coin_selection(strategy)
  │     │
  │     ├── BranchAndBound: tries to find a changeless solution first
  │     ├── LargestFirst: sorts UTXOs by value descending
  │     └── OldestFirst: sorts UTXOs by confirmation time
  │
  ▼
TxBuilder::finish()
  │
  ├── Select UTXOs
  ├── Calculate fee = rate * estimated_weight
  ├── Add change output (if not dust)
  └── Build unsigned PSBT
  │
  ▼
Psbt (unsigned)
```

### 9.3 Signing flow

```
Wallet::sign(&mut psbt)
  │
  ▼
WalletEngine::sign(psbt)
  │
  ▼
BdkEngine::sign(psbt)
  │
  ├── Check signer exists (not watch-only)
  ├── Psbt::sign(keymap, secp_ctx)
  │     │
  │     ├── For each input:
  │     │   ├── Look up the pubkey in the keymap
  │     │   ├── Derive the private key at the derivation path
  │     │   ├── Sign the sighash with ECDSA or Schnorr
  │     │   └── Add the partial signature to the PSBT
  │     │
  │     └── Return (partial_signatures, errors)
  │
  ├── Check if any signatures were added
  ├── Wallet::finalize_psbt(psbt, SignOptions::default())
  │     │
  │     ├── For each input:
  │     │   ├── Combine partial signatures
  │     │   ├── Build the final witness stack
  │     │   └── Verify the script
  │     │
  │     └── Return whether all inputs are finalized
  │
  ▼
SignOutcome::Finalized | Partial
```

### 9.4 Reorg handling flow

```
Wallet::sync(source)
  │
  ▼
BdkEngine::sync(source)
  │
  ├── Check genesis hash matches
  │
  ├── Walk local checkpoints down:
  │   for cp in local_tip.iter():
  │     if source.block_hash(cp.height) == cp.hash:
  │       agree = cp
  │       break
  │
  ├── If no agreement point → SyncError::WrongChain
  │
  ├── reorg_depth = from.height - agree.height
  │
  ├── Fetch and apply blocks from agree+1 to tip:
  │   for height in start..=tip.height:
  │     block = source.block(hash)
  │     wallet.apply_block_connected_to(block, height, connected_to)
  │     connected_to = BlockId { height, hash }
  │
  ├── Apply mempool transactions
  │
  ├── Evict unconfirmed txs the node no longer has
  │
  ├── Persist
  │
  ▼
SyncReport { from, to, blocks_applied, reorg_depth, mempool_txs }
```

---

## 10. Dependency Graph & Version Matrix

### 10.1 Workspace dependencies

| Crate | Dependencies |
|---|---|
| `wallet-core` | `bitcoin 0.32`, `thiserror 2`, `zeroize 1.8`, `serde 1` (optional) |
| `wallet-bdk` | `wallet-core`, `bdk_wallet 3.2`, `zeroize 1.8` |
| `wallet-rpc` | `wallet-core`, `bitcoincore-rpc 0.19` |
| `wallet` | `wallet-core`, `wallet-bdk`, `wallet-rpc` (optional) |
| `wallet-cli` | `wallet` (features: rpc, serde), `clap 4.5`, `anyhow 1`, `serde 1`, `serde_json 1` |

### 10.2 External crate versions

| Crate | Version | Purpose |
|---|---|---|
| `bitcoin` | 0.32 | Core types, keys, BIP32, PSBT, signing |
| `bdk_wallet` | 3.2 | Wallet engine (v1) |
| `bitcoincore-rpc` | 0.19 | Bitcoin Core JSON-RPC client |
| `thiserror` | 2 | Custom error enums |
| `zeroize` | 1.8 | Secret wiping |
| `serde` | 1 | Serialization |
| `serde_json` | 1 | JSON output |
| `clap` | 4.5 | CLI argument parsing |
| `anyhow` | 1 | Error handling (binary only) |
| `corepc-node` | 0.12 | bitcoind download for tests |
| `tempfile` | 3 | Temp directories for tests |

### 10.3 Feature flags

| Crate | Feature | Default | Effect |
|---|---|---|---|
| `wallet-core` | `serde` | no | Enables `Serialize`/`Deserialize` on types |
| `wallet-core` | `testkit` | no | Enables `MockChain` |
| `wallet` | `rpc` | yes | Includes `wallet-rpc` |
| `wallet` | `serde` | no | Enables `serde` on `wallet-core` types |

---

## Appendix: Key Files Reference

| File | Lines | What to look at |
|---|---|---|
| `crates/wallet-core/src/types.rs` | 177 | All domain types |
| `crates/wallet-core/src/error.rs` | 181 | All error enums |
| `crates/wallet-core/src/chain.rs` | 95 | Chain traits + blanket impls |
| `crates/wallet-core/src/engine.rs` | 51 | `WalletEngine` trait |
| `crates/wallet-core/src/keys.rs` | 87 | `KeySource` + redacted Debug |
| `crates/wallet-core/src/testkit.rs` | 205 | `MockChain` |
| `crates/wallet-bdk/src/builder.rs` | 175 | `BdkEngineBuilder` |
| `crates/wallet-bdk/src/engine.rs` | 256 | `BdkEngine`: the main impl |
| `crates/wallet-bdk/src/keys.rs` | 82 | `resolve()` + `generate_mnemonic()` |
| `crates/wallet-bdk/src/convert.rs` | 87 | Anti-corruption layer |
| `crates/wallet-bdk/src/sync.rs` | 107 | Block-based sync |
| `crates/wallet-rpc/src/lib.rs` | 153 | `RpcClient` |
| `crates/wallet/src/lib.rs` | 208 | `Wallet<E>` facade |
| `crates/wallet-cli/src/main.rs` | 292 | CLI commands |
| `crates/wallet-cli/src/config.rs` | 151 | DataDir + RpcArgs |
| `crates/wallet-cli/src/output.rs` | 189 | Output formatting |
| `crates/wallet/tests/wallet.rs` | 425 | Public API tests |
| `crates/wallet/tests/regtest.rs` | 159 | Regtest e2e tests |
| `docs/architecture.md` | 107 | Architecture doc |
| `docs/v2/README.md` | 57 | v2 native engine plan |
