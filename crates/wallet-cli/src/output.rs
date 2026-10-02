//! Human-readable and JSON output. Every command prints through here so the
//! two formats stay in sync.
//!
//! Text output is colored with badges from the shared palette (see
//! `style.rs`); JSON output is plain and unchanged by colors or spinners.

use colored::Colorize;
use serde_json::{Value, json};
use wallet::bitcoin::{Address, Amount, FeeRate, Network, Transaction, Txid, consensus};
use wallet::{AddressInfo, Balance, Keychain, SyncReport, TxDetails, TxStatus, Utxo};

use crate::config::DataDir;
use crate::pager::Page;
use crate::palette::PALETTE;
use crate::style::{self, amount, badge, label, row, rule, title, value};
use crate::tui::format::{btc, net, paid_fee, sats, signed_sats, thousands};

/// Prints command results as colored text or JSON.
pub struct Output {
    json: bool,
}

impl Output {
    /// Text unless `json`.
    pub fn new(json: bool) -> Self {
        Self { json }
    }

    fn emit(&self, value: Value, text: impl FnOnce() -> String) {
        if self.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&value).expect("valid json")
            );
        } else {
            println!("{}", text());
        }
    }

    // === Messages

    /// A side note on stderr, e.g. a fee fallback.
    pub fn note(&self, msg: &str) {
        if !self.json {
            eprintln!("{} {msg}", badge("NOTE", PALETTE.info));
        }
    }

    /// A failure, its causes and, when we know one, what to do about it.
    pub fn error(&self, err: &anyhow::Error) {
        if self.json {
            let chain: Vec<String> = err.chain().map(|e| e.to_string()).collect();
            eprintln!("{}", json!({ "error": chain }));
            return;
        }
        eprintln!(
            "{} {}",
            badge("✖ ERROR", PALETTE.error),
            style::error(&err.to_string()).bold()
        );
        for cause in err.chain().skip(1) {
            eprintln!("  {} {cause}", label("caused by"));
        }
        if let Some(hint) = hint(err) {
            eprintln!("  {} {hint}", style::info("→"));
        }
    }

    // === Wallet lifecycle

    /// After `init` / `restore`.
    pub fn created(
        &self,
        dir: &DataDir,
        network: Network,
        mnemonic: Option<&str>,
        encrypted: bool,
    ) {
        let path = dir.path().display().to_string();
        self.emit(json!({ "datadir": path, "mnemonic": mnemonic }), || {
            let what = if mnemonic.is_some() { "✔ WALLET CREATED" } else { "✔ WALLET RESTORED" };
            let mut s = format!(
                "{} {}\n\n{}",
                badge(what, PALETTE.success),
                network_badge(network),
                row("Data directory", 14, value(&path))
            );
            if let Some(words) = mnemonic {
                s.push_str(&format!(
                    "\n\n  {}  {}\n{}",
                    title("Recovery words"),
                    label("(write them down in order; they are the only backup)"),
                    word_grid(words)
                ));
            }
            let storage = if encrypted {
                format!("\n\n{} {}", badge("🔒 ENCRYPTED", PALETTE.success), label("The words are stored encrypted with your password."))
            } else {
                format!(
                    "\n\n{} {}",
                    badge("⚠ WARNING", PALETTE.warning),
                    style::warning("The words are also stored unencrypted in the data directory. Fine for regtest and signet, not for real funds.")
                )
            };
            s.push_str(&storage);
            s
        });
    }

    /// `descriptors`.
    pub fn descriptors(&self, external: &str, internal: &str) {
        self.emit(
            json!({ "external": external, "internal": internal }),
            || {
                format!(
                    "{} {}\n\n{}\n{}",
                    badge("DESCRIPTORS", PALETTE.brand),
                    label("public only; safe to share for a watch-only copy"),
                    row("Receive", 8, value(external)),
                    row("Change", 8, value(internal)),
                )
            },
        );
    }

    /// A short confirmation, e.g. "alice is now the default wallet".
    pub fn done(&self, message: &str) {
        self.emit(json!({ "ok": true, "message": message }), || {
            format!("{} {}", badge("✔ DONE", PALETTE.success), value(message))
        });
    }

    // === Named wallets and the local node

    /// `wallets`.
    pub fn wallets(&self, list: &[crate::wallets::Entry], root: &std::path::Path) {
        let items: Vec<Value> = list
            .iter()
            .map(|w| {
                json!({
                    "name": w.name,
                    "default": w.is_default,
                    "encrypted": w.encrypted,
                    "last_balance_sat": w.last_balance_sat,
                })
            })
            .collect();
        self.emit(
            json!({ "root": root.display().to_string(), "wallets": items }),
            || {
                let mut s = format!(
                    "{} {}\n{}",
                    badge("WALLETS", PALETTE.brand),
                    label(&format!("in {}", root.display())),
                    rule(true)
                );
                if list.is_empty() {
                    s.push_str(&format!(
                        "\n  {}\n  {}",
                        label("No wallets yet."),
                        value("Create one: wallets create alice")
                    ));
                }
                for w in list {
                    let marker = if w.is_default {
                        style::success("●")
                    } else {
                        label(" ")
                    };
                    let balance = w
                        .last_balance_sat
                        .map(|b| btc(Amount::from_sat(b)))
                        .unwrap_or_else(|| "not synced".into());
                    let lock = if w.encrypted { "🔒" } else { "  " };
                    s.push_str(&format!(
                        "\n  {marker} {}  {}  {lock}{}",
                        title(&format!("{:<16}", w.name)),
                        amount(&format!("{balance:>18}")),
                        if w.is_default {
                            label("  default")
                        } else {
                            label("")
                        }
                    ));
                }
                s.push_str(&format!(
                    "\n{}\n  {}",
                    rule(true),
                    label("use one: -w <name> · make default: wallets use <name>")
                ));
                s
            },
        );
    }

    /// `node start`.
    pub fn node_started(&self, started: bool, shared: &crate::node::Shared) {
        let port = shared.port();
        self.emit(
            json!({ "started": started, "port": port, "dir": shared.dir().display().to_string() }),
            || {
                let headline = if started {
                    badge("✔ NODE STARTED", PALETTE.success)
                } else {
                    badge("● NODE RUNNING", PALETTE.info)
                };
                format!(
                    "{headline} {}\n\n{}\n{}",
                    label("regtest; stays up until `node stop`"),
                    row(
                        "RPC port",
                        8,
                        value(&port.map(|p| p.to_string()).unwrap_or_default())
                    ),
                    row("Data", 8, value(&shared.dir().display().to_string())),
                )
            },
        );
    }

    /// `node stop`.
    pub fn node_stopped(&self, stopped: bool) {
        self.emit(json!({ "stopped": stopped }), || {
            if stopped {
                format!(
                    "{} {}",
                    badge("✔ NODE STOPPED", PALETTE.success),
                    label("the chain is kept on disk")
                )
            } else {
                format!(
                    "{} {}",
                    badge("NODE", PALETTE.info),
                    label("was not running")
                )
            }
        });
    }

    /// `node status`.
    pub fn node_status(&self, status: &crate::node::Status, shared: &crate::node::Shared) {
        use crate::node::Status;
        let dir = shared.dir().display().to_string();
        match status {
            Status::Stopped => self.emit(json!({ "running": false, "dir": dir }), || {
                format!(
                    "{} {}\n\n{}\n  {}",
                    badge("○ NODE STOPPED", PALETTE.warning),
                    label("regtest"),
                    row("Data", 4, value(&dir)),
                    label("start it: node start (or any --node local command starts it on demand)")
                )
            }),
            Status::Running {
                port,
                height,
                pinned,
                users,
            } => self.emit(
                json!({ "running": true, "port": port, "height": height, "pinned": pinned, "users": users, "dir": dir }),
                || {
                    let lifetime = if *pinned {
                        "until `node stop`"
                    } else {
                        "until its last user exits"
                    };
                    format!(
                        "{} {}\n\n{}\n{}\n{}\n{}\n{}",
                        badge("● NODE RUNNING", PALETTE.success),
                        label("regtest"),
                        row("Height", 9, value(&thousands(u64::from(*height)))),
                        row("RPC port", 9, value(&port.to_string())),
                        row("In use by", 9, value(&format!("{users} program{}", plural(*users)))),
                        row("Runs", 9, value(lifetime)),
                        row("Data", 9, value(&dir)),
                    )
                },
            ),
        }
    }

    // === Addresses and balances

    /// `address`.
    pub fn address(&self, info: &AddressInfo) {
        self.emit(json!(info), || {
            let kind = match info.keychain {
                Keychain::External => "receive",
                Keychain::Internal => "change",
            };
            format!(
                "{} {}\n\n  {}",
                badge("RECEIVE", PALETTE.brand),
                label(&format!("{kind} address #{}", info.index)),
                value(&info.address.to_string()).bold()
            )
        });
    }

    /// `balance`.
    pub fn balance(&self, b: &Balance) {
        self.emit(json!(b), || {
            format!(
                "{}\n\n{}",
                badge("BALANCE", PALETTE.brand),
                balance_lines(b)
            )
        });
    }

    /// `sync`.
    pub fn sync(&self, r: &SyncReport, b: &Balance) {
        self.emit(json!({ "sync": r, "balance": b }), || {
            let mut s = format!(
                "{} {}\n",
                badge("✔ SYNCED", PALETTE.success),
                label(&format!(
                    "block {} · {} new block{} · {} mempool tx{} scanned",
                    thousands(u64::from(r.to.height)),
                    r.blocks_applied,
                    plural(r.blocks_applied as usize),
                    r.mempool_txs,
                    plural(r.mempool_txs)
                ))
            );
            if r.reorg_depth > 0 {
                s.push_str(&format!(
                    "{} {}\n",
                    badge("⚠ REORG", PALETTE.warning),
                    style::warning(&format!(
                        "chain reorganised; up to {} blocks replaced",
                        r.reorg_depth
                    ))
                ));
            }
            s.push('\n');
            s.push_str(&balance_lines(b));
            s
        });
    }

    /// `mine`.
    pub fn mined(&self, blocks: u32, to: &Address, r: &SyncReport, b: &Balance) {
        self.emit(
            json!({ "blocks": blocks, "to": to.to_string(), "sync": r, "balance": b }),
            || {
                format!(
                    "{} {}\n{}\n\n{}",
                    badge("⛏ MINED", PALETTE.success),
                    label(&format!(
                        "{blocks} block{} · tip {}",
                        plural(blocks as usize),
                        thousands(u64::from(r.to.height))
                    )),
                    row("Rewards to", 10, value(&to.to_string())),
                    balance_lines(b)
                )
            },
        );
    }

    // === Lists (paginated)

    /// `utxos`.
    pub fn utxos(&self, page: &Page<'_, Utxo>) {
        self.emit(page_json(page, json!(page.items)), || {
            let mut s = list_header("COINS", page);
            if page.items.is_empty() {
                s.push_str(&format!("\n  {}", label("No unspent outputs yet.")));
            }
            for u in page.items {
                let kind = match u.keychain {
                    Keychain::External => "receive",
                    Keychain::Internal => "change",
                };
                s.push_str(&format!(
                    "\n  {}  {}\n    {}  {}\n",
                    amount(&format!("{:>20}", sats(u.value))),
                    status_text(&u.status),
                    label(&format!("{kind} #{:<4}", u.derivation_index)),
                    value(&u.outpoint.to_string())
                ));
            }
            s.push_str(&list_footer("utxos", page));
            s
        });
    }

    /// `history`.
    pub fn history(&self, page: &Page<'_, TxDetails>) {
        self.emit(page_json(page, json!(page.items)), || {
            let mut s = list_header("HISTORY", page);
            if page.items.is_empty() {
                s.push_str(&format!("\n  {}", label("No transactions yet.")));
            }
            for t in page.items {
                let n = net(t);
                let (arrow, kind, tint) = if n < 0 {
                    ("↑", "SENT    ", PALETTE.warning)
                } else {
                    ("↓", "RECEIVED", PALETTE.success)
                };
                let fee = paid_fee(t).map(sats).unwrap_or_else(|| "-".into());
                s.push_str(&format!(
                    "\n  {} {}  {}  {}\n    {}  {}   {} {}\n",
                    arrow.color(tint.term()).bold(),
                    kind.color(tint.term()).bold(),
                    amount(&format!("{:>18} sat", signed_sats(n))),
                    status_text(&t.status),
                    label("txid"),
                    value(&t.txid.to_string()),
                    label("fee"),
                    value(&fee)
                ));
            }
            s.push_str(&list_footer("history", page));
            s
        });
    }

    // === Spending

    /// A broadcast payment (`send`, `sign-psbt --broadcast`).
    pub fn sent(&self, txid: Txid, fee: Amount, rate: FeeRate) {
        self.emit(
            json!({ "txid": txid, "fee_sat": fee.to_sat(), "fee_rate_sat_vb": rate.to_sat_per_vb_ceil() }),
            || {
                format!(
                    "{} {}\n\n{}\n{}",
                    badge("✔ SENT", PALETTE.success),
                    label("broadcast to the node; confirm with `status` after the next block"),
                    row("Txid", 4, value(&txid.to_string())),
                    row("Fee", 4, format!("{} {}", amount(&sats(fee)), label(&format!("({} sat/vB)", rate.to_sat_per_vb_ceil())))),
                )
            },
        );
    }

    /// `send --dry-run`.
    pub fn dry_run(&self, tx: &Transaction, fee: Amount, rate: FeeRate) {
        let hex = consensus::encode::serialize_hex(tx);
        self.emit(
            json!({ "txid": tx.compute_txid(), "hex": hex, "fee_sat": fee.to_sat(), "vsize": tx.vsize() }),
            || {
                format!(
                    "{} {}\n\n{}\n{}\n{}\n\n  {}",
                    badge("DRY RUN", PALETTE.info),
                    label("signed, not broadcast"),
                    row("Txid", 5, value(&tx.compute_txid().to_string())),
                    row("Size", 5, value(&format!("{} vB", tx.vsize()))),
                    row("Fee", 5, format!("{} {}", amount(&sats(fee)), label(&format!("({} sat/vB)", rate.to_sat_per_vb_ceil())))),
                    label(&hex)
                )
            },
        );
    }

    /// `bump-fee`.
    pub fn fee_bumped(&self, old_txid: Txid, new_txid: Txid) {
        self.emit(
            json!({ "old_txid": old_txid, "new_txid": new_txid }),
            || {
                format!(
                    "{} {}\n\n{}\n{}",
                    badge("✔ FEE BUMPED", PALETTE.success),
                    label("the replacement pays more; the original will drop out"),
                    row("Replaced", 8, label(&old_txid.to_string())),
                    row("New", 8, value(&new_txid.to_string())),
                )
            },
        );
    }

    /// `status`.
    pub fn status(&self, txid: Txid, status: &TxStatus) {
        self.emit(json!({ "txid": txid, "status": status }), || {
            format!(
                "{}\n\n{}\n{}",
                badge("TRANSACTION", PALETTE.brand),
                row("Txid", 6, value(&txid.to_string())),
                row("Status", 6, status_text(status)),
            )
        });
    }

    /// `export-psbt`.
    pub fn psbt_exported(&self, path: &std::path::Path, fee: Option<Amount>) {
        let path_str = path.display().to_string();
        self.emit(
            json!({ "path": path_str, "fee_sat": fee.map(|f| f.to_sat()) }),
            || {
                format!(
                    "{} {}\n\n{}\n{}",
                    badge("✔ PSBT EXPORTED", PALETTE.success),
                    label("unsigned; sign it with `sign-psbt` (here or on an offline machine)"),
                    row("File", 4, value(&path_str)),
                    row(
                        "Fee",
                        4,
                        amount(&fee.map(sats).unwrap_or_else(|| "-".into()))
                    ),
                )
            },
        );
    }

    /// `sign-psbt` without broadcasting.
    pub fn signed_psbt(&self, tx: &Transaction) {
        let txid = tx.compute_txid();
        let hex = consensus::encode::serialize_hex(tx);
        self.emit(json!({ "txid": txid, "hex": hex }), || {
            format!(
                "{} {}\n\n{}\n\n  {}",
                badge("✔ SIGNED", PALETTE.success),
                label("not broadcast; send the hex with any node"),
                row("Txid", 4, value(&txid.to_string())),
                label(&hex)
            )
        });
    }
}

// === Helpers

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn network_badge(network: Network) -> colored::ColoredString {
    badge(
        &network.to_string().to_uppercase(),
        PALETTE.network(network),
    )
}

fn status_text(status: &TxStatus) -> String {
    match status {
        TxStatus::Unconfirmed => style::warning("◌ unconfirmed").to_string(),
        TxStatus::Confirmed {
            height,
            confirmations,
        } => format!(
            "{} {}",
            style::success("● confirmed"),
            label(&format!(
                "in block {} · {confirmations} conf",
                thousands(u64::from(*height))
            ))
        ),
    }
}

fn balance_lines(b: &Balance) -> String {
    let line = |name: &str, a: Amount| {
        format!(
            "  {}  {}  {}",
            label(&format!("{name:<12}")),
            amount(&format!("{:>22}", sats(a))),
            label(&format!("{:>18}", btc(a)))
        )
    };
    format!(
        "{}\n{}\n{}\n  {}\n{}",
        line("Confirmed", b.confirmed),
        line("Unconfirmed", b.unconfirmed),
        line("Immature", b.immature),
        label(&"─".repeat(60)),
        line("Total", b.total())
    )
}

/// Recovery words in a numbered four-column grid.
fn word_grid(words: &str) -> String {
    words
        .split(' ')
        .collect::<Vec<_>>()
        .chunks(4)
        .enumerate()
        .map(|(row, chunk)| {
            let cells: Vec<String> = chunk
                .iter()
                .enumerate()
                .map(|(i, w)| {
                    format!(
                        "{} {}",
                        label(&format!("{:>4}.", row * 4 + i + 1)),
                        value(&format!("{w:<10}")).bold()
                    )
                })
                .collect();
            cells.join("  ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn list_header<T>(name: &str, page: &Page<'_, T>) -> String {
    format!(
        "{} {}\n{}",
        badge(name, PALETTE.brand),
        label(&page.summary()),
        rule(true)
    )
}

fn list_footer<T>(command: &str, page: &Page<'_, T>) -> String {
    let mut s = format!("{}", rule(true));
    if page.has_next() {
        s.push_str(&format!(
            "\n  {} {}",
            label("more:"),
            value(&format!("{command} --page {}   (or --all)", page.page + 1))
        ));
    }
    s
}

fn page_json<T>(page: &Page<'_, T>, items: Value) -> Value {
    json!({
        "page": page.page,
        "pages": page.pages,
        "per_page": page.per_page,
        "total": page.total,
        "items": items,
    })
}

/// What to do next for errors we recognise.
fn hint(err: &anyhow::Error) -> Option<String> {
    let text = err
        .chain()
        .map(|e| e.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    let has = |needle: &str| text.contains(needle);
    let hint = if has("already exists") {
        "Use another --datadir, or open this wallet: `tui`, `balance`, `address`."
    } else if has("no wallet yet") || has("no wallet at") {
        "Create one with `init` (or `wallets create <name>`), or bring one back with `restore --mnemonic \"...\"`."
    } else if has("could not start the local regtest node") {
        "Close the TUI or any other command using this wallet's node, then retry."
    } else if has("could not connect") || has("chain backend unreachable") {
        "Start bitcoind, check --rpc-url / --rpc-cookie, or use `--node local` (regtest, nothing to install)."
    } else if has("insufficient funds") {
        "Get coins first; on regtest: `--node local mine 101`."
    } else if has("invalid mnemonic") {
        "Check the words and their order: 12, 15, 18, 21 or 24 words from the BIP39 list."
    } else if has("wrong password") || has("decryption failed") {
        "The password for this wallet's encrypted words is different; try again."
    } else if has("not valid for") {
        "The address belongs to a different network than this wallet (see --network)."
    } else {
        return None;
    };
    Some(hint.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() {
        colored::control::set_override(false);
    }

    #[test]
    fn known_errors_get_a_hint() {
        let err = anyhow::anyhow!("a wallet already exists at .wallet/regtest");
        assert!(hint(&err).unwrap().contains("--datadir"));
        let nested =
            anyhow::anyhow!("chain backend unreachable").context("could not connect to http://x");
        assert!(hint(&nested).unwrap().contains("--node local"));
        assert!(hint(&anyhow::anyhow!("something odd")).is_none());
    }

    #[test]
    fn word_grid_numbers_every_word() {
        plain();
        let words = "a b c d e f g h i j k l";
        let grid = word_grid(words);
        assert_eq!(grid.lines().count(), 3);
        assert!(grid.contains("12. l"));
    }

    #[test]
    fn plain_text_has_no_escape_codes() {
        plain();
        let lines = balance_lines(&Balance {
            confirmed: Amount::from_sat(1_234_567),
            ..Balance::default()
        });
        assert!(!lines.contains('\u{1b}'));
        assert!(lines.contains("1,234,567 sat"));
        assert!(lines.contains("0.01234567 BTC"));
    }
}
