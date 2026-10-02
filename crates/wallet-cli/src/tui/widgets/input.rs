//! A text input and a validated form field built on it.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};

use crate::tui::theme::Theme;
use crate::tui::validate::Validator;

/// Editable single-line text with a cursor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInput {
    value: String,
    /// Cursor position in characters, not bytes.
    cursor: usize,
    masked: bool,
}

impl TextInput {
    /// An input that shows `•` instead of the text (passwords).
    pub fn masked() -> Self {
        Self {
            masked: true,
            ..Self::default()
        }
    }

    /// Current text.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Replace the text and move the cursor to the end.
    pub fn set(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.value.chars().count();
    }

    /// Cursor position in characters.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// What to draw: the text, or bullets when masked.
    pub fn display(&self) -> String {
        if self.masked {
            "•".repeat(self.value.chars().count())
        } else {
            self.value.clone()
        }
    }

    fn byte_at(&self, char_index: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_index)
            .map_or(self.value.len(), |(i, _)| i)
    }

    /// Apply an editing key. Returns true when the text changed.
    pub fn handle(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let len = self.value.chars().count();
        match key.code {
            KeyCode::Char('u') if ctrl => {
                let changed = !self.value.is_empty();
                self.set("");
                changed
            }
            KeyCode::Char('w') if ctrl => self.delete_word(),
            KeyCode::Char(c) if !ctrl => {
                let at = self.byte_at(self.cursor);
                self.value.insert(at, c);
                self.cursor += 1;
                true
            }
            KeyCode::Backspace if self.cursor > 0 => {
                let at = self.byte_at(self.cursor - 1);
                self.value.remove(at);
                self.cursor -= 1;
                true
            }
            KeyCode::Delete if self.cursor < len => {
                let at = self.byte_at(self.cursor);
                self.value.remove(at);
                true
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                false
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(len);
                false
            }
            KeyCode::Home => {
                self.cursor = 0;
                false
            }
            KeyCode::End => {
                self.cursor = len;
                false
            }
            _ => false,
        }
    }

    fn delete_word(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let chars: Vec<char> = self.value.chars().collect();
        let mut start = self.cursor;
        while start > 0 && chars[start - 1] == ' ' {
            start -= 1;
        }
        while start > 0 && chars[start - 1] != ' ' {
            start -= 1;
        }
        let (a, b) = (self.byte_at(start), self.byte_at(self.cursor));
        self.value.replace_range(a..b, "");
        self.cursor = start;
        true
    }
}

/// A labelled input paired with a validator.
///
/// Errors appear only after the user has typed in the field or tried to
/// submit, so an untouched form is not covered in red.
#[derive(Debug, Clone)]
pub struct Field<V: Validator> {
    /// Shown above the input.
    pub label: &'static str,
    /// Shown inside the input while it is empty.
    pub placeholder: &'static str,
    /// The text being edited.
    pub input: TextInput,
    /// Checks the text.
    pub validator: V,
    error: Option<String>,
    touched: bool,
}

impl<V: Validator> Field<V> {
    /// A plain field.
    pub fn new(label: &'static str, placeholder: &'static str, validator: V) -> Self {
        Self {
            label,
            placeholder,
            input: TextInput::default(),
            validator,
            error: None,
            touched: false,
        }
    }

    /// A field that hides what is typed.
    pub fn masked(label: &'static str, placeholder: &'static str, validator: V) -> Self {
        Self {
            input: TextInput::masked(),
            ..Self::new(label, placeholder, validator)
        }
    }

    /// Apply a key; re-validates once the field has been touched.
    pub fn handle(&mut self, key: KeyEvent) -> bool {
        let changed = self.input.handle(key);
        if changed {
            self.touched = true;
            self.check();
        }
        changed
    }

    /// Set the text programmatically (e.g. a fee estimate) and validate it.
    pub fn set(&mut self, value: impl Into<String>) {
        self.input.set(value);
        self.touched = true;
        self.check();
    }

    /// Clear text and errors.
    pub fn reset(&mut self) {
        self.input.set("");
        self.touched = false;
        self.error = None;
    }

    /// Validate now (e.g. on submit), showing any error.
    pub fn submit(&mut self) -> Result<V::Output, String> {
        self.touched = true;
        let result = self.validator.validate(self.input.value());
        self.error = result.as_ref().err().cloned();
        result
    }

    /// The parsed value without changing what is shown.
    pub fn value(&self) -> Result<V::Output, String> {
        self.validator.validate(self.input.value())
    }

    /// The error to show, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Show a server-side error on this field (e.g. "wrong password").
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.touched = true;
        self.error = Some(message.into());
    }

    fn check(&mut self) {
        if self.touched {
            self.error = self.validator.validate(self.input.value()).err();
        }
    }

    /// What the renderer needs.
    pub fn view(&self) -> FieldView<'_> {
        FieldView {
            label: self.label,
            placeholder: self.placeholder,
            text: self.input.display(),
            cursor: self.input.cursor(),
            error: self.error(),
        }
    }
}

/// Render data for a field, independent of its validator type.
pub struct FieldView<'a> {
    /// Field label.
    pub label: &'a str,
    /// Placeholder text.
    pub placeholder: &'a str,
    /// Text to draw (bullets when masked).
    pub text: String,
    /// Cursor position in characters.
    pub cursor: usize,
    /// Error to show under the field.
    pub error: Option<&'a str>,
}

/// Height a field needs: bordered input plus an error line.
pub const FIELD_HEIGHT: u16 = 4;

/// Draw a field. Focused fields get the accent border and a cursor.
pub fn render_field(f: &mut Frame, area: Rect, view: &FieldView<'_>, focused: bool, theme: &Theme) {
    let input_area = Rect {
        height: 3.min(area.height),
        ..area
    };
    let border = if view.error.is_some() {
        Style::new().fg(theme.error)
    } else {
        theme.border(focused)
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border)
        .title(Span::styled(
            format!(" {} ", view.label),
            if focused {
                theme.title()
            } else {
                theme.muted()
            },
        ));
    let content = if view.text.is_empty() && !focused {
        Line::from(Span::styled(view.placeholder, theme.muted()))
    } else {
        Line::from(Span::styled(view.text.clone(), theme.text()))
    };
    f.render_widget(Paragraph::new(content).block(block), input_area);

    if focused && input_area.width > 2 {
        let max = input_area.width.saturating_sub(3);
        let x = input_area.x + 1 + (view.cursor as u16).min(max);
        f.set_cursor_position((x, input_area.y + 1));
    }
    if let Some(error) = view.error
        && area.height > 3
    {
        let line = Rect {
            y: area.y + 3,
            height: 1,
            ..area
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                format!(" ✗ {error}"),
                Style::new().fg(theme.error),
            )),
            line,
        );
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyEventKind;

    use super::*;
    use crate::tui::validate::{AmountValidator, PasswordValidator};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new_with_kind(KeyCode::Char(c), KeyModifiers::CONTROL, KeyEventKind::Press)
    }

    fn typed(text: &str) -> TextInput {
        let mut input = TextInput::default();
        for c in text.chars() {
            input.handle(key(KeyCode::Char(c)));
        }
        input
    }

    #[test]
    fn typing_moving_and_deleting() {
        let mut input = typed("helo");
        input.handle(key(KeyCode::Left));
        input.handle(key(KeyCode::Char('l')));
        assert_eq!(input.value(), "hello");
        input.handle(key(KeyCode::Home));
        input.handle(key(KeyCode::Delete));
        assert_eq!(input.value(), "ello");
        input.handle(key(KeyCode::End));
        input.handle(key(KeyCode::Backspace));
        assert_eq!(input.value(), "ell");
        assert_eq!(input.cursor(), 3);
    }

    #[test]
    fn multibyte_characters_are_safe() {
        let mut input = typed("₿é");
        input.handle(key(KeyCode::Left));
        input.handle(key(KeyCode::Backspace));
        assert_eq!(input.value(), "é");
    }

    #[test]
    fn ctrl_shortcuts_clear_and_delete_words() {
        let mut input = typed("one two three");
        input.handle(ctrl('w'));
        assert_eq!(input.value(), "one two ");
        input.handle(ctrl('u'));
        assert_eq!(input.value(), "");
        assert!(!input.handle(ctrl('x')), "other ctrl keys are ignored");
    }

    #[test]
    fn masked_inputs_hide_text() {
        let mut input = TextInput::masked();
        input.set("secret");
        assert_eq!(input.display(), "••••••");
        assert_eq!(input.value(), "secret");
    }

    #[test]
    fn fields_validate_after_touch_and_on_submit() {
        let mut field = Field::new("Amount", "sats", AmountValidator::default());
        assert_eq!(field.error(), None, "untouched field shows no error");
        assert!(field.submit().is_err());
        assert_eq!(field.error(), Some("enter an amount"));
        field.handle(key(KeyCode::Char('5')));
        assert_eq!(field.error(), None);
        assert_eq!(field.value().unwrap().to_sat(), 5);
        field.reset();
        assert_eq!((field.input.value(), field.error()), ("", None));
    }

    #[test]
    fn server_errors_can_be_attached() {
        let mut field = Field::masked("Password", "", PasswordValidator { min_len: 1 });
        field.set("pw");
        field.set_error("wrong password");
        assert_eq!(field.error(), Some("wrong password"));
        assert_eq!(field.view().text, "••");
    }
}
