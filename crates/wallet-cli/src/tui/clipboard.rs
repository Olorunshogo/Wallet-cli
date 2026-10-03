//! Copying text out of the TUI.
//!
//! The app only queues [`ClipRequest`]s; the event loop runs them between
//! frames on the UI thread. That matters for the terminal fallback: its
//! escape code goes to the same output as the screen, so it must not land in
//! the middle of a frame.

use std::io::Write;

use base64::Engine;
use zeroize::Zeroizing;

/// Something the app wants done with the clipboard.
pub enum ClipRequest {
    /// Put `text` on the clipboard.
    Copy {
        /// What to copy.
        text: Zeroizing<String>,
        /// Shown in the notification, e.g. "recovery words".
        what: &'static str,
        /// Secret text is cleared again after a while.
        secret: bool,
    },
    /// Clear the clipboard if it still holds `text` (a copied secret).
    Clear {
        /// The secret that was copied.
        text: Zeroizing<String>,
    },
}

/// How text reached the clipboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// The desktop clipboard (X11, Wayland, macOS, Windows).
    System,
    /// The terminal's clipboard escape code (OSC 52), for terminals and SSH
    /// sessions without a desktop clipboard. Not every terminal supports it.
    Terminal,
}

/// Where copied text goes. The real one is [`SystemClipboard`]; tests use a
/// fake.
pub trait Clipboard {
    /// Copy `text`.
    fn set(&mut self, text: &str) -> Result<Via, String>;
    /// Clear the clipboard if it still holds `text`.
    fn clear_if(&mut self, text: &str);
}

/// The desktop clipboard, falling back to OSC 52.
///
/// On X11 the copying program serves the text itself, so it stays pasteable
/// while the TUI runs (and after, with a clipboard manager). The instance is
/// therefore kept for the whole session.
#[derive(Default)]
pub struct SystemClipboard {
    system: Option<arboard::Clipboard>,
    tried: bool,
}

impl SystemClipboard {
    fn system(&mut self) -> Option<&mut arboard::Clipboard> {
        if !self.tried {
            self.tried = true;
            self.system = arboard::Clipboard::new()
                .inspect_err(|e| tracing::info!("no desktop clipboard ({e}); using OSC 52"))
                .ok();
        }
        self.system.as_mut()
    }

    fn osc52(text: &str) -> Result<Via, String> {
        let encoded = base64::engine::general_purpose::STANDARD.encode(text);
        let mut out = std::io::stdout();
        write!(out, "\x1b]52;c;{encoded}\x07")
            .and_then(|()| out.flush())
            .map_err(|e| e.to_string())?;
        Ok(Via::Terminal)
    }
}

impl Clipboard for SystemClipboard {
    fn set(&mut self, text: &str) -> Result<Via, String> {
        if let Some(system) = self.system()
            && system.set_text(text).is_ok()
        {
            return Ok(Via::System);
        }
        Self::osc52(text)
    }

    fn clear_if(&mut self, text: &str) {
        match self.system() {
            Some(system) => {
                // Leave it alone if the user copied something else since.
                if system.get_text().is_ok_and(|now| now == text) {
                    let _ = system.clear();
                }
            }
            // OSC 52 cannot read the clipboard back; clear unconditionally.
            None => {
                let _ = Self::osc52("");
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// An in-memory clipboard for tests: no desktop, no terminal.
    #[derive(Default)]
    pub struct FakeClipboard {
        pub text: Option<String>,
        pub clears: u32,
    }

    impl Clipboard for FakeClipboard {
        fn set(&mut self, text: &str) -> Result<Via, String> {
            self.text = Some(text.to_string());
            Ok(Via::System)
        }

        fn clear_if(&mut self, text: &str) {
            if self.text.as_deref() == Some(text) {
                self.text = None;
                self.clears += 1;
            }
        }
    }

    #[test]
    fn clears_only_when_the_clipboard_still_holds_that_text() {
        let mut clip = FakeClipboard::default();
        clip.set("words").unwrap();
        clip.clear_if("different");
        assert_eq!(clip.text.as_deref(), Some("words"), "left alone");
        clip.clear_if("words");
        assert_eq!(clip.text, None);
        assert_eq!(clip.clears, 1);
    }
}
