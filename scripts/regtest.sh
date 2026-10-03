#!/usr/bin/env bash
# One-word regtest workflows for wallet_library.
#
# A thin wrapper over `wallet-cli`: everything here can also be done by hand
# (see docs/CLI.md). Wallet data lives in ./.wallet of the directory you run
# it from; the shared node runs regtest only.
#
#   ./scripts/regtest.sh start            start the shared node (stays up)
#   ./scripts/regtest.sh stop             stop it
#   ./scripts/regtest.sh status           is it running, height, users
#   ./scripts/regtest.sh reset            stop it and delete the chain (asks)
#   ./scripts/regtest.sh fund <wallet> [blocks]   mine to a wallet (default 101)
#   ./scripts/regtest.sh mine [blocks] [wallet]   mine blocks (default 1, to alice)
#   ./scripts/regtest.sh demo [payer] [payee]     two wallets (default alice,
#                                         bob): fund the payer, pay the payee, confirm
#   ./scripts/regtest.sh demo-tui [payer] [payee] print the same walkthrough
#                                         for two TUIs in two terminals
#
# Environment: WALLET_NODE_PORT (default 18543), WALLET_CLI (a prebuilt
# wallet-cli binary instead of `cargo run`).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ -n "${WALLET_CLI:-}" ]]; then
    W=("$WALLET_CLI")
else
    W=(cargo run --quiet --manifest-path "$ROOT/Cargo.toml" -p wallet-cli --)
fi
export WALLET_NETWORK=regtest

if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
    say() { printf '\n\033[1;38;2;247;147;26m▶ %s\033[0m\n' "$*"; }
else
    say() { printf '\n▶ %s\n' "$*"; }
fi

has_wallet() {
    "${W[@]}" --json wallets | grep -q "\"name\": \"$1\""
}

ensure_wallet() {
    if has_wallet "$1"; then
        echo "  $1 already exists"
    else
        "${W[@]}" wallets create "$1"
    fi
}

address_of() {
    "${W[@]}" --json -w "$1" address | sed -n 's/.*"address": "\(.*\)".*/\1/p'
}

cmd="${1:-help}"
shift || true
case "$cmd" in
    start) "${W[@]}" node start ;;
    stop) "${W[@]}" node stop ;;
    status) "${W[@]}" node status ;;
    reset) "${W[@]}" node reset "$@" ;;
    fund)
        name="${1:?usage: regtest.sh fund <wallet> [blocks]}"
        blocks="${2:-101}"
        ensure_wallet "$name"
        "${W[@]}" --node local -w "$name" mine "$blocks"
        ;;
    mine)
        blocks="${1:-1}"
        name="${2:-alice}"
        "${W[@]}" --node local -w "$name" mine "$blocks"
        ;;
    demo)
        payer="${1:-alice}"
        payee="${2:-bob}"
        say "Starting the shared regtest node"
        "${W[@]}" node start
        say "Wallets: $payer and $payee"
        ensure_wallet "$payer"
        ensure_wallet "$payee"
        say "Mining 101 blocks to $payer (the first reward becomes spendable)"
        "${W[@]}" --node local -w "$payer" mine 101
        to="$(address_of "$payee")"
        say "$payer pays $payee 250,000 sat at $to"
        "${W[@]}" --node local -w "$payer" send "$to" 250000 --fee-rate 2
        say "Mining 1 block to confirm"
        "${W[@]}" --node local -w "$payer" mine 1
        say "$payee syncs"
        "${W[@]}" --node local -w "$payee" sync
        say "Done. The node is still running: ./scripts/regtest.sh stop when finished"
        ;;
    demo-tui)
        payer="${1:-alice}"
        payee="${2:-bob}"
        cat <<EOF
Two wallets, two terminals, one shared regtest node. Run from $ROOT.

  Terminal 1:  cargo wallet -w $payer tui
  Terminal 2:  cargo wallet -w $payee tui

Each one asks to create its wallet the first time: enter, enter, enter
(new words, written down, no password).

  1. Terminal 1 ($payer): press f       101 blocks mined to $payer, coins arrive
  2. Terminal 1 ($payer): press 3, enter, then ctrl+w
                          fills in $payee's address (or paste one)
  3. Type an amount (25000, or 0.5btc), enter to review, y to broadcast
  4. Terminal 2 ($payee): r to sync     the payment shows as unconfirmed
  5. Either terminal: press m           one block, confirmed in both

One terminal instead: cargo wallet tui, name a wallet, then w and n to name
another; w switches between them.
EOF
        ;;
    help | -h | --help)
        sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
        ;;
    *)
        echo "unknown command: $cmd (try: help)" >&2
        exit 2
        ;;
esac
