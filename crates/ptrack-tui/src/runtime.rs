use std::fmt;
use std::io::{self, IsTerminal, stdout};
use std::time::Duration;

use ptrack_app::ApplicationPort;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::cursor::{Hide, Show};
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

use crate::input::Key;
use crate::model::{Effect, Model, RuntimeContext};
use crate::reducer::{success_message, update};
use crate::render::draw;

#[derive(Debug)]
pub enum RuntimeError {
    NotTerminal,
    Application(ptrack_app::AppError),
    Io(io::Error),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotTerminal => {
                formatter.write_str("terminal UI requires an interactive terminal")
            }
            Self::Application(error) => error.fmt(formatter),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<io::Error> for RuntimeError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ptrack_app::AppError> for RuntimeError {
    fn from(error: ptrack_app::AppError) -> Self {
        Self::Application(error)
    }
}

pub(crate) struct TerminalMode {
    raw: bool,
    alternate: bool,
    cleanup: Box<dyn FnMut(bool, bool)>,
}

impl TerminalMode {
    fn enter() -> io::Result<Self> {
        Self::enter_with(
            enable_raw_mode,
            || execute!(stdout(), EnterAlternateScreen, EnableBracketedPaste, Hide),
            Box::new(restore_terminal),
        )
    }

    pub(crate) fn enter_with(
        enable_raw: impl FnOnce() -> io::Result<()>,
        setup_screen: impl FnOnce() -> io::Result<()>,
        cleanup: Box<dyn FnMut(bool, bool)>,
    ) -> io::Result<Self> {
        enable_raw()?;
        let mode = Self {
            raw: true,
            // The combined setup write may fail after changing one or more
            // terminal modes, so cleanup owns every screen capability before
            // the first command is attempted.
            alternate: true,
            cleanup,
        };
        setup_screen()?;
        Ok(mode)
    }
}

impl Drop for TerminalMode {
    fn drop(&mut self) {
        (self.cleanup)(self.raw, self.alternate);
    }
}

fn restore_terminal(raw: bool, alternate: bool) {
    if alternate {
        // Keep cleanup commands independent so one failed terminal write does
        // not prevent the remaining restoration attempts.
        let _ = execute!(stdout(), Show);
        let _ = execute!(stdout(), DisableBracketedPaste);
        let _ = execute!(stdout(), LeaveAlternateScreen);
    }
    if raw {
        let _ = disable_raw_mode();
    }
}

/// Runs the TUI. Application operations occur only while applying explicit
/// effects; the event loop retains no database or filesystem handle while idle.
///
/// # Errors
///
/// Returns an error when stdin/stdout are not interactive, the initial
/// application snapshot fails, or terminal setup/event handling fails.
pub fn run(
    application: &mut dyn ApplicationPort,
    context: RuntimeContext,
) -> Result<(), RuntimeError> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(RuntimeError::NotTerminal);
    }
    let snapshot = application.snapshot()?;
    let mut model = Model::new(snapshot, context);
    let _mode = TerminalMode::enter()?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    loop {
        let size = terminal.size()?;
        model.resize(size.width, size.height);
        terminal.draw(|frame| draw(frame, &model))?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let key = match event::read()? {
            Event::Key(event)
                if matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat) =>
            {
                let Some(key) = translate_key(event) else {
                    continue;
                };
                key
            }
            Event::Paste(value) => Key::Paste(value),
            _ => continue,
        };
        if let Some(effect) = update(&mut model, &key)
            && apply_effect(application, &mut model, effect)
        {
            break;
        }
    }
    Ok(())
}

pub(crate) fn apply_effect(
    application: &mut dyn ApplicationPort,
    model: &mut Model,
    effect: Effect,
) -> bool {
    match effect {
        Effect::Quit => return true,
        Effect::Reload {
            success,
            reopen_detail,
        } => match application.snapshot() {
            Ok(snapshot) => {
                model.replace_snapshot(snapshot);
                if reopen_detail {
                    model.detail = model.selected_detail();
                    model.detail_offset = 0;
                }
                model.status = success;
            }
            Err(error) => model.status = error.to_string(),
        },
        Effect::Backup => match application.backup() {
            Ok(path) => model.status = format!("backed up → {}", path.display()),
            Err(error) => model.status = format!("backup error: {error}"),
        },
        Effect::Mutate { mutation, success } => match application.mutate(mutation) {
            Ok(result) => {
                let Some(message) = success_message(&success, &result) else {
                    "application returned an unexpected mutation result"
                        .clone_into(&mut model.status);
                    return false;
                };
                match application.snapshot() {
                    Ok(snapshot) => {
                        if let crate::model::Success::MovedCard { column, .. } = &success {
                            model.board_col = *column;
                        }
                        model.replace_snapshot(snapshot);
                        model.status = message;
                    }
                    Err(error) => model.status = format!("change saved; reload failed: {error}"),
                }
            }
            Err(error) => model.status = error.to_string(),
        },
    }
    false
}

fn translate_key(event: KeyEvent) -> Option<Key> {
    let control = event.modifiers.contains(KeyModifiers::CONTROL);
    let alt = event.modifiers.contains(KeyModifiers::ALT);
    match (control, alt, event.code) {
        (true, _, KeyCode::Char(character)) => return Some(Key::Ctrl(character)),
        (false, true, KeyCode::Char(character)) => return Some(Key::Alt(character)),
        (true, _, KeyCode::Left) => return Some(Key::CtrlLeft),
        (true, _, KeyCode::Right) => return Some(Key::CtrlRight),
        (false, true, KeyCode::Left) => return Some(Key::AltLeft),
        (false, true, KeyCode::Right) => return Some(Key::AltRight),
        (false, true, KeyCode::Backspace) => return Some(Key::AltBackspace),
        (false, true, KeyCode::Delete) => return Some(Key::AltDelete),
        _ => {}
    }
    match event.code {
        KeyCode::Char(character) => Some(Key::Char(character)),
        KeyCode::Enter => Some(Key::Enter),
        KeyCode::Esc => Some(Key::Escape),
        KeyCode::Backspace => Some(Key::Backspace),
        KeyCode::Delete => Some(Key::Delete),
        KeyCode::Left => Some(Key::Left),
        KeyCode::Right => Some(Key::Right),
        KeyCode::Up => Some(Key::Up),
        KeyCode::Down => Some(Key::Down),
        KeyCode::Home => Some(Key::Home),
        KeyCode::End => Some(Key::End),
        KeyCode::Tab => Some(Key::Tab),
        KeyCode::BackTab => Some(Key::BackTab),
        KeyCode::PageUp => Some(Key::PageUp),
        KeyCode::PageDown => Some(Key::PageDown),
        KeyCode::F(value) => Some(Key::F(value)),
        _ => None,
    }
}
