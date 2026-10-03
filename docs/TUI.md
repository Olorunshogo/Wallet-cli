# TUI guide

`wallet-cli tui` is a full wallet in the terminal: balances, receive with a QR code, send with a signed review, history, coins, fee bumping, several named wallets, and a regtest playground with mining keys. For commands and configuration see [CLI.md](CLI.md).

**Contents:** [Starting it](#starting-it) · [Two wallets paying each other](#two-wallets-paying-each-other) · [Screens](#screens) · [Keys](#keys) · [Switching wallets](#switching-wallets) · [First start and unlocking](#first-start-and-unlocking) · [Node modes and offline](#node-modes-and-offline) · [Quitting](#quitting) · [Customising](#customising)

## Starting it

```bash
cargo wallet tui                        # pick a wallet, or name a new one
cargo wallet -w savings tui             # the wallet called savings (set up on first use)
cargo wallet tui --demo                 # throwaway node and wallet, nothing kept
cargo wallet --node none tui            # offline, no node
```

On regtest the TUI uses the shared local node by default: a `bitcoind` the app starts for you, so it comes up **ONLINE** with nothing installed. To use your own Bitcoin Core or [Polar](CLI.md#connecting-to-polar) instead, set `WALLET_RPC_URL` (and the cookie or user/password) in `.env`.

**Wallet names are yours to choose.** With no `-w` and no wallet yet, the TUI asks for a name first, then runs the setup screen. Any name works (`a-z`, `0-9`, `-`, `_`; capitals are lowered). `alice` and `bob` below are only examples.

## Two wallets paying each other

**In one terminal:**

1. `cargo wallet tui`, type a name (say `alice`), `enter`, then `enter` three times: new words, written down, no password.
2. Press `f`: 101 blocks are mined to her and she has coins.
3. Press `w`, then `n`, type a second name (say `bob`), `enter`, and set it up the same way.
4. Press `w`, pick `alice`, `enter`.
5. Press `3` (Send), `enter` to fill in, then `ctrl+w`: bob's address is filled in for you (press it again to cycle through your other wallets). Type an amount (`25000` or `0.5btc`), `enter` to review, `y` to send.
6. Press `m` to mine a block and confirm it, then `w` to switch to `bob` and see it arrive.

**In two terminals,** both on the same shared node and so on one chain:

```bash
cargo wallet -w alice tui               # terminal 1
cargo wallet -w bob tui                 # terminal 2
```

1. Set up each wallet when asked.
2. In alice's TUI press `f` for coins.
3. In alice's TUI press `3`, `enter`, `ctrl+w` (or paste the address from bob's Receive screen, `2`), an amount, `enter`, `y`.
4. Bob's TUI picks the payment up on its next auto-sync (or press `r`): unconfirmed.
5. Press `m` in either TUI to mine a block: it confirms in both.

`./scripts/regtest.sh demo-tui <name> <name>` prints these steps with your own names.

## Screens

Switch with `1` to `5` or `tab`.

| Screen | What it does |
|---|---|
| **1 Dashboard** | animated balances (confirmed, unconfirmed, immature), wallet and node details, recent activity |
| **2 Receive** | the current address and its QR code; `n` for a new one |
| **3 Send** | a validated form: address checked against the network (`ctrl+w` fills in one of your other wallets), amount capped at your balance (sats, or `0.001 btc`), fee rate within bounds (`ctrl+e` to estimate), coin selection (`←→`). The payment is built and signed first, then shown with its real fee and size; `y` sends, `n` cancels |
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
| `f` | mine 101 blocks to this wallet (faucet) | any regtest node (local, demo, your own, Polar) |
| `m` | mine one block (confirms pending payments) | any regtest node (local, demo, your own, Polar) |
| `o` | send payments saved while offline | online with saved payments |
| `c` | copy (recovery words on the setup screen, with a yes/no confirmation; the address on Receive) | setup screen, Receive |
| `L` | use the local node instead for this session | Polar mode, while it is unreachable |
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
- `n` asks for a name (any name you like) and runs the setup screen to create it.
- `●` marks the open wallet; balances are as of each wallet's last sync.

Switching needs named wallets: it is not available with `--datadir` or `--demo`.

## First start and unlocking

- **No wallet named, none yet:** the wallet picker asks for a name first; `esc` quits. With wallets already there it lists them: `enter` opens one, `n` names a new one.
- **No wallet yet:** a setup screen offers *create* (shows fresh recovery words; `←→` picks 12, 15, 18, 21 or 24 words, regenerating them; `g` for new ones at the same length; `c` to copy them, after confirming) or *restore* (validated words, optional passphrase and birthday height), then an optional password. A password is required on mainnet. The word count defaults to `WALLET_WORDS` (12 if unset).
- **Encrypted wallet:** asks for the password; `esc` opens it watch-only instead (view everything, sign nothing).
- **`--demo`:** skips setup and shows the new recovery words in a welcome dialog; `c` copies them the same way.

### Copying recovery words

`c` always asks first: "Anyone who sees your clipboard can take the coins." `y` copies, anything else cancels. Once copied:

- The clipboard is cleared automatically after `clipboard_clear_secs` in `tui.toml` (default 60 s), and immediately on quit.
- It uses the desktop clipboard where one is available (X11, Wayland, macOS, Windows); over SSH or in a terminal without one, it falls back to the terminal's own clipboard escape code (OSC 52) and says so. Some terminals do not support OSC 52, in which case write the words down instead.

## Node modes and offline

| Mode | Behaviour |
|---|---|
| `--node local` | the default on regtest. The shared local regtest node, started on demand and shared with other wallets and terminals. Mining keys work. |
| `--node polar` | [Polar](https://lightningpolar.com)'s regtest node, Docker-based, with its own wallets kept apart in `.wallet/polar` (Polar is a different chain from the local node). If it cannot be reached the wallet opens **OFFLINE**, says why (the URL it tried and how to fix it), and retries every few seconds. Press `L` to use the local node and its wallets instead for this session; `ctrl+c` and restart to go back to Polar. Mining keys work on Polar too. |
| `--node external` | your own Bitcoin Core (the default on networks other than regtest, or once you set `WALLET_RPC_*` on regtest without choosing `polar`). If it cannot be reached the wallet opens **OFFLINE**, says why, and retries every few seconds. On regtest, mining keys work here too. |
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
mnemonic_words = 24       # 12, 15, 18, 21 or 24 for new wallets (also WALLET_WORDS, --words)
clipboard_clear_secs = 60 # seconds before copied recovery words are cleared
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
