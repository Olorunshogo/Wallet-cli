//! Loading states for commands that wait on the node: a spinner with a
//! message, which can turn into a progress bar (for sync).
//!
//! Drawn on stderr and only when stderr is a terminal and `--json` is off, so
//! piped output and scripts never see it.

use std::io::IsTerminal;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", " "];

/// A running spinner or progress bar. Cleared when finished or dropped.
pub struct Activity {
    bar: ProgressBar,
    color: bool,
}

/// Whether spinners should be drawn at all.
pub fn enabled(json: bool) -> bool {
    !json && std::io::stderr().is_terminal()
}

impl Activity {
    /// Start a spinner with `message`, or a silent no-op when not `enabled`.
    pub fn start(enabled: bool, message: impl Into<String>) -> Self {
        let color = std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
        let bar = if enabled {
            ProgressBar::with_draw_target(None, ProgressDrawTarget::stderr())
        } else {
            ProgressBar::hidden()
        };
        bar.set_style(spinner_style(color));
        bar.set_message(message.into());
        bar.enable_steady_tick(Duration::from_millis(80));
        Self { bar, color }
    }

    /// Change the message.
    pub fn say(&self, message: impl Into<String>) {
        self.bar.set_message(message.into());
    }

    /// Switch to a progress bar over `total` steps (e.g. blocks to fetch).
    pub fn progress(&self, total: u64, message: impl Into<String>) {
        self.bar.set_length(total);
        self.bar.set_position(0);
        self.bar.set_message(message.into());
        self.bar.set_style(bar_style(self.color));
    }

    /// Steps done so far.
    pub fn set_position(&self, done: u64) {
        self.bar.set_position(done);
    }

    /// Remove the spinner from the screen.
    pub fn done(self) {
        self.bar.finish_and_clear();
    }
}

impl Drop for Activity {
    fn drop(&mut self) {
        // Errors return early; never leave a spinner on screen.
        self.bar.finish_and_clear();
    }
}

fn spinner_style(color: bool) -> ProgressStyle {
    let template = if color {
        "{spinner:.yellow} {msg}"
    } else {
        "{spinner} {msg}"
    };
    ProgressStyle::with_template(template)
        .expect("valid template")
        .tick_strings(FRAMES)
}

fn bar_style(color: bool) -> ProgressStyle {
    let template = if color {
        "{spinner:.yellow} {msg} {bar:30.yellow/black} {pos}/{len} blocks"
    } else {
        "{spinner} {msg} {bar:30} {pos}/{len} blocks"
    };
    ProgressStyle::with_template(template)
        .expect("valid template")
        .tick_strings(FRAMES)
        .progress_chars("━━─")
}
