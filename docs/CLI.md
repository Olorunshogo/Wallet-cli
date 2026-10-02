# CLI guide

Everything `wallet-cli` can do, how to run it, and how the pieces fit: named wallets, the shared regtest node, the `regtest.sh` script, configuration and scripting. For the interactive interface see [TUI.md](TUI.md).

**Contents:** [Running it](#running-it) · [Your first two wallets](#your-first-two-wallets) · [Where chain data comes from](#where-chain-data-comes-from) · [The shared local node](#the-shared-local-node) · [Named wallets](#named-wallets) · [Command reference](#command-reference) · [The regtest script](#the-regtest-script) · [Configuration](#configuration) · [Output and scripting](#output-and-scripting) · [Data on disk](#data-on-disk) · [Troubleshooting](#troubleshooting)

## Running it

The program is `wallet-cli`. It is built into `target/`, not installed, so pick one way to run it:

| From | Command |
|---|---|
| The repo, any wallet | `cargo wallet <command>` |
| The repo, Alice or Bob | `cargo alice <command>`, `cargo bob <command>` |
| An existing build | `./target/debug/wallet-cli <command>` |
| Anywhere | `cargo install --path crates/wallet-cli`, then `wallet-cli <command>` |

The aliases live in [`.cargo/config.toml`](../.cargo/config.toml): `cargo wallet` is `cargo run --quiet -p wallet-cli --`, and `cargo alice` adds `--wallet alice`. Everything after the alias goes to the app. Cargo's own flags go before it: `cargo --offline wallet init`. Do not repeat the program name (`cargo run wallet-cli init` fails).

This guide writes commands as `wallet-cli …`; read them as `cargo wallet …` inside the repo.

## Your first two wallets

Alice and Bob on one private regtest network, with nothing to install:

```bash
cp .env.example .env                       # then set WALLET_NODE=local in it (or add --node local below)

cargo alice init                           # Alice's wallet, fresh recovery words
cargo bob init                             # Bob's wallet
cargo alice --node local mine 101          # 101 blocks to Alice: her first reward becomes spendable
cargo bob address                          # copy Bob's bcrt1… address
cargo alice --node local send <bob's address> 250000
cargo bob --node local sync                # Bob sees it, unconfirmed
cargo alice --node local mine 1            # confirm it
cargo bob --node local history             # Bob: +250,000 sat, confirmed
```

Or in one go: `./scripts/regtest.sh demo` (see [The regtest script](#the-regtest-script)).

Why 101 blocks: a mining reward can only be spent once it has 100 confirmations, so mining 101 makes the first one spendable.

## Where chain data comes from

`--node` (or `WALLET_NODE`) applies to every command and the TUI:

| Mode | What it uses | Needs |
|---|---|---|
| `external` (default) | a Bitcoin Core you run, reached with `--rpc-url`, `--rpc-cookie` or `--rpc-user/--rpc-pass` | a running `bitcoind` |
| `local` | the [shared local node](#the-shared-local-node): a regtest `bitcoind` this app runs for all your wallets | nothing (regtest only) |
| `none` | no node; commands that need one say so | nothing |

Commands that work without a node: `init`, `restore`, `address`, `balance`, `history`, `utxos`, `status`, `descriptors`, `wallets`, and building or signing PSBTs offline. Commands that need one: `sync`, `send`, `bump-fee`, `mine`, `export-psbt`, `sign-psbt --broadcast`.

**Your own Bitcoin Core:** start it with `bitcoind -regtest -daemon` (or `-signet`, `-testnet4`). The wallet finds it on the network's default port with the cookie in `~/.bitcoin/<network>/.cookie`. No Bitcoin Core installed? The build already downloaded one: `$(find target -path '*bitcoin-29.0/bin/bitcoind' | head -1) -regtest -daemon`.

## The shared local node

`--node local` uses one regtest `bitcoind` for **all** your wallets, so they are on the same chain and can pay each other.

```bash
wallet-cli node start      # start it in the background; it stays up until `node stop`
wallet-cli node status     # running? height, port, how many programs use it
wallet-cli node stop       # stop it (the chain is kept)
wallet-cli node reset      # stop it and delete the chain (asks; wallets are kept)
```

- **On demand:** any `--node local` command or TUI starts it if it is not running, and joins it if it is.
- **Stops by itself:** a node started on demand is stopped by the last program using it. Every user holds a lease (a file in `node/leases/`), so a node shared by two TUIs stays up until both have quit (`q` included).
- **Pinned:** a node started with `node start` (or `regtest.sh start`) keeps running until `node stop`, which makes repeated commands faster.
- **Where:** data in `.wallet/regtest/node` (`--node-dir`, `WALLET_NODE_DIR`), RPC on port 18543 and P2P on 18544 (`--node-port`, `WALLET_NODE_PORT`). The port avoids a `bitcoind` you may run yourself on 18443.
- **Regtest only.** It never runs signet, testnet or mainnet.

After `node reset`, wallets still remember the old chain; a `sync` treats it like a reorg back to an empty chain, and their coins disappear until you mine again.

## Named wallets

Several wallets live side by side, picked by name:

```bash
wallet-cli wallets create alice                 # fresh recovery words (12 by default)
wallet-cli wallets create bob --words 24 --encrypt
wallet-cli wallets                              # list: name, balance at last sync, 🔒, default
wallet-cli wallets use alice                    # the default when no wallet is named
wallet-cli wallets rename bob carol
wallet-cli wallets remove carol                 # asks you to type the name
```

- **Pick one per command:** `-w <name>` / `--wallet <name>` / `WALLET_NAME`. `wallet-cli -w bob address`.
- **Default:** the one set with `wallets use`; if you have exactly one wallet, that one; otherwise name it.
- **`init` and `restore`** create the named wallet (`-w bob init`), or `default` if none is named.
- **Names:** 1 to 32 characters, `a-z`, `0-9`, `-`, `_`.
- **Removing** deletes the wallet's files; its recovery words are the only way back. It refuses a wallet that held coins at its last sync unless you pass `--force`, and asks you to type the name unless you pass `--yes`.
- **`--datadir <path>`** opens the wallet in exactly that directory instead (named wallets do not apply).
- **Upgrading:** a wallet from before named wallets (files directly in `.wallet/regtest`) is moved to `wallets/default` automatically the first time, with a note.

## Command reference

Global options go anywhere on the line.

| Option | Env | Default | Meaning |
|---|---|---|---|
| `-n, --network` | `WALLET_NETWORK` | `regtest` | `regtest`, `signet`, `testnet`, `testnet4`, `bitcoin` (also `mainnet`, `main`, `test`) |
| `-w, --wallet` | `WALLET_NAME` | the default wallet | which named wallet |
| `--datadir` | `WALLET_DATADIR` | | a wallet directory by path instead |
| `--node` | `WALLET_NODE` | `external` | `external`, `local`, `none` |
| `--node-dir` | `WALLET_NODE_DIR` | `.wallet/regtest/node` | local node data |
| `--node-port` | `WALLET_NODE_PORT` | `18543` | local node RPC port |
| `--rpc-url` | `WALLET_RPC_URL` | localhost, network port | your Bitcoin Core |
| `--rpc-cookie` | `WALLET_RPC_COOKIE` | `~/.bitcoin/<network>/.cookie` | its cookie file |
| `--rpc-user`, `--rpc-pass` | `WALLET_RPC_USER`, `WALLET_RPC_PASS` | | instead of the cookie |
| `--json` | | off | machine-readable output |
| `--encrypt` | `WALLET_ENCRYPT` | off | password-protect recovery words on `init`, `restore`, `wallets create` |
| `--allow-mainnet` | `WALLET_ALLOW_MAINNET` | off | required for `--network bitcoin` |

### Wallets

| Command | Does |
|---|---|
| `init [--words 12\|15\|18\|21\|24]` | create a wallet with fresh recovery words |
| `restore --mnemonic "…" [--passphrase …] [--birthday N]` | rebuild a wallet from its words; `--birthday` skips older blocks on the first sync |
| `wallets [list]` | list wallets |
| `wallets create <name> [--words N]` | same as `-w <name> init` |
| `wallets use <name>` | set the default |
| `wallets rename <old> <new>` | rename |
| `wallets remove <name> [--yes] [--force]` | delete |
| `descriptors` | public descriptors, for a watch-only copy elsewhere |

### Receiving and balances

| Command | Does |
|---|---|
| `address [--new]` | the next unused receive address (`--new`: always a fresh one) |
| `sync` | read new blocks and the mempool (progress bar) |
| `balance` | confirmed, unconfirmed, immature and total |
| `history [--page N] [--per-page N] [--all]` | transactions, newest first, 20 per page |
| `utxos [--page N] [--per-page N] [--all]` | coins, 20 per page |
| `status <txid>` | unconfirmed, or confirmed with block and confirmations |

### Sending

| Command | Does |
|---|---|
| `send <address> <sats> [--fee-rate N] [--target BLOCKS] [--selection bnb\|largest\|oldest] [--dry-run]` | build, sign and broadcast; the fee is estimated when `--fee-rate` is left out (1 sat/vB when the node has no data yet) |
| `bump-fee <txid> --fee-rate N` | replace an unconfirmed payment with a higher-fee version (RBF) |
| `export-psbt <address> <sats> [--fee-rate N] [--output FILE]` | an unsigned PSBT for an offline or hardware signer |
| `sign-psbt [--input FILE] [--broadcast]` | sign a PSBT; broadcast it or print the hex |

### Regtest

| Command | Does |
|---|---|
| `mine [BLOCKS] [--to ADDRESS]` | mine blocks to this wallet (or `--to`), then sync; works with `--node local` or your own regtest node |
| `node start\|stop\|status\|reset` | control the shared local node |

### Interfaces

| Command | Does |
|---|---|
| `tui [--demo] [--tui-config FILE]` | the terminal interface, see [TUI.md](TUI.md) |

## The regtest script

[`scripts/regtest.sh`](../scripts/regtest.sh) wraps common workflows in one word. Everything it does is a `wallet-cli` command you could type yourself.

| Command | Does |
|---|---|
| `./scripts/regtest.sh start` | start the shared node (pinned) |
| `./scripts/regtest.sh stop` | stop it |
| `./scripts/regtest.sh status` | running? height, users |
| `./scripts/regtest.sh reset` | stop it and delete the chain (asks) |
| `./scripts/regtest.sh fund <wallet> [blocks]` | create the wallet if needed and mine to it (default 101) |
| `./scripts/regtest.sh mine [blocks] [wallet]` | mine blocks (default 1, to alice) to confirm payments |
| `./scripts/regtest.sh demo` | start the node, create alice and bob, fund alice, pay bob 250,000 sat, confirm, show bob's balance |
| `./scripts/regtest.sh help` | this list |

Wallet data goes in `./.wallet` of the directory you run it from. `WALLET_NODE_PORT` picks the port; `WALLET_CLI` points at a prebuilt `wallet-cli` instead of `cargo run`.

## Configuration

Settings come from, in order of priority: command-line flags, then environment variables, then a `.env` file in the directory you run from. Start from the template:

```bash
cp .env.example .env
```

A good `.env` for regtest work:

```bash
WALLET_NETWORK=regtest
WALLET_NODE=local          # the shared node, started on demand
```

[`.env.example`](../.env.example) documents every variable: network, wallet name and directory, node mode, local node directory and port, RPC URL and auth, logging, colors, and the test node download.

## Output and scripting

- **Text** output uses colored badges (`✔ SENT`, `REGTEST`, `✖ ERROR` plus a hint on what to do next). Waiting on the node shows a spinner or a progress bar on stderr. All colors come from [`palette.rs`](../crates/wallet-cli/src/palette.rs).
- **Plain text** automatically when output goes to a pipe or a file, or with `NO_COLOR=1`.
- **`--json`** prints machine-readable output and no colors or spinners. Lists (`history`, `utxos`) come as `{ "page", "pages", "per_page", "total", "items": [...] }`; use `--all` for everything. `wallets` returns `{ "root", "wallets": [...] }`.
- **Errors** exit with code 1; with `--json` they are `{ "error": [message, causes…] }` on stderr.

```bash
wallet-cli --json -w alice balance | jq .confirmed
wallet-cli --json history --all | jq '.items[] | select(.sent > 0) | .txid'
```

## Data on disk

```text
.wallet/
  regtest/                      one folder per network
    wallets/
      alice/
        wallet.sqlite           addresses, transactions, sync position (no private keys)
        mnemonic, passphrase    recovery words (owner-only), or mnemonic.enc when encrypted
        meta.json               birthday height, balance at the last sync
        outbox/                 payments signed offline in the TUI (sent/, failed/)
        tui.toml, tui.log       TUI settings and logs
      bob/
    default                     name of the default wallet
    node/                       the shared local node (chain, node.json, leases/)
```

Plain-text recovery words are fine on regtest and signet. Use `--encrypt` for anything else; mainnet requires it.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `wallet-cli: command not found` | Not installed. Use `cargo wallet …` in the repo, `./target/debug/wallet-cli`, or `cargo install --path crates/wallet-cli`. |
| `unrecognized subcommand 'wallet-cli'` | The program name was passed as an argument. Use `cargo wallet init`, not `cargo run wallet-cli init`. |
| `unexpected argument '--offline'` | A Cargo flag; put it before the alias: `cargo --offline wallet init`. |
| `no wallet yet` | Create one: `init`, or `wallets create <name>`. |
| `no wallet named "default"; you have: alice, bob` | Name one with `-w alice`, or `wallets use alice`. |
| `a wallet already exists` | `init` never overwrites keys. Use another name (`-w bob init`). |
| `could not connect` | No node at the RPC URL. Start `bitcoind`, check `--rpc-url`/`--rpc-cookie`, or use `--node local`. |
| `the local node did not start on port 18543` | The port is taken. Pick another: `WALLET_NODE_PORT=18643`. The error shows the end of the node's log. |
| Alice pays Bob but Bob never sees it | They are on different chains. Use `--node local` for both (one shared node), or the same `--rpc-url`. |
| `insufficient funds` on regtest | Mine first: `-w <name> --node local mine 101`. |
| A node keeps running after I'm done | It was started with `node start` (pinned). `wallet-cli node stop` or `./scripts/regtest.sh stop`. |
| `refusing without confirmation; pass --yes` | `wallets remove` and `node reset` ask you to type a confirmation; in scripts pass `--yes`. |
| `mainnet is disabled by default` | Add `--allow-mainnet`, and `--encrypt` when creating. |
| Plain text wanted, or colors look wrong | `NO_COLOR=1`; colors live in `palette.rs`. |
| More detail | `RUST_LOG=debug`. |
