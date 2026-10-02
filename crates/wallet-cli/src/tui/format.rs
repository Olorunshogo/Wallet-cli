//! Turning library values into display text. Pure functions, no styling.

use std::time::Duration;

use wallet::bitcoin::Amount;
use wallet::{
    BroadcastError, BuildTxError, BumpFeeError, Error, FeeEstimateError, SignError, SourceError,
    SyncError, TxDetails, TxStatus,
};

/// `1234567` -> `"1,234,567"`.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `"1,234,567 sat"`.
pub fn sats(amount: Amount) -> String {
    format!("{} sat", thousands(amount.to_sat()))
}

/// `"0.01234567 BTC"`, always eight decimals.
pub fn btc(amount: Amount) -> String {
    let sat = amount.to_sat();
    format!("{}.{:08} BTC", sat / 100_000_000, sat % 100_000_000)
}

/// Signed change in the wallet's balance from a transaction.
pub fn net(tx: &TxDetails) -> i64 {
    tx.received.to_sat() as i64 - tx.sent.to_sat() as i64
}

/// The fee this wallet paid, if any. Only transactions that spend our coins
/// cost us a fee; for incoming payments the fee was the sender's.
pub fn paid_fee(tx: &TxDetails) -> Option<Amount> {
    (tx.sent > Amount::ZERO).then_some(tx.fee).flatten()
}

/// `+25,000` / `-10,141`.
pub fn signed_sats(value: i64) -> String {
    let sign = if value < 0 { "-" } else { "+" };
    format!("{sign}{}", thousands(value.unsigned_abs()))
}

/// Shorten a hash or address: `abcdef12…7890ab`.
pub fn short(text: &str, keep: usize) -> String {
    if text.chars().count() <= keep * 2 + 1 {
        return text.to_string();
    }
    let start: String = text.chars().take(keep).collect();
    let end: String = text
        .chars()
        .rev()
        .take(keep)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{start}…{end}")
}

/// `"unconfirmed"` / `"3 conf"`.
pub fn status(status: &TxStatus) -> String {
    match status {
        TxStatus::Unconfirmed => "unconfirmed".into(),
        TxStatus::Confirmed { confirmations, .. } => format!("{confirmations} conf"),
    }
}

/// `"just now"`, `"42s ago"`, `"3m ago"`, `"2h ago"`.
pub fn ago(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    match secs {
        0..=2 => "just now".into(),
        3..=59 => format!("{secs}s ago"),
        60..=3599 => format!("{}m ago", secs / 60),
        _ => format!("{}h ago", secs / 3600),
    }
}

/// A user-facing explanation of a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorView {
    /// Short headline.
    pub title: String,
    /// What it means or what to do.
    pub hint: String,
}

impl ErrorView {
    /// Build from parts.
    pub fn new(title: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            hint: hint.into(),
        }
    }
}

/// Explain a library error in plain words, using the typed variants.
///
/// This is where the library's per-operation error enums pay off: each case
/// gets a specific message and suggestion instead of a generic failure.
pub fn describe(err: &Error) -> ErrorView {
    match err {
        Error::BuildTx(BuildTxError::InsufficientFunds { needed, available }) => ErrorView::new(
            "Insufficient funds",
            format!(
                "Need {}, have {}. Short by {}.",
                sats(*needed),
                sats(*available),
                sats(needed.checked_sub(*available).unwrap_or(Amount::ZERO))
            ),
        ),
        Error::BuildTx(BuildTxError::WrongNetwork { address, expected }) => ErrorView::new(
            "Wrong network",
            format!("{} is not a {expected} address.", short(address, 10)),
        ),
        Error::BuildTx(BuildTxError::OutputBelowDust { .. }) => ErrorView::new(
            "Amount too small",
            "Outputs below the dust limit are not relayed. Send more.",
        ),
        Error::BuildTx(BuildTxError::NoRecipients) => {
            ErrorView::new("Nothing to send", "Add a recipient first.")
        }
        Error::BumpFee(BumpFeeError::FeeRateTooLow { required }) => ErrorView::new(
            "Fee rate too low",
            format!(
                "The replacement must pay at least {} sat/vB.",
                required.to_sat_per_vb_ceil()
            ),
        ),
        Error::BumpFee(BumpFeeError::AlreadyConfirmed(_)) => ErrorView::new(
            "Already confirmed",
            "Mined transactions cannot be replaced.",
        ),
        Error::BumpFee(BumpFeeError::NotReplaceable(_)) => ErrorView::new(
            "Not replaceable",
            "This transaction opted out of replace-by-fee.",
        ),
        Error::BumpFee(BumpFeeError::NotFound(_)) => {
            ErrorView::new("Unknown transaction", "Sync and try again.")
        }
        Error::BumpFee(BumpFeeError::InsufficientFunds { .. }) => ErrorView::new(
            "Cannot afford the bump",
            "There is not enough change or other coins to pay the higher fee.",
        ),
        Error::Sign(SignError::WatchOnly) => ErrorView::new(
            "Watch-only wallet",
            "This wallet has no private keys, so it cannot sign.",
        ),
        Error::Sign(SignError::NothingToSign) => ErrorView::new(
            "Nothing to sign",
            "None of the inputs belong to this wallet.",
        ),
        Error::Sync(SyncError::WrongChain) => ErrorView::new(
            "Wrong chain",
            "The node is on a different network than this wallet.",
        ),
        Error::Sync(SyncError::Source(SourceError::Connection(_)))
        | Error::Source(SourceError::Connection(_))
        | Error::Broadcast(BroadcastError::Connection(_))
        | Error::FeeEstimate(FeeEstimateError::Connection(_)) => ErrorView::new(
            "Node unreachable",
            "Check that bitcoind is running and the RPC settings are right. Retrying automatically.",
        ),
        Error::Broadcast(BroadcastError::Rejected { reason }) => {
            ErrorView::new("Rejected by the node", reason.clone())
        }
        Error::FeeEstimate(FeeEstimateError::Unavailable { .. }) => ErrorView::new(
            "No fee estimate",
            "The node has no fee data yet. Enter a fee rate manually.",
        ),
        Error::NotFinalized => ErrorView::new(
            "Not fully signed",
            "More signatures are needed than this wallet holds.",
        ),
        other => ErrorView::new("Something went wrong", chain(other)),
    }
}

/// True when the error means the node could not be reached (as opposed to
/// the node answering with a refusal).
pub fn is_connection(err: &Error) -> bool {
    matches!(
        err,
        Error::Sync(SyncError::Source(SourceError::Connection(_)))
            | Error::Source(SourceError::Connection(_))
            | Error::Broadcast(BroadcastError::Connection(_))
            | Error::FeeEstimate(FeeEstimateError::Connection(_))
    )
}

/// The error and its causes on one line.
pub fn chain(err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use wallet::bitcoin::{FeeRate, Network};

    use super::*;

    #[test]
    fn numbers_get_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(sats(Amount::from_sat(25_000)), "25,000 sat");
    }

    #[test]
    fn btc_always_has_eight_decimals() {
        assert_eq!(btc(Amount::from_sat(1)), "0.00000001 BTC");
        assert_eq!(btc(Amount::from_sat(150_000_000)), "1.50000000 BTC");
    }

    #[test]
    fn signed_amounts_and_shortening() {
        assert_eq!(signed_sats(-10_141), "-10,141");
        assert_eq!(signed_sats(25_000), "+25,000");
        assert_eq!(short("abcdefghijklmnop", 3), "abc…nop");
        assert_eq!(short("abc", 3), "abc");
    }

    #[test]
    fn only_outgoing_transactions_show_a_fee() {
        let mut tx = TxDetails {
            txid: wallet::bitcoin::Txid::from_raw_hash(wallet::bitcoin::hashes::Hash::all_zeros()),
            sent: Amount::ZERO,
            received: Amount::from_sat(5_000),
            fee: Some(Amount::ZERO),
            status: TxStatus::Unconfirmed,
        };
        assert_eq!(paid_fee(&tx), None, "incoming: not our fee");
        tx.sent = Amount::from_sat(10_000);
        tx.fee = Some(Amount::from_sat(141));
        assert_eq!(paid_fee(&tx), Some(Amount::from_sat(141)));
    }

    #[test]
    fn relative_times() {
        assert_eq!(ago(Duration::from_secs(1)), "just now");
        assert_eq!(ago(Duration::from_secs(42)), "42s ago");
        assert_eq!(ago(Duration::from_secs(180)), "3m ago");
        assert_eq!(ago(Duration::from_secs(7_200)), "2h ago");
    }

    #[test]
    fn typed_errors_get_specific_messages() {
        let short_by = describe(&Error::BuildTx(BuildTxError::InsufficientFunds {
            needed: Amount::from_sat(50_000),
            available: Amount::from_sat(10_000),
        }));
        assert_eq!(short_by.title, "Insufficient funds");
        assert!(short_by.hint.contains("40,000 sat"), "{}", short_by.hint);

        let network = describe(&Error::BuildTx(BuildTxError::WrongNetwork {
            address: "bc1qxyz".into(),
            expected: Network::Regtest,
        }));
        assert!(network.hint.contains("regtest"));

        let bump = describe(&Error::BumpFee(BumpFeeError::FeeRateTooLow {
            required: FeeRate::from_sat_per_vb_u32(3),
        }));
        assert!(bump.hint.contains("3 sat/vB"));

        let offline = Error::Source(SourceError::Connection("refused".into()));
        assert_eq!(describe(&offline).title, "Node unreachable");
        assert!(is_connection(&offline));
        assert!(!is_connection(&Error::Broadcast(
            BroadcastError::Rejected {
                reason: "insufficient fee".into()
            }
        )));
    }

    #[test]
    fn unknown_errors_keep_their_cause_chain() {
        let err = Error::Extract("fee rate absurdly high".into());
        let view = describe(&err);
        assert!(view.hint.contains("absurdly high"), "{}", view.hint);
    }
}
