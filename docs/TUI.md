# TUI guide

`wallet-cli tui` is a full wallet in the terminal: balances, receive with a QR code, send with a signed review, history, coins, fee bumping, several named wallets, and a regtest playground with mining keys. For commands and configuration see [CLI.md](CLI.md).

**Contents:** [Starting it](#starting-it) · [Two wallets in two terminals](#two-wallets-in-two-terminals) · [Screens](#screens) · [Keys](#keys) · [Switching wallets](#switching-wallets) · [First start and unlocking](#first-start-and-unlocking) · [Node modes and offline](#node-modes-and-offline) · [Quitting](#quitting) · [Customising](#customising)

## Starting it

```bash
cargo wallet tui --demo                 # throwaway node and wallet, nothing kept
cargo wallet --node local tui           # the default wallet on the shared local node
cargo alice --node local tui            # Alice's wallet
cargo wallet -w bob tui                 # Bob's, on your own Bitcoin Core (--node external)
cargo wallet --node none tui            # offline, no node
```

With `WALLET_NODE=local` in your `.env`, `--node local` can be left out.

## Two wallets in two terminals

```bash
cargo alice --node local tui            # terminal 1
cargo bob --node local tui              # terminal 2
```

Both join the same shared node, so they are on one chain.

1. In Alice's TUI press `f`: 101 blocks are mined to her and she has coins.
2. In Bob's TUI press `2` (Receive) and copy his address.
3. In Alice's TUI press `3` (Send), `enter` to fill in, paste the address, `tab`, an amount, `enter` to review, `y` to send.
4. Bob's TUI picks the payment up on its next auto-sync (or press `r`): unconfirmed.
5. Press `m` in either TUI to mine a block: it confirms in both.

## Screens

Switch with `1` to `5` or `tab`.

| Screen | What it does |
|---|---|
| **1 Dashboard** | animated balances (confirmed, unconfirmed, immature), wallet and node details, recent activity |
| **2 Receive** | the current address and its QR code; `n` for a new one |
| **3 Send** | a validated form: address checked against the network, amount capped at your balance (sats, or `0.001 btc`), fee rate within bounds (`ctrl+e` to estimate), coin selection (`←→`). The payment is built and signed first, then shown with its real fee and size; `y` sends, `n` cancels |
| **4 History** | every transaction, paginated to fit the screen; `enter` for details, `b` to bump the fee |
| **5 UTXOs** | your coins, paginated |

Loading states: skeleton placeholders until the first sync, a spinner and progress bar while syncing, notifications that slide in at the bottom right, and new transactions briefly highlighted. The header shows the network, the wallet name, the tip height, sync status and **ONLINE / OFFLINE / NO NODE**.

## Keys

| Key | Does | When |
|---|---|---|
| `1`-`5`, `tab`, `shift+tab` | switch screens | always |
| `r` | sync now (retry the node when offline) | always |
| `w` | open the wallet switcher | named wallets |
| `?` | every key for the current screen | always |
| `f` | mine 101 blocks to this wallet (faucet) | `--node local` or `--demo` |
| `m` | mine one block (confirms pending payments) | `--node local` or `--demo` |
| `o` | send payments saved while offline | online with saved payments |
| `q` | quit | not while typing |
| `ctrl+c` | quit | always |
| `↑↓` | select | lists |
| `←→`, `PgUp/PgDn`, `Home/End` | change page, first/last | History, UTXOs |
| `enter`, `esc` | open / confirm, back / close | dialogs and forms |

Typing in a form never triggers single-letter keys; press `esc` to leave the form first.

## Switching wallets

Press `w`:

```text
╔══ Wallets ════════════════════════════════════════════╗
║ › ● alice           5099.99750000 BTC       default   ║
║     bob                0.00250000 BTC   🔒             ║
║                                                        ║
║  enter open   n new wallet   esc close                 ║
╚════════════════════════════════════════════════════════╝
```

- `↑↓` and `enter` open another wallet. The current one is closed, the new one is opened (after its password, if encrypted), and the node connection stays.
- `n` asks for a name and runs the setup screen to create it.
- `●` marks the open wallet; balances are as of each wallet's last sync.

Switching needs named wallets: it is not available with `--datadir` or `--demo`.

## First start and unlocking

- **No wallet yet:** a setup screen offers *create* (shows fresh recovery words, 12 to 24 as configured; `g` for new ones) or *restore* (validated words, optional passphrase and birthday height), then an optional password. A password is required on mainnet.
- **Encrypted wallet:** asks for the password; `esc` opens it watch-only instead (view everything, sign nothing).
- **`--demo`:** skips setup and shows the new recovery words in a welcome dialog.

## Node modes and offline

| Mode | Behaviour |
|---|---|
| `--node external` | your Bitcoin Core. If it cannot be reached the wallet opens **OFFLINE** and retries every few seconds. |
| `--node local` | the shared local regtest node, started on demand and shared with other wallets and terminals. Mining keys work. |
| `--node none` | offline on purpose; never connects. |
| `--demo` | a throwaway node and wallet. |

**Offline** you can open and unlock the wallet, see balances and history as of the last sync, get receive addresses, and build and sign payments. Confirming a payment offline saves it to the wallet's `outbox/` and counts it right away (its coins are reserved, its change shows as unconfirmed). When the node is back, saved payments are **broadcast automatically, oldest first**; `o` retries by hand.

## Quitting

`q` (or `ctrl+c`) stops everything the TUI started:

- its background worker,
- a local node it started on demand, unless another program (another TUI, a CLI command) is still using it; the last one out stops it,
- the throwaway node and wallet of `--demo`.

A node started with `wallet-cli node start` or `./scripts/regtest.sh start` is pinned and keeps running until you stop it.

## Customising

Settings live in `<wallet directory>/tui.toml` (or `--tui-config FILE`). Every key is optional; typos are rejected with a clear error.

```toml
refresh_secs = 5          # auto-sync interval, 0 = off
reconnect_secs = 5        # retry interval while offline
fallback_fee_rate = 2     # sat/vB when there is no estimate
max_fee_rate = 1000       # typo guard for the fee field
mnemonic_words = 24       # 12, 15, 18, 21 or 24 for new wallets
fps = 30                  # redraws per second
toast_secs = 4            # how long notifications stay
tween_ms = 700            # balance count-up duration
recent_txs = 6            # dashboard activity rows

[theme]                   # hex ("#f7931a") or names ("cyan")
background = "#0a0e14"
accent = "#f7931a"        # active tab, titles, focused borders
border = "#22b8cf"
```

Default colors come from [`palette.rs`](../crates/wallet-cli/src/palette.rs), shared with the CLI's printed output. Theme keys: `background`, `accent`, `text`, `muted`, `border`, `border_focus`, `selection`, `success`, `warning`, `error`, `info`, `confirmed`, `unconfirmed`, `immature`, `skeleton`, `skeleton_shine`, `mainnet`, `testnet`, `signet`, `regtest`.

Logs go to `<wallet directory>/tui.log` (never to the screen); `RUST_LOG=debug` for more.
