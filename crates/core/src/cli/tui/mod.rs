//! Codex-style full-screen terminal UI for `lcgn` (alternate screen,
//! scrollable transcript, composer, approval overlays).
//!
//! `app` holds the terminal-free state and event reducer, `driver` performs
//! the API calls, `ui` draws; this file owns the terminal and event loop.

mod app;
mod composer;
mod diff;
mod driver;
mod highlight;
mod markdown;
mod overlay;
mod popup;
mod transcript;
mod ui;
mod wrap;

use crate::cli::repl::ReplConfig;
use crate::state::AppState;
use app::App;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{cursor::Show, execute};
use driver::{Driver, DriverConfig};
use futures::StreamExt as _;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, Stdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Redraw tick (~30 fps); redraws are coalesced to at most one per tick.
const TICK: Duration = Duration::from_millis(33);
/// Windows consoles have no bracketed paste: an Enter arriving this soon
/// after a typed character is part of a paste, not a submit.
const PASTE_BURST_WINDOW: Duration = Duration::from_millis(8);

static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

type Tui = Terminal<CrosstermBackend<Stdout>>;

fn enter_terminal() -> io::Result<Tui> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    if supports_keyboard_enhancement().unwrap_or(false)
        && execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok()
    {
        KEYBOARD_ENHANCED.store(true, Ordering::SeqCst);
    }
    Terminal::new(CrosstermBackend::new(out))
}

/// Puts the terminal back; safe to call more than once (also from the
/// panic hook).
fn restore_terminal() {
    let mut out = io::stdout();
    if KEYBOARD_ENHANCED.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, DisableBracketedPaste, DisableMouseCapture, LeaveAlternateScreen, Show);
    let _ = disable_raw_mode();
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    let ui_thread = std::thread::current().id();
    std::panic::set_hook(Box::new(move |info| {
        // A panic in a background task must not tear the screen down under
        // the still-running UI: log it, restore only for the UI thread.
        if std::thread::current().id() == ui_thread {
            restore_terminal();
            previous(info);
        } else {
            tracing::error!("panic in a background thread: {info}");
        }
    }));
}

/// Runs the TUI until the user quits; returns the process exit code.
pub async fn run(state: Arc<AppState>, config: ReplConfig) -> i32 {
    let mut terminal = match enter_terminal() {
        Ok(terminal) => terminal,
        Err(err) => {
            restore_terminal();
            eprintln!("lcgn: cannot start the terminal UI: {err} (try --no-tui)");
            return 2;
        }
    };
    install_panic_hook();
    let result = event_loop(&mut terminal, state, config).await;
    restore_terminal();
    let _ = terminal.show_cursor();
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("lcgn: terminal error: {err}");
            1
        }
    }
}

async fn event_loop(terminal: &mut Tui, state: Arc<AppState>, config: ReplConfig) -> io::Result<()> {
    let (files_tx, mut files_rx) = tokio::sync::mpsc::unbounded_channel();
    crate::cli::runner::ensure_project(&state, &config.project);
    let app = App::new(
        config.project.clone(),
        config.model.clone(),
        config.thinking.clone(),
        config.mode.clone(),
        config.bypass,
        config.initial_session.clone(),
    );
    let mut driver = Driver::new(
        state,
        app,
        DriverConfig {
            no_session: config.no_session,
            system_prompt: config.system_prompt.clone(),
            append_system_prompt: config.append_system_prompt.clone(),
        },
        files_tx,
    );
    driver.load_model_choices().await;
    match &config.initial_session {
        Some(id) => driver.load_session(id).await,
        None if driver.app.model.is_none() => driver
            .app
            .push_notice("No model selected. Type /model and pick one from the list."),
        None => {}
    }

    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_char_at: Option<Instant> = None;
    loop {
        tokio::select! {
            maybe_event = events.next() => match maybe_event {
                Some(Ok(event)) => handle_terminal_event(&mut driver, event, &mut last_char_at).await,
                Some(Err(err)) => return Err(err),
                None => break,
            },
            Some(received) = driver.recv_run() => driver.on_run_recv(received).await,
            Some((query, files)) = files_rx.recv() => driver.app.set_file_matches(&query, &files),
            _ = ticker.tick() => {
                driver.app.tick();
                if driver.app.dirty {
                    driver.app.dirty = false;
                    terminal.draw(|frame| ui::draw(frame, &mut driver.app))?;
                }
            }
        }
        if driver.app.should_quit {
            break;
        }
    }
    Ok(())
}

async fn handle_terminal_event(driver: &mut Driver, event: Event, last_char_at: &mut Option<Instant>) {
    let actions = match event {
        Event::Key(key) => driver.app.on_key(normalize_key(key, last_char_at)),
        Event::Paste(text) => driver.app.on_paste(&text),
        Event::Mouse(mouse) => {
            driver.app.on_mouse(mouse);
            Vec::new()
        }
        Event::Resize(..) => {
            driver.app.dirty = true;
            Vec::new()
        }
        _ => Vec::new(),
    };
    for action in actions {
        driver.perform(action).await;
    }
}

/// Turns an Enter that directly follows typed characters into Shift+Enter
/// (paste on consoles without bracketed paste) and tracks typing time.
fn normalize_key(mut key: KeyEvent, last_char_at: &mut Option<Instant>) -> KeyEvent {
    if key.kind == KeyEventKind::Release {
        return key;
    }
    let now = Instant::now();
    let in_burst = last_char_at.is_some_and(|at| now.duration_since(at) < PASTE_BURST_WINDOW);
    if cfg!(windows) && in_burst && key.code == KeyCode::Enter && key.modifiers.is_empty() {
        key.modifiers = KeyModifiers::SHIFT;
        *last_char_at = Some(now);
    } else if composer::printable_char(&key).is_some() {
        *last_char_at = Some(now);
    } else {
        *last_char_at = None;
    }
    key
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn enter_right_after_typing_becomes_a_newline_on_windows() {
        let mut last = None;
        normalize_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE), &mut last);
        let enter = normalize_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut last);
        assert!(enter.modifiers.contains(KeyModifiers::SHIFT));
        std::thread::sleep(PASTE_BURST_WINDOW * 3);
        let later = normalize_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut last);
        assert!(later.modifiers.is_empty());
    }
}
