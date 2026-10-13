# CLI Improvements — wallet-library vs bdk-cli

A side-by-side comparison of what wallet-cli already does, what bdk-cli does that wallet-cli does not, and a prioritised improvement plan.

---

## What wallet-cli already does well

wallet-cli is a clean, opinionated CLI with a clear UX philosophy: named wallets, a shared local regtest node, Polar support, a `--json` flag for scripting, coloured output via `palette.rs`, and a full `ratatui` TUI. It does more than bdk-cli in terms of UX polish (node lifecycle, wallet migration, `indicatif` progress, TUI). Where it falls behind is in backend flexibility and advanced PSBT/wallet operations.

---

## Ratatui: already in use — nothing to change here

`ratatui` is already in `wallet-cli/Cargo.toml` and powers the `tui` subcommand. The question of "should we use ratatui" is already answered yes. The CLI half (non-TUI) intentionally does not use ratatui — and that is correct. The CLI is for scripting, piping to `jq`, and one-shot operations. `indicatif` (also already in use) handles progress bars and spinners there. There is no benefit to switching the CLI surface to ratatui.

---

## Gap analysis: what bdk-cli does that wallet-cli does not

### Priority 1 — Foundational gaps (block everything else)

| Feature | bdk-cli approach | wallet-cli today | Notes |
|---|---|---|---|
| **Electrum backend** | `bdk_electrum` feature flag | RPC only | Needed for signet/testnet without running a full node |
| **Esplora backend** | `bdk_esplora` feature flag | RPC only | REST-based, easy for testnet/mainnet |
| **Full scan vs incremental sync** | `full_scan` and `sync` as separate commands | single `sync` command | BDK distinguishes these; a first-time wallet needs a full scan |
| **Standalone `broadcast` command** | takes a raw tx hex or base64 PSBT | only `sign-psbt --broadcast` | Needed for offline signing workflows |

### Priority 2 — PSBT and transaction building gaps

| Feature | bdk-cli approach | wallet-cli today | Notes |
|---|---|---|---|
| **`combine-psbt`** | merges multiple PSBTs into one | not present | Required for multi-party signing |
| **`finalize-psbt`** | separate from sign; finalizes with timelock options | not present | bdk-cli separates sign and finalize |
| **`lock-utxo` / `unlock-utxo` / `locked-utxos`** | persisted in wallet DB | not present | Useful for coin control |
| **`--must-spend` UTXOs in send** | `--utxos` flag on `create_tx` | not present | Forces specific inputs |
| **`--unspendable` UTXOs** | `--unspendable` flag on `create_tx` | not present | Excludes specific inputs |
| **OP_RETURN output** | `--add-string` / `--add-data` on `create_tx` | not present | Needed for L2 protocols, inscriptions |
| **`--offline-signer` flag** | adds `PSBT_GLOBAL_XPUB` and extra PSBT fields | not present | Required for hardware wallet compatibility |

### Priority 3 — Key and descriptor tooling

| Feature | bdk-cli approach | wallet-cli today | Notes |
|---|---|---|---|
| **`key generate`** | standalone xprv generation without creating a wallet | embedded in `init`/`restore` | bdk-cli separates key generation from wallet creation |
| **`key restore`** | mnemonic → xprv without writing to disk | embedded in `restore` | |
| **`key derive`** | xprv + derivation path → child key | not present | Useful for descriptor construction |
| **`descriptor` command** | generate descriptors from xprv/xpub/mnemonic | not present | Helps users construct custom descriptors |
| **Policies command** | shows miniscript spending policy tree | not present | Useful for multisig/miniscript wallets |
| **`public-descriptor`** | prints public-only descriptor (watch-only) | `descriptors` does this | Covered |

### Priority 4 — Developer and power-user features

| Feature | bdk-cli approach | wallet-cli today | Notes |
|---|---|---|---|
| **REPL mode** | interactive loop, type commands without re-loading wallet | not present | Very useful for quick manual testing |
| **Shell completions** | `bdk-cli completions bash\|zsh\|fish\|elvish\|powershell` | not present | Low effort, high dev UX |
| **Wallet config persistence** | `wallet config` saves `config.toml` so you don't repeat flags | `.env` file only | bdk-cli's config is per-wallet; wallet-cli uses env vars globally |
| **`--wallet` required flag** | every wallet command needs `-w` | `-w` optional with smart default | Different design choice; wallet-cli's default is more ergonomic |

### Priority 5 — Experimental / protocol features (not needed yet)

| Feature | bdk-cli status | wallet-cli | Notes |
|---|---|---|---|
| **Payjoin (BIP-78/v2)** | full send/receive/resume/history | not present | Complex, async; skip until core is solid |
| **BIP322 message signing** | `sign-message` / `verify-message` | not present | Nice to have |
| **Silent payments (BIP-352)** | experimental, sends only (no scanning) | not present | BDK marks this experimental |
| **BIP-353 DNS payment instructions** | `resolve-dns-recipient` | not present | Bleeding edge |
| **Compact block filters (Kyoto)** | `cbf` feature | not present | Light client option |

---

## Design decisions: where wallet-cli intentionally differs

These are **not gaps** — they are choices worth keeping or consciously reconsidering.

### 1. Opinionated node management
wallet-cli has a full node lifecycle (start/stop/status/reset), Polar support, and on-demand node startup. bdk-cli treats the node as external configuration you provide. wallet-cli's approach is better for learning and development.

### 2. Named wallets with a default
wallet-cli has a named wallet system with a `wallets use` default and smart auto-detection when only one wallet exists. bdk-cli requires `-w` on every command. wallet-cli's UX is clearly better here.

### 3. Encryption baked in
wallet-cli stores the mnemonic on disk with optional AES-GCM encryption. bdk-cli does not handle key storage — it takes descriptors as flags. wallet-cli's approach is more complete for an end-user tool.

### 4. No descriptor-as-flag design
bdk-cli's wallet is defined by passing raw descriptor strings via `-e` and `-i` flags. wallet-cli derives descriptors from a mnemonic internally. wallet-cli is more beginner-friendly; bdk-cli is more flexible for power users. A future improvement could be adding an `--ext-descriptor` / `--int-descriptor` bypass for advanced users while keeping the mnemonic default.

---

## Suggested improvement order

Based on the gaps above, here is a reasonable sequence for adding features without over-engineering:

1. `full-scan` as a distinct command (wraps the first-time BDK scan)
2. `broadcast <hex>` as a standalone command
3. Shell completions (`clap_complete` — single function call)
4. REPL mode (`shlex` + a loop — good for manual testing)
5. `lock-utxo` / `unlock-utxo` / `locked-utxos`
6. `--must-spend` and `--unspendable` on `send`
7. `combine-psbt`
8. `finalize-psbt` as a separate command
9. Electrum backend (needs `wallet-bdk` to expose it)
10. Esplora backend
11. `key` subcommand (generate / restore / derive)
12. `descriptor` command (generate from key material)
13. OP_RETURN / `--add-data` on send
14. Per-wallet config persistence
15. BIP322 message signing (nice to have)
16. Payjoin, silent payments (advanced, defer)

---

## On ratatui in the CLI surface

As noted, ratatui is already used in the TUI. For the pure CLI commands, the right tools are already in place:

- `indicatif` — progress bars and spinners during sync (already used)
- `colored` — styled terminal output (already used)
- `rpassword` — secure password prompt (already used)
- `clap` — argument parsing and `--help` output

There is no case for using ratatui widgets inside the non-TUI CLI commands. The TUI already exists for interactive use. The CLI should stay a clean, scriptable, pipe-friendly interface.

If you wanted to enrich the `--json` output for the TUI or a future web frontend, the output types in `output.rs` are the right place to evolve.
