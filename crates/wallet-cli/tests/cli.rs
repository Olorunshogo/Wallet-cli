//! Tests that run the real `wallet-cli` binary, the way a user or a script
//! would: arguments in, exit code, stdout/stderr and files out.
//!
//! Each test gets its own directory, and `HOME` plus every `WALLET_*`
//! variable are cleared so nothing on the developer's machine leaks in.

use std::path::Path;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use serde_json::Value;
use tempfile::TempDir;

const WALLET_VARS: [&str; 14] = [
    "WALLET_NAME",
    "WALLET_NODE_DIR",
    "WALLET_NODE_PORT",
    "WALLET_NETWORK",
    "WALLET_DATADIR",
    "WALLET_ENCRYPT",
    "WALLET_ALLOW_MAINNET",
    "WALLET_MNEMONIC",
    "WALLET_PASSPHRASE",
    "WALLET_RPC_URL",
    "WALLET_RPC_COOKIE",
    "WALLET_RPC_USER",
    "WALLET_RPC_PASS",
    "WALLET_NODE",
];

/// `wallet-cli` running in `dir`, isolated from the real environment.
fn cli(dir: &Path) -> Command {
    let mut cmd = cargo_bin_cmd!("wallet-cli");
    cmd.current_dir(dir).env("HOME", dir).env_remove("RUST_LOG");
    for var in WALLET_VARS {
        cmd.env_remove(var);
    }
    cmd
}

/// Run and parse stdout as JSON, failing loudly with stderr on error.
fn json(cmd: &mut Command) -> Value {
    let output = cmd.output().unwrap();
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("valid JSON on stdout")
}

fn init(dir: &Path, extra: &[&str]) -> Value {
    json(cli(dir).args(["--json", "init"]).args(extra))
}

// === Without a node

#[test]
fn help_lists_the_commands() {
    let tmp = TempDir::new().unwrap();
    cli(tmp.path()).arg("--help").assert().success().stdout(
        predicate::str::contains("init")
            .and(predicate::str::contains("tui"))
            .and(predicate::str::contains("send")),
    );
}

#[test]
fn init_generates_fresh_valid_words_and_stores_them_privately() {
    let tmp = TempDir::new().unwrap();
    let first = init(tmp.path(), &["--words", "15"]);
    let words = first["mnemonic"].as_str().unwrap();
    assert_eq!(words.split(' ').count(), 15);
    wallet::validate_mnemonic(words).unwrap();

    let dir = tmp.path().join(".wallet/regtest/wallets/default");
    assert!(dir.join("wallet.sqlite").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("mnemonic"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    let other = TempDir::new().unwrap();
    let second = init(other.path(), &[]);
    assert_ne!(
        first["mnemonic"], second["mnemonic"],
        "every wallet gets new words"
    );
}

#[test]
fn init_refuses_to_overwrite_an_existing_wallet() {
    let tmp = TempDir::new().unwrap();
    init(tmp.path(), &[]);
    cli(tmp.path())
        .arg("init")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("already exists"));
}

#[test]
fn bad_arguments_are_rejected_before_anything_happens() {
    let tmp = TempDir::new().unwrap();
    cli(tmp.path())
        .args(["init", "--words", "13"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("12, 15, 18, 21 or 24"));
    cli(tmp.path())
        .args(["--network", "moon", "balance"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown network"));
    assert!(!tmp.path().join(".wallet").exists(), "nothing was created");
}

#[test]
fn address_is_stable_until_a_new_one_is_asked_for() {
    let tmp = TempDir::new().unwrap();
    init(tmp.path(), &[]);
    let a = json(cli(tmp.path()).args(["--json", "address"]));
    let b = json(cli(tmp.path()).args(["--json", "address"]));
    assert_eq!(a, b);
    assert!(a["address"].as_str().unwrap().starts_with("bcrt1q"));
    let c = json(cli(tmp.path()).args(["--json", "address", "--new"]));
    assert_eq!(c["index"], 1);
}

#[test]
fn restore_rebuilds_the_same_wallet_and_rejects_bad_words() {
    let original = TempDir::new().unwrap();
    let words = init(original.path(), &[])["mnemonic"]
        .as_str()
        .unwrap()
        .to_string();
    let expected = json(cli(original.path()).args(["--json", "descriptors"]));

    let restored = TempDir::new().unwrap();
    cli(restored.path())
        .args(["restore", "--mnemonic", &words])
        .assert()
        .success();
    let got = json(cli(restored.path()).args(["--json", "descriptors"]));
    assert_eq!(got, expected, "same words, same wallet");

    let bad = TempDir::new().unwrap();
    cli(bad.path())
        .args(["restore", "--mnemonic", "these are not recovery words"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid mnemonic"));
}

#[test]
fn restore_reads_words_from_the_environment() {
    let original = TempDir::new().unwrap();
    let words = init(original.path(), &[])["mnemonic"]
        .as_str()
        .unwrap()
        .to_string();
    let restored = TempDir::new().unwrap();
    cli(restored.path())
        .arg("restore")
        .env("WALLET_MNEMONIC", &words)
        .assert()
        .success();
}

#[test]
fn mainnet_is_guarded() {
    let tmp = TempDir::new().unwrap();
    cli(tmp.path())
        .args(["--network", "bitcoin", "balance"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--allow-mainnet"));
    cli(tmp.path())
        .args(["--network", "mainnet", "--allow-mainnet", "init"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must be encrypted"));
}

#[test]
fn dot_env_file_is_loaded_and_flags_win() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join(".env"),
        "WALLET_NETWORK=signet\nWALLET_DATADIR=from-env\n",
    )
    .unwrap();
    init(tmp.path(), &[]);
    assert!(tmp.path().join("from-env/wallet.sqlite").exists());
    let address = json(cli(tmp.path()).args(["--json", "address"]));
    assert!(
        address["address"].as_str().unwrap().starts_with("tb1q"),
        "signet from .env"
    );

    // A flag overrides the .env value.
    init(tmp.path(), &["--datadir", "from-flag"]);
    assert!(tmp.path().join("from-flag/wallet.sqlite").exists());
}

#[test]
fn json_errors_are_machine_readable() {
    let tmp = TempDir::new().unwrap();
    let output = cli(tmp.path())
        .args(["--json", "balance"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let err: Value = serde_json::from_slice(&output.stderr).expect("JSON error on stderr");
    assert!(err["error"][0].as_str().unwrap().contains("no wallet yet"));
    assert!(output.stdout.is_empty(), "stdout stays clean for scripts");
}

#[test]
fn commands_that_need_a_node_fail_clearly_without_one() {
    let tmp = TempDir::new().unwrap();
    init(tmp.path(), &[]);
    // Nothing listens on port 9 and there is no cookie under the fake HOME.
    cli(tmp.path())
        .args(["--rpc-url", "http://127.0.0.1:9", "sync"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("could not connect"));
}

#[test]
fn piped_output_is_plain_text_without_escape_codes() {
    let tmp = TempDir::new().unwrap();
    cli(tmp.path())
        .arg("init")
        .assert()
        .success()
        .stdout(predicate::str::contains("WALLET CREATED"))
        .stdout(predicate::str::contains("Recovery words"))
        .stdout(predicate::str::contains("\u{1b}[").not());
    cli(tmp.path())
        .arg("balance")
        .env("NO_COLOR", "1")
        .assert()
        .success()
        .stdout(predicate::str::contains("0 sat"))
        .stdout(predicate::str::contains("\u{1b}[").not());
}

#[test]
fn errors_come_with_a_hint() {
    let tmp = TempDir::new().unwrap();
    cli(tmp.path())
        .arg("balance")
        .assert()
        .failure()
        .stderr(predicate::str::contains("ERROR"))
        .stderr(predicate::str::contains("Create one with `init`"));
}

#[test]
fn empty_lists_page_cleanly() {
    let tmp = TempDir::new().unwrap();
    init(tmp.path(), &[]);
    let page = json(cli(tmp.path()).args(["--json", "history"]));
    assert_eq!(page["total"], 0);
    assert_eq!(page["pages"], 1);
    assert_eq!(page["items"].as_array().unwrap().len(), 0);
    cli(tmp.path())
        .arg("utxos")
        .assert()
        .success()
        .stdout(predicate::str::contains("No unspent outputs yet"));
    cli(tmp.path())
        .args(["history", "--per-page", "0"])
        .assert()
        .failure();
    cli(tmp.path())
        .args(["history", "--all", "--page", "2"])
        .assert()
        .failure();
}

#[test]
fn node_none_explains_what_needs_a_node() {
    let tmp = TempDir::new().unwrap();
    init(tmp.path(), &[]);
    cli(tmp.path())
        .args(["--node", "none", "sync"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--node none"));
    // Offline commands still work.
    cli(tmp.path())
        .args(["--node", "none", "address"])
        .assert()
        .success();
}

// === With a real regtest node

mod regtest {
    use corepc_node::Node;
    use wallet::bitcoin::Address;

    use super::*;

    struct Env {
        node: Node,
        tmp: TempDir,
    }

    impl Env {
        fn start() -> Self {
            let exe = corepc_node::exe_path().expect("find bitcoind");
            let node = Node::new(exe).expect("start bitcoind");
            let tmp = TempDir::new().unwrap();
            init(tmp.path(), &[]);
            Self { node, tmp }
        }

        fn cli(&self) -> Command {
            let mut cmd = cli(self.tmp.path());
            cmd.env("WALLET_RPC_URL", self.node.rpc_url())
                .env("WALLET_RPC_COOKIE", &self.node.params.cookie_file);
            cmd
        }

        fn json(&self, args: &[&str]) -> Value {
            json(self.cli().arg("--json").args(args))
        }

        fn mine(&self, blocks: usize, to: &str) {
            let address: Address = to.parse::<Address<_>>().unwrap().assume_checked();
            self.node
                .client
                .generate_to_address(blocks, &address)
                .unwrap();
        }

        /// A fresh address belonging to some other wallet.
        fn stranger(&self) -> String {
            let other = TempDir::new().unwrap();
            init(other.path(), &[]);
            json(cli(other.path()).args(["--json", "address"]))["address"]
                .as_str()
                .unwrap()
                .to_string()
        }
    }

    fn txid_of(v: &Value) -> String {
        v["txid"].as_str().unwrap().to_string()
    }

    #[test]
    fn receive_sync_send_bump_and_confirm() {
        let env = Env::start();
        let address = env.json(&["address"])["address"]
            .as_str()
            .unwrap()
            .to_string();
        env.mine(101, &address);

        let synced = env.json(&["sync"]);
        assert_eq!(synced["sync"]["to"]["height"], 101);
        assert_eq!(synced["balance"]["confirmed"], 2 * 5_000_000_000u64);

        // Pay at 1 sat/vB, then bump to 10.
        let bob = env.stranger();
        let sent = env.json(&["send", &bob, "250000", "--fee-rate", "1"]);
        let first = txid_of(&sent);
        assert_eq!(
            env.json(&["status", &first])["status"]["state"],
            "unconfirmed"
        );

        env.cli()
            .args(["bump-fee", &first, "--fee-rate", "10"])
            .assert()
            .success();

        env.mine(1, &bob);
        env.cli().arg("sync").assert().success();
        let history = env.json(&["history"]);
        let txs = history["items"].as_array().unwrap();
        assert!(!txs.iter().any(|t| t["txid"] == first.as_str()), "replaced");
        let replacement = txs
            .iter()
            .find(|t| t["sent"].as_u64().unwrap_or(0) > 0)
            .expect("the payment");
        assert_eq!(replacement["status"]["state"], "confirmed");
        assert!(
            replacement["fee"].as_u64().unwrap() > 1_000,
            "paid the bumped fee"
        );

        let utxos = env.json(&["utxos"]);
        assert!(!utxos["items"].as_array().unwrap().is_empty());
    }

    #[test]
    fn dry_run_and_psbt_export_then_sign_and_broadcast() {
        let env = Env::start();
        let address = env.json(&["address"])["address"]
            .as_str()
            .unwrap()
            .to_string();
        env.mine(101, &address);
        env.cli().arg("sync").assert().success();
        let bob = env.stranger();

        // Dry run signs but does not broadcast.
        let dry = env.json(&["send", &bob, "10000", "--fee-rate", "2", "--dry-run"]);
        assert!(dry["hex"].as_str().unwrap().len() > 100);
        env.cli().arg("sync").assert().success();
        assert!(
            env.json(&["history"])["items"]
                .as_array()
                .unwrap()
                .iter()
                .all(|t| t["sent"] == 0)
        );

        // Export an unsigned PSBT, then sign and broadcast it.
        env.cli()
            .args([
                "export-psbt",
                &bob,
                "20000",
                "--fee-rate",
                "2",
                "--output",
                "pay.psbt",
            ])
            .assert()
            .success();
        assert!(env.tmp.path().join("pay.psbt").exists());
        env.cli()
            .args(["sign-psbt", "--input", "pay.psbt", "--broadcast"])
            .assert()
            .success()
            .stdout(predicate::str::contains("broadcast"));
        env.cli().arg("sync").assert().success();
        assert!(
            env.json(&["history"])["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["sent"].as_u64().unwrap() > 0)
        );
    }

    #[test]
    fn node_rejections_and_shortfalls_are_reported() {
        let env = Env::start();
        let bob = env.stranger();
        // Empty wallet: a typed insufficient-funds error, exit code 1.
        env.cli()
            .args(["send", &bob, "1000", "--fee-rate", "1"])
            .assert()
            .code(1)
            .stderr(predicate::str::contains("insufficient funds"));
        // A mainnet address on regtest.
        env.cli()
            .args(["send", "bc1qcr8te4kr609gcawutmrza0j4xv80jy8z306fyu", "1000"])
            .assert()
            .failure();
    }

    #[test]
    fn history_pages_through_many_transactions() {
        let env = Env::start();
        let address = env.json(&["address"])["address"]
            .as_str()
            .unwrap()
            .to_string();
        // 25 coinbase rewards to this wallet: 25 transactions.
        env.mine(25, &address);
        env.cli().arg("sync").assert().success();

        let first = env.json(&["history", "--per-page", "10"]);
        assert_eq!(
            (first["total"].as_u64(), first["pages"].as_u64()),
            (Some(25), Some(3))
        );
        assert_eq!(first["items"].as_array().unwrap().len(), 10);
        let last = env.json(&["history", "--per-page", "10", "--page", "3"]);
        assert_eq!(last["items"].as_array().unwrap().len(), 5);
        let all = env.json(&["history", "--all"]);
        assert_eq!(all["items"].as_array().unwrap().len(), 25);
        // Pages do not overlap.
        let ids = |v: &Value| -> Vec<String> {
            v["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["txid"].to_string())
                .collect()
        };
        assert!(ids(&first).iter().all(|id| !ids(&last).contains(id)));

        env.cli()
            .args(["history", "--per-page", "10"])
            .assert()
            .success()
            .stdout(predicate::str::contains("page 1 of 3 · 1–10 of 25"))
            .stdout(predicate::str::contains("history --page 2"));
    }

    #[test]
    fn mine_command_funds_the_wallet() {
        let env = Env::start();
        let mined = env.json(&["mine", "101"]);
        assert_eq!(mined["blocks"], 101);
        assert_eq!(mined["balance"]["confirmed"], 2 * 5_000_000_000u64);
    }
}

// === With a node the CLI starts itself

#[cfg(feature = "local-node")]
#[test]
fn local_node_needs_nothing_installed_and_keeps_its_chain() {
    let tmp = TempDir::new().unwrap();
    init(tmp.path(), &[]);
    let local = |args: &[&str]| {
        json(
            cli(tmp.path())
                .args(["--json", "--node", "local"])
                .args(args),
        )
    };

    let mined = local(&["mine", "101"]);
    assert_eq!(mined["sync"]["to"]["height"], 101);
    assert!(tmp.path().join(".wallet/regtest/node").exists());

    // A second command restarts the same node on the same chain.
    let synced = local(&["sync"]);
    assert_eq!(synced["sync"]["to"]["height"], 101);
    assert_eq!(synced["balance"]["confirmed"], 2 * 5_000_000_000u64);
}

// === Named wallets and the shared local node

#[test]
fn named_wallets_lifecycle() {
    let tmp = TempDir::new().unwrap();
    let wallets = |args: &[&str]| json(cli(tmp.path()).args(["--json", "wallets"]).args(args));

    assert_eq!(wallets(&[])["wallets"].as_array().unwrap().len(), 0);
    json(cli(tmp.path()).args(["--json", "wallets", "create", "alice"]));
    json(cli(tmp.path()).args(["--json", "-w", "bob", "init", "--words", "15"]));
    let list = wallets(&["list"]);
    let names: Vec<&str> = list["wallets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["alice", "bob"]);

    // Two wallets, none chosen: commands must name one.
    cli(tmp.path())
        .arg("address")
        .assert()
        .failure()
        .stderr(predicate::str::contains("you have: alice, bob"));
    wallets(&["use", "bob"]);
    let bob_addr = json(cli(tmp.path()).args(["--json", "address"]));
    let alice_addr = json(cli(tmp.path()).args(["--json", "-w", "alice", "address"]));
    assert_ne!(
        bob_addr["address"], alice_addr["address"],
        "different wallets"
    );

    wallets(&["rename", "bob", "carol"]);
    assert!(
        wallets(&["list"])["wallets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["name"] == "carol" && w["default"] == true)
    );

    // Removing needs confirmation (no terminal here, so --yes).
    cli(tmp.path())
        .args(["wallets", "remove", "carol"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--yes"));
    wallets(&["remove", "carol", "--yes"]);
    assert_eq!(wallets(&["list"])["wallets"].as_array().unwrap().len(), 1);

    cli(tmp.path())
        .args(["wallets", "create", "Not Valid"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid wallet name"));
}

#[test]
fn an_old_single_wallet_moves_to_default() {
    let tmp = TempDir::new().unwrap();
    // The old layout: files straight in .wallet/regtest.
    cli(tmp.path())
        .args(["--datadir", ".wallet/regtest", "init"])
        .assert()
        .success();
    let before = json(cli(tmp.path()).args(["--json", "--datadir", ".wallet/regtest", "address"]));

    cli(tmp.path())
        .arg("address")
        .assert()
        .success()
        .stderr(predicate::str::contains("moved your existing wallet"));
    let after = json(cli(tmp.path()).args(["--json", "-w", "default", "address"]));
    assert_eq!(before["address"], after["address"], "same wallet, new home");
    assert!(
        tmp.path()
            .join(".wallet/regtest/wallets/default/wallet.sqlite")
            .exists()
    );
}

#[cfg(feature = "local-node")]
mod shared_node {
    use super::*;

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    struct Env {
        tmp: TempDir,
        port: String,
    }

    impl Env {
        fn new() -> Self {
            let tmp = TempDir::new().unwrap();
            for name in ["alice", "bob"] {
                cli(tmp.path())
                    .args(["wallets", "create", name])
                    .assert()
                    .success();
            }
            Self {
                tmp,
                port: free_port().to_string(),
            }
        }

        fn cli(&self, wallet: &str) -> Command {
            let mut cmd = cli(self.tmp.path());
            cmd.args(["--node", "local", "--node-port", &self.port, "-w", wallet]);
            cmd
        }

        fn json(&self, wallet: &str, args: &[&str]) -> Value {
            json(self.cli(wallet).arg("--json").args(args))
        }

        fn status(&self) -> Value {
            json(cli(self.tmp.path()).args(["--json", "--node-port", &self.port, "node", "status"]))
        }
    }

    impl Drop for Env {
        fn drop(&mut self) {
            let _ = cli(self.tmp.path())
                .args(["--node-port", &self.port, "node", "stop"])
                .output();
        }
    }

    #[test]
    fn two_wallets_share_one_chain_and_the_node_stops_when_unused() {
        let env = Env::new();
        env.json("alice", &["mine", "101"]);
        let bob = env.json("bob", &["address"])["address"]
            .as_str()
            .unwrap()
            .to_string();
        env.json("alice", &["send", &bob, "250000", "--fee-rate", "2"]);
        env.json("alice", &["mine", "1"]);

        let synced = env.json("bob", &["sync"]);
        assert_eq!(
            synced["sync"]["to"]["height"], 102,
            "bob is on alice's chain"
        );
        assert_eq!(synced["balance"]["confirmed"], 250_000);

        // Started on demand, so it stopped after the last command.
        assert_eq!(env.status()["running"], false);

        // The wallet list remembers balances from the last sync.
        let list = json(cli(env.tmp.path()).args(["--json", "wallets"]));
        let bob_entry = list["wallets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["name"] == "bob")
            .unwrap()
            .clone();
        assert_eq!(bob_entry["last_balance_sat"], 250_000);

        // A wallet with coins is not removed without --force.
        cli(env.tmp.path())
            .args(["wallets", "remove", "bob", "--yes"])
            .assert()
            .failure()
            .stderr(predicate::str::contains("--force"));
    }

    #[test]
    fn node_start_pins_it_until_stop() {
        let env = Env::new();
        json(cli(env.tmp.path()).args(["--json", "--node-port", &env.port, "node", "start"]));
        let status = env.status();
        assert_eq!(
            (status["running"].as_bool(), status["pinned"].as_bool()),
            (Some(true), Some(true))
        );

        // Commands come and go; a pinned node stays up.
        env.json("alice", &["mine", "1"]);
        assert_eq!(env.status()["running"], true);
        assert_eq!(env.status()["height"], 1);

        let stopped =
            json(cli(env.tmp.path()).args(["--json", "--node-port", &env.port, "node", "stop"]));
        assert_eq!(stopped["stopped"], true);
        assert_eq!(env.status()["running"], false);

        // The chain survived the stop.
        assert_eq!(env.json("alice", &["sync"])["sync"]["to"]["height"], 1);
    }

    #[cfg(unix)]
    #[test]
    fn regtest_script_demo_pays_bob() {
        let tmp = TempDir::new().unwrap();
        let port = free_port().to_string();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/regtest.sh");
        let bin = assert_cmd::cargo::cargo_bin!("wallet-cli");
        let run = |args: &[&str]| {
            let mut cmd = Command::new(&script);
            cmd.current_dir(tmp.path())
                .env("HOME", tmp.path())
                .env("WALLET_CLI", bin)
                .env("WALLET_NODE_PORT", &port)
                .env_remove("WALLET_NAME")
                .env_remove("WALLET_NODE")
                .env_remove("WALLET_DATADIR")
                .args(args);
            cmd
        };
        run(&["demo"])
            .assert()
            .success()
            .stdout(predicate::str::contains("250,000 sat"))
            .stdout(predicate::str::contains("\u{1b}[").not());
        run(&["status"])
            .assert()
            .success()
            .stdout(predicate::str::contains("RUNNING"));
        run(&["stop"]).assert().success();
        run(&["status"])
            .assert()
            .success()
            .stdout(predicate::str::contains("STOPPED"));
    }
}
