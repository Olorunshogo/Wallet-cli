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
#   ./scripts/regtest.sh demo             alice + bob, fund alice, pay bob, confirm
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
        say "Starting the shared regtest node"
        "${W[@]}" node start
        say "Wallets: alice and bob"
        ensure_wallet alice
        ensure_wallet bob
        say "Mining 101 blocks to alice (the first reward becomes spendable)"
        "${W[@]}" --node local -w alice mine 101
        bob="$(address_of bob)"
        say "alice pays bob 250,000 sat at $bob"
        "${W[@]}" --node local -w alice send "$bob" 250000 --fee-rate 2
        say "Mining 1 block to confirm"
        "${W[@]}" --node local -w alice mine 1
        say "bob syncs"
        "${W[@]}" --node local -w bob sync
        say "Done. The node is still running: ./scripts/regtest.sh stop when finished"
        ;;
    help | -h | --help)
        sed -n '2,19p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
        ;;
    *)
        echo "unknown command: $cmd (try: help)" >&2
        exit 2
        ;;
esac
