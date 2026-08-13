use unicode_width::UnicodeWidthChar;

/// Terminal keys understood by the pure reducer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Key {
    Char(char),
    Ctrl(char),
    Alt(char),
    CtrlLeft,
    CtrlRight,
    AltLeft,
    AltRight,
    AltBackspace,
    AltDelete,
    Paste(String),
    Enter,
    Escape,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Tab,
    BackTab,
    PageUp,
    PageDown,
    F(u8),
}

/// A Unicode-scalar-safe single-line editor. Cursor movement and deletion
/// never split UTF-8, while visible clipping uses terminal cell widths.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InputEditor {
    chars: Vec<char>,
    cursor: usize,
}

impl InputEditor {
    #[must_use]
    pub fn new(value: &str) -> Self {
        let chars: Vec<char> = value.chars().filter_map(sanitize_character).collect();
        let cursor = chars.len();
        Self { chars, cursor }
    }

    #[must_use]
    pub fn value(&self) -> String {
        self.chars.iter().collect()
    }

    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn apply(&mut self, key: &Key) {
        match key {
            Key::Char(character) => {
                if let Some(character) = sanitize_character(*character) {
                    self.chars.insert(self.cursor, character);
                    self.cursor += 1;
                }
            }
            Key::Paste(value) => self.insert_sanitized(value),
            Key::Backspace | Key::Ctrl('h') if self.cursor > 0 => {
                self.cursor -= 1;
                self.chars.remove(self.cursor);
            }
            Key::Delete | Key::Ctrl('d') if self.cursor < self.chars.len() => {
                self.chars.remove(self.cursor);
            }
            Key::Left | Key::Ctrl('b') if self.cursor > 0 => self.cursor -= 1,
            Key::Right | Key::Ctrl('f') if self.cursor < self.chars.len() => self.cursor += 1,
            Key::CtrlLeft | Key::AltLeft | Key::Alt('b') => self.word_backward(),
            Key::CtrlRight | Key::AltRight | Key::Alt('f') => self.word_forward(),
            Key::AltBackspace | Key::Ctrl('w') => self.delete_word_backward(),
            Key::AltDelete | Key::Alt('d') => self.delete_word_forward(),
            Key::Ctrl('k') => self.chars.truncate(self.cursor),
            Key::Ctrl('u') => {
                self.chars.drain(..self.cursor);
                self.cursor = 0;
            }
            Key::Home | Key::Ctrl('a') => self.cursor = 0,
            Key::End | Key::Ctrl('e') => self.cursor = self.chars.len(),
            _ => {}
        }
    }

    fn insert_sanitized(&mut self, value: &str) {
        let sanitized = value.chars().filter_map(sanitize_character);
        for character in sanitized {
            self.chars.insert(self.cursor, character);
            self.cursor += 1;
        }
    }

    fn word_backward(&mut self) {
        while self.cursor > 0 && self.chars[self.cursor - 1].is_whitespace() {
            self.cursor -= 1;
        }
        while self.cursor > 0 && !self.chars[self.cursor - 1].is_whitespace() {
            self.cursor -= 1;
        }
    }

    fn word_forward(&mut self) {
        while self.cursor < self.chars.len() && self.chars[self.cursor].is_whitespace() {
            self.cursor += 1;
        }
        while self.cursor < self.chars.len() && !self.chars[self.cursor].is_whitespace() {
            self.cursor += 1;
        }
    }

    fn delete_word_backward(&mut self) {
        let end = self.cursor;
        self.word_backward();
        self.chars.drain(self.cursor..end);
    }

    fn delete_word_forward(&mut self) {
        let start = self.cursor;
        self.word_forward();
        self.chars.drain(start..self.cursor);
        self.cursor = start;
    }

    #[must_use]
    pub fn visible(&self, width: usize) -> (String, u16) {
        if width == 0 {
            return (String::new(), 0);
        }
        let widths: Vec<usize> = self
            .chars
            .iter()
            .map(|value| value.width().unwrap_or(0))
            .collect();
        let cursor_cells: usize = widths[..self.cursor].iter().sum();
        let mut start = 0;
        let mut cells_before = 0;
        while cursor_cells.saturating_sub(cells_before) >= width && start < self.cursor {
            cells_before += widths[start];
            start += 1;
        }
        let mut used = 0;
        let mut end = start;
        while end < self.chars.len() && used + widths[end] <= width {
            used += widths[end];
            end += 1;
        }
        let visible = self.chars[start..end].iter().collect();
        let cursor = cursor_cells.saturating_sub(cells_before).min(width - 1);
        (visible, u16::try_from(cursor).unwrap_or(u16::MAX))
    }
}

fn sanitize_character(character: char) -> Option<char> {
    match character {
        '\n' | '\r' | '\t' => Some(' '),
        '\u{fffd}' => None,
        value if value.is_control() => None,
        value => Some(value),
    }
}
