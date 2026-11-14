//! Interactive shell sessions for the desktop Terminal panel.
//!
//! Each session is the user's own shell (`pwsh`/`powershell` on Windows,
//! `$SHELL` or `/bin/sh` elsewhere) running on a pseudo terminal
//! (`portable-pty`, ConPTY on Windows). A reader thread feeds the output to a
//! `vt100` parser, which keeps the screen grid and its scrollback; the UI
//! takes [`TerminalScreen`] snapshots of it and redraws when the session's
//! version (a `watch` channel) changes. Input goes through a writer thread so
//! a stalled child never blocks the UI.
//!
//! This is not an agent tool: no permission gating, the user's normal
//! environment, cwd = the project folder. Sessions live in one process-wide
//! registry ([`TerminalSessions::global`]) so they survive the panel being
//! hidden; [`TerminalSessions::close_all`] kills them on app exit.

use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};

/// Lines of history kept above the visible screen.
const SCROLLBACK_LINES: usize = 5000;
/// Smallest grid a session is sized to (guards against a collapsed panel).
const MIN_COLS: u16 = 10;
const MIN_ROWS: u16 = 2;

/// A cell colour as the terminal program asked for it; the UI maps it to
/// its own palette (`Indexed(0..16)` are the themable ANSI colours).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum TerminalColor {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl From<vt100::Color> for TerminalColor {
    fn from(color: vt100::Color) -> Self {
        match color {
            vt100::Color::Default => TerminalColor::Default,
            vt100::Color::Idx(index) => TerminalColor::Indexed(index),
            vt100::Color::Rgb(r, g, b) => TerminalColor::Rgb(r, g, b),
        }
    }
}

/// A run of adjacent cells on one row that share the same style.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSpan {
    pub text: String,
    pub fg: TerminalColor,
    pub bg: TerminalColor,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    /// This span is the single cell under the visible cursor.
    pub cursor: bool,
}

impl TerminalSpan {
    fn same_style(&self, other: &TerminalSpan) -> bool {
        self.fg == other.fg
            && self.bg == other.bg
            && self.bold == other.bold
            && self.dim == other.dim
            && self.italic == other.italic
            && self.underline == other.underline
            && self.inverse == other.inverse
            && !self.cursor
            && !other.cursor
    }
}

/// What the UI draws for one session.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalScreen {
    pub version: u64,
    pub rows: Vec<Vec<TerminalSpan>>,
    pub cols: u16,
    /// Lines scrolled back into history (0 = live screen).
    pub scroll_offset: usize,
    /// Arrow keys must be sent as `ESC O x` (DECCKM).
    pub application_cursor: bool,
    /// Pastes must be wrapped in `ESC [200~ … ESC [201~`.
    pub bracketed_paste: bool,
    /// Full-screen programs (vim, less) use the alternate screen.
    pub alternate_screen: bool,
    pub exited: bool,
}

/// One open shell, as listed for the tab strip.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub id: String,
    pub project_dir: String,
    /// Window title set by the shell (OSC 0/2), else the shell's name.
    pub title: String,
    pub exited: bool,
}

/// Answers the queries a shell sends to its terminal (cursor position,
/// device attributes) and records the window title.
#[derive(Default)]
struct TerminalReplies {
    pending: Vec<u8>,
    title: Option<String>,
}

impl vt100::Callbacks for TerminalReplies {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        let title = String::from_utf8_lossy(title).trim().to_string();
        self.title = (!title.is_empty()).then_some(title);
    }

    fn set_window_icon_name(&mut self, screen: &mut vt100::Screen, name: &[u8]) {
        self.set_window_title(screen, name);
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, c, first) {
            // DSR: cursor position report. ConPTY asks for it at startup and
            // waits for the answer before the shell prints anything.
            (None, 'n', 6) => {
                let (row, col) = screen.cursor_position();
                self.pending
                    .extend_from_slice(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
            }
            // DSR: "terminal OK".
            (None, 'n', 5) => self.pending.extend_from_slice(b"\x1b[0n"),
            // DA1: a VT220-class terminal with ANSI colour.
            (None, 'c', 0) => self.pending.extend_from_slice(b"\x1b[?62;22c"),
            _ => {}
        }
    }
}

/// State shared between a session handle and its reader / waiter threads.
struct SessionShared {
    parser: Mutex<vt100::Parser<TerminalReplies>>,
    version: tokio::sync::watch::Sender<u64>,
    counter: AtomicU64,
    exited: AtomicBool,
}

impl SessionShared {
    fn bump(&self) {
        let next = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        self.version.send_replace(next);
    }
}

struct TerminalSession {
    project_dir: String,
    shell_name: String,
    shared: Arc<SessionShared>,
    input: mpsc::Sender<Vec<u8>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

/// Registry of open shell sessions.
#[derive(Default)]
pub struct TerminalSessions {
    sessions: Mutex<HashMap<String, Arc<TerminalSession>>>,
    /// Open order, so tabs keep their position.
    order: Mutex<Vec<String>>,
}

impl TerminalSessions {
    /// The process-wide registry used by the api facade.
    pub fn global() -> &'static TerminalSessions {
        static GLOBAL: OnceLock<TerminalSessions> = OnceLock::new();
        GLOBAL.get_or_init(TerminalSessions::default)
    }

    /// Starts the default shell in `project_dir` on a `cols` x `rows` pty.
    pub fn open(&self, project_dir: &Path, cols: u16, rows: u16) -> anyhow::Result<String> {
        let (cols, rows) = clamp_size(cols, rows);
        let pair = native_pty_system().openpty(pty_size(cols, rows))?;
        let shell = default_shell();
        let mut command = CommandBuilder::new(&shell);
        for arg in shell_args(&shell) {
            command.arg(arg);
        }
        command.cwd(project_dir);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        command.env("TERM_PROGRAM", "Locagens");
        let mut child = pair.slave.spawn_command(command)?;
        // The slave end belongs to the child now; keeping it open would hide
        // the child's exit from the reader on Unix.
        drop(pair.slave);
        let killer = child.clone_killer();
        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        let (version, _) = tokio::sync::watch::channel(0);
        let shared = Arc::new(SessionShared {
            parser: Mutex::new(vt100::Parser::new_with_callbacks(
                rows,
                cols,
                SCROLLBACK_LINES,
                TerminalReplies::default(),
            )),
            version,
            counter: AtomicU64::new(0),
            exited: AtomicBool::new(false),
        });
        let (input, input_rx) = mpsc::channel::<Vec<u8>>();
        spawn_writer(writer, input_rx);
        spawn_reader(reader, shared.clone(), input.clone());
        {
            let shared = shared.clone();
            std::thread::spawn(move || {
                let _ = child.wait();
                shared.exited.store(true, Ordering::SeqCst);
                shared.bump();
            });
        }

        let id = uuid::Uuid::new_v4().to_string();
        let session = TerminalSession {
            project_dir: project_dir.to_string_lossy().to_string(),
            shell_name: shell_display_name(&shell),
            shared,
            input,
            master: Mutex::new(pair.master),
            killer: Mutex::new(killer),
        };
        lock(&self.sessions).insert(id.clone(), Arc::new(session));
        lock(&self.order).push(id.clone());
        Ok(id)
    }

    fn get(&self, id: &str) -> Option<Arc<TerminalSession>> {
        lock(&self.sessions).get(id).cloned()
    }

    /// Open sessions, oldest first, optionally only those of `project_dir`.
    pub fn list(&self, project_dir: Option<&Path>) -> Vec<TerminalInfo> {
        let wanted = project_dir.map(|dir| dir.to_string_lossy().to_string());
        let order = lock(&self.order).clone();
        order
            .into_iter()
            .filter_map(|id| self.get(&id).map(|session| (id, session)))
            .filter(|(_, session)| wanted.as_ref().is_none_or(|dir| *dir == session.project_dir))
            .map(|(id, session)| {
                let title = lock(&session.shared.parser).callbacks().title.clone();
                TerminalInfo {
                    id,
                    project_dir: session.project_dir.clone(),
                    title: title.map(|title| tab_title(&title)).unwrap_or_else(|| session.shell_name.clone()),
                    exited: session.shared.exited.load(Ordering::SeqCst),
                }
            })
            .collect()
    }

    /// Sends keyboard / paste bytes to the shell and returns the view to the
    /// live screen. `false` when the session is unknown.
    pub fn write(&self, id: &str, bytes: &[u8]) -> bool {
        let Some(session) = self.get(id) else { return false };
        {
            let mut parser = lock(&session.shared.parser);
            if parser.screen().scrollback() != 0 {
                parser.screen_mut().set_scrollback(0);
                drop(parser);
                session.shared.bump();
            }
        }
        session.input.send(bytes.to_vec()).is_ok()
    }

    /// Resizes the pty and the screen grid.
    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> anyhow::Result<()> {
        let Some(session) = self.get(id) else { anyhow::bail!("unknown terminal {id}") };
        let (cols, rows) = clamp_size(cols, rows);
        {
            let mut parser = lock(&session.shared.parser);
            if parser.screen().size() == (rows, cols) {
                return Ok(());
            }
            parser.screen_mut().set_size(rows, cols);
        }
        lock(&session.master).resize(pty_size(cols, rows))?;
        session.shared.bump();
        Ok(())
    }

    /// Scrolls the view `lines` into history (positive = older output).
    pub fn scroll(&self, id: &str, lines: i32) {
        let Some(session) = self.get(id) else { return };
        let mut parser = lock(&session.shared.parser);
        let current = parser.screen().scrollback() as i64;
        let target = (current + i64::from(lines)).max(0) as usize;
        parser.screen_mut().set_scrollback(target);
        let changed = parser.screen().scrollback() as i64 != current;
        drop(parser);
        if changed {
            session.shared.bump();
        }
    }

    /// Current version of the session's screen; changes on every redraw-worthy event.
    pub fn version(&self, id: &str) -> Option<u64> {
        self.get(id).map(|session| *session.shared.version.borrow())
    }

    /// A receiver that wakes whenever the session's screen changes.
    pub fn subscribe(&self, id: &str) -> Option<tokio::sync::watch::Receiver<u64>> {
        self.get(id).map(|session| session.shared.version.subscribe())
    }

    /// The visible screen (taking the scroll position into account).
    pub fn snapshot(&self, id: &str) -> Option<TerminalScreen> {
        let session = self.get(id)?;
        let version = *session.shared.version.borrow();
        let parser = lock(&session.shared.parser);
        let mut screen = screen_snapshot(parser.screen());
        screen.version = version;
        screen.exited = session.shared.exited.load(Ordering::SeqCst);
        Some(screen)
    }

    /// Kills the shell and releases its pty. `false` when unknown.
    pub fn close(&self, id: &str) -> bool {
        let Some(session) = lock(&self.sessions).remove(id) else { return false };
        lock(&self.order).retain(|open| open != id);
        let _ = lock(&session.killer).kill();
        // Dropping the master closes the pseudo console, which ends the
        // reader thread (and on Windows can block until it drained), so it
        // happens off the caller's thread.
        std::thread::spawn(move || drop(session));
        true
    }

    /// Closes every session (app exit).
    pub fn close_all(&self) {
        let ids: Vec<String> = lock(&self.order).clone();
        for id in ids {
            self.close(&id);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn clamp_size(cols: u16, rows: u16) -> (u16, u16) {
    (cols.max(MIN_COLS), rows.max(MIN_ROWS))
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }
}

fn spawn_writer(mut writer: Box<dyn Write + Send>, input: mpsc::Receiver<Vec<u8>>) {
    std::thread::spawn(move || {
        while let Ok(bytes) = input.recv() {
            if writer.write_all(&bytes).and_then(|_| writer.flush()).is_err() {
                break;
            }
        }
    });
}

fn spawn_reader(mut reader: Box<dyn Read + Send>, shared: Arc<SessionShared>, input: mpsc::Sender<Vec<u8>>) {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 16 * 1024];
        loop {
            let read = match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            let replies = {
                let mut parser = lock(&shared.parser);
                parser.process(&buffer[..read]);
                std::mem::take(&mut parser.callbacks_mut().pending)
            };
            if !replies.is_empty() {
                let _ = input.send(replies);
            }
            shared.bump();
        }
        shared.exited.store(true, Ordering::SeqCst);
        shared.bump();
    });
}

/// Turns the visible grid into styled runs; the cell under the cursor is a
/// span of its own (`cursor: true`) unless the cursor is hidden or the view
/// is scrolled into history.
fn screen_snapshot(screen: &vt100::Screen) -> TerminalScreen {
    let (rows, cols) = screen.size();
    let scroll_offset = screen.scrollback();
    let cursor = (scroll_offset == 0 && !screen.hide_cursor()).then(|| screen.cursor_position());
    let mut lines = Vec::with_capacity(rows as usize);
    for row in 0..rows {
        let mut spans: Vec<TerminalSpan> = Vec::new();
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else { continue };
            if cell.is_wide_continuation() {
                continue;
            }
            let text = if cell.has_contents() { cell.contents() } else { " " };
            let span = TerminalSpan {
                text: String::new(),
                fg: cell.fgcolor().into(),
                bg: cell.bgcolor().into(),
                bold: cell.bold(),
                dim: cell.dim(),
                italic: cell.italic(),
                underline: cell.underline(),
                inverse: cell.inverse(),
                cursor: cursor == Some((row, col)),
            };
            match spans.last_mut() {
                Some(last) if last.same_style(&span) => last.text.push_str(text),
                _ => spans.push(TerminalSpan { text: text.to_string(), ..span }),
            }
        }
        lines.push(spans);
    }
    TerminalScreen {
        version: 0,
        rows: lines,
        cols,
        scroll_offset,
        application_cursor: screen.application_cursor(),
        bracketed_paste: screen.bracketed_paste(),
        alternate_screen: screen.alternate_screen(),
        exited: false,
    }
}

/// `pwsh.exe` when installed, else Windows PowerShell; `$SHELL` or `/bin/sh` on Unix.
fn default_shell() -> String {
    if cfg!(windows) {
        match find_on_path("pwsh.exe") {
            Some(path) => path.to_string_lossy().to_string(),
            None => "powershell.exe".to_string(),
        }
    } else {
        std::env::var("SHELL")
            .ok()
            .filter(|shell| !shell.trim().is_empty())
            .unwrap_or_else(|| "/bin/sh".to_string())
    }
}

fn shell_args(shell: &str) -> &'static [&'static str] {
    let name = shell_display_name(shell).to_ascii_lowercase();
    if name == "pwsh" || name == "powershell" {
        &["-NoLogo"]
    } else {
        &[]
    }
}

/// A shell-set title for a tab: a bare executable path (ConPTY titles the
/// console with it) reads as its name; anything else is kept.
fn tab_title(title: &str) -> String {
    let executable = title.to_ascii_lowercase().ends_with(".exe");
    let unix_path = title.starts_with('/') && !title.contains(' ');
    if !executable && !unix_path {
        return title.to_string();
    }
    let name = title.rsplit(['\\', '/']).next().unwrap_or(title);
    let stem = if executable { &name[..name.len() - 4] } else { name };
    stem.to_string()
}

/// `C:\...\pwsh.exe` -> `pwsh`, `/bin/zsh` -> `zsh`.
fn shell_display_name(shell: &str) -> String {
    Path::new(shell)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| shell.to_string())
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn screen_text(sessions: &TerminalSessions, id: &str) -> String {
        let screen = sessions.snapshot(id).expect("session");
        screen
            .rows
            .iter()
            .map(|row| row.iter().map(|span| span.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if done() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("timed out waiting for {what}");
    }

    #[test]
    fn shell_echoes_output_resizes_and_closes() {
        let sessions = TerminalSessions::default();
        let dir = tempfile::tempdir().unwrap();
        let id = sessions.open(dir.path(), 80, 24).expect("open shell");
        assert_eq!(sessions.list(Some(dir.path())).len(), 1);
        assert!(sessions.list(Some(Path::new("/elsewhere"))).is_empty());

        let mut rx = sessions.subscribe(&id).unwrap();
        // The marker is assembled by the shell (`$((6*7))` is arithmetic in
        // sh and a subexpression in PowerShell), so the echoed command line
        // alone cannot satisfy the check.
        assert!(sessions.write(&id, b"echo locagens$((6*7))marker\r"));
        if cfg!(windows) {
            assert!(sessions.write(&id, b"echo ('locagens' + (6*7) + 'marker')\r"));
        }
        wait_for("shell output", || screen_text(&sessions, &id).contains("locagens42marker"));
        assert!(*rx.borrow_and_update() > 0);

        let before = sessions.version(&id).unwrap();
        sessions.resize(&id, 100, 30).unwrap();
        let screen = sessions.snapshot(&id).unwrap();
        assert_eq!((screen.cols, screen.rows.len()), (100, 30));
        assert!(sessions.version(&id).unwrap() > before);

        assert!(sessions.close(&id));
        assert!(!sessions.close(&id));
        assert!(sessions.snapshot(&id).is_none());
        assert!(sessions.list(None).is_empty());
    }

    #[test]
    fn replies_answer_cursor_position_queries() {
        let mut parser = vt100::Parser::new_with_callbacks(5, 20, 0, TerminalReplies::default());
        parser.process(b"ab\x1b[6n\x1b]0;my title\x07");
        assert_eq!(parser.callbacks().pending, b"\x1b[1;3R");
        assert_eq!(parser.callbacks().title.as_deref(), Some("my title"));
    }

    #[test]
    fn snapshot_merges_styles_and_marks_the_cursor() {
        let mut parser = vt100::Parser::new(2, 10, 10);
        parser.process(b"\x1b[31mred\x1b[0m ok");
        let screen = screen_snapshot(parser.screen());
        let row = &screen.rows[0];
        assert_eq!(row[0].text, "red");
        assert_eq!(row[0].fg, TerminalColor::Indexed(1));
        assert_eq!(row[1].text, " ok");
        assert!(row[2].cursor);
        assert_eq!(row.iter().map(|s| s.text.chars().count()).sum::<usize>(), 10);
    }

    #[test]
    fn tab_titles_shorten_executable_paths() {
        assert_eq!(tab_title("C:\\Program Files\\PowerShell\\7\\pwsh.exe"), "pwsh");
        assert_eq!(tab_title("/usr/bin/zsh"), "zsh");
        assert_eq!(tab_title("user@host: ~/repo"), "user@host: ~/repo");
    }

    #[test]
    fn scrolling_is_clamped_to_history() {
        let mut parser = vt100::Parser::new(2, 10, 100);
        parser.process(b"1\r\n2\r\n3\r\n4");
        parser.screen_mut().set_scrollback(50);
        let screen = screen_snapshot(parser.screen());
        assert_eq!(screen.scroll_offset, 2);
        assert!(screen.rows.iter().flatten().all(|span| !span.cursor));
    }
}
