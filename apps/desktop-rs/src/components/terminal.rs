//! Interactive terminal panel: one tab per shell session of the project
//! (`locagens_core::api::terminal`), a compact tab strip and the active
//! session's screen.
//!
//! The screen is rendered from core snapshots (styled spans per row) and
//! redrawn when the session's version changes. Input is captured by one
//! script per panel ([`VIEW_SCRIPT`]): keys go to a hidden textarea (so
//! paste and IME composition work), are described to Rust and translated to
//! bytes by [`key_bytes`]; the script also measures the cell size to fit the
//! grid, turns the wheel into scrolling and hands selections to the
//! clipboard. Sessions outlive the panel (hidden panel, project switch);
//! `api::terminal::close_all` ends them on app exit.

use crate::state_ui::UiStore;
use dioxus::prelude::*;
use locagens_core::api::terminal::{self, TerminalColor, TerminalInfo, TerminalScreen, TerminalSpan};
use serde::Deserialize;
use std::sync::atomic::{AtomicU32, Ordering};

/// Grid size used until the view has been measured.
const DEFAULT_SIZE: (u16, u16) = (80, 24);
/// Pause after a redraw so output bursts coalesce into one frame.
const REDRAW_COALESCE_MS: u64 = 8;
/// Arrow presses sent per wheel line in full-screen programs, at most.
const MAX_WHEEL_ARROWS: i32 = 10;

static NEXT_PANEL: AtomicU32 = AtomicU32::new(1);

/// What the view script reports.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "t", rename_all = "lowercase")]
enum ViewEvent {
    Key { key: String, ctrl: bool, alt: bool, shift: bool },
    Text { text: String },
    Paste { text: String },
    Copy { text: String },
    Wheel { lines: i32 },
    Resize { cols: u16, rows: u16 },
}

#[component]
pub fn TerminalPanel(project: Option<String>) -> Element {
    let store: UiStore = use_context();
    let panel = use_hook(|| NEXT_PANEL.fetch_add(1, Ordering::Relaxed));
    let view_id = format!("lcg-term-view-{panel}");
    let input_id = format!("lcg-term-input-{panel}");
    let measure_id = format!("lcg-term-measure-{panel}");

    let mut tabs: Signal<Vec<TerminalInfo>> = use_signal(Vec::new);
    let mut active: Signal<Option<String>> = use_signal(|| None);
    let mut screen: Signal<Option<TerminalScreen>> = use_signal(|| None);
    let size = use_signal(|| DEFAULT_SIZE);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut pump: Signal<Option<dioxus::core::Task>> = use_signal(|| None);
    let project_dir = use_signal(|| project.clone());

    // Project changed (or first mount): show its shells, opening one when
    // it has none yet.
    use_effect(use_reactive!(|project| {
        let mut project_dir = project_dir;
        project_dir.set(project.clone());
        error.set(None);
        let Some(dir) = project.clone() else {
            tabs.set(Vec::new());
            active.set(None);
            return;
        };
        let mut open = terminal::list(Some(&dir));
        if open.is_empty() {
            let (cols, rows) = *size.peek();
            match terminal::open(&dir, cols, rows) {
                Ok(_) => open = terminal::list(Some(&dir)),
                Err(err) => error.set(Some(err.message.clone())),
            }
        }
        active.set(open.last().map(|tab| tab.id.clone()));
        tabs.set(open);
    }));

    // Redraw the active session whenever its version changes.
    use_effect(move || {
        let id = active.read().clone();
        if let Some(task) = pump.write().take() {
            task.cancel();
        }
        screen.set(id.as_deref().and_then(terminal::snapshot));
        let Some(id) = id else { return };
        focus_input(&format!("lcg-term-input-{panel}"));
        let task = spawn(async move {
            let Some(mut changes) = terminal::subscribe(&id) else { return };
            while changes.changed().await.is_ok() {
                screen.set(terminal::snapshot(&id));
                refresh_tabs(tabs, project_dir);
                tokio::time::sleep(std::time::Duration::from_millis(REDRAW_COALESCE_MS)).await;
            }
        });
        pump.set(Some(task));
    });

    // Input, measuring, wheel and copy events from the view script.
    {
        let script = VIEW_SCRIPT
            .replace("VIEW_ID", &view_id)
            .replace("INPUT_ID", &input_id)
            .replace("MEASURE_ID", &measure_id);
        use_future(move || {
            let script = script.clone();
            async move {
                let mut channel = dioxus::document::eval(&script);
                while let Ok(event) = channel.recv::<ViewEvent>().await {
                    handle_view_event(event, active, screen, tabs, size);
                }
            }
        });
    }

    let open_new = move |_| {
        let Some(dir) = project_dir.peek().clone() else { return };
        let (cols, rows) = *size.peek();
        match terminal::open(&dir, cols, rows) {
            Ok(id) => {
                error.set(None);
                refresh_tabs(tabs, project_dir);
                active.set(Some(id));
            }
            Err(err) => error.set(Some(err.message.clone())),
        }
    };

    let current_tabs = tabs.read().clone();
    let active_id = active.read().clone();
    let current = screen.read().clone();
    let has_project = project.is_some();

    rsx! {
        style { {TERMINAL_CSS} }
        div {
            class: "terminal-panel",
            if has_project {
                div {
                    class: "term-tabs",
                    role: "tablist",
                    "aria-label": store.t("terminal.tabs"),
                    for tab in current_tabs.iter().cloned() {
                        div {
                            key: "{tab.id}",
                            class: if active_id.as_deref() == Some(tab.id.as_str()) { "term-tab active" } else { "term-tab" },
                            class: if tab.exited { "exited" },
                            role: "tab",
                            "aria-selected": active_id.as_deref() == Some(tab.id.as_str()),
                            title: "{tab.title}",
                            onclick: {
                                let id = tab.id.clone();
                                move |_| active.set(Some(id.clone()))
                            },
                            span { class: "term-tab-title", "{tab.title}" }
                            button {
                                class: "term-tab-close",
                                title: store.t("terminal.close"),
                                onclick: {
                                    let id = tab.id.clone();
                                    move |event: Event<MouseData>| {
                                        event.stop_propagation();
                                        close_tab(&id, tabs, active, project_dir);
                                    }
                                },
                                {super::icons::icon_x()}
                            }
                        }
                    }
                    button {
                        class: "term-new",
                        title: store.t("terminal.new"),
                        onclick: open_new,
                        {super::icons::icon_plus()}
                    }
                }
            }
            div {
                id: "{view_id}",
                class: "term-view",
                tabindex: "-1",
                textarea {
                    id: "{input_id}",
                    class: "term-input",
                    autocomplete: "off",
                    spellcheck: "false",
                    "autocorrect": "off",
                    "autocapitalize": "off",
                    "aria-label": store.t("terminal.input"),
                }
                span { id: "{measure_id}", class: "term-measure", "aria-hidden": "true", "{MEASURE_TEXT}" }
                if let Some(message) = error.read().as_ref() {
                    div { class: "term-note", "{store.tf(\"terminal.open_failed\", &[message.as_str()])}" }
                } else if !has_project {
                    div { class: "term-note", "{store.t(\"terminal.no_project\")}" }
                } else if let Some(current) = current {
                    div {
                        class: "term-rows",
                        for (index, row) in current.rows.iter().enumerate() {
                            // One HTML string per row: Dioxus does not reliably
                            // patch changing span lists inside reused rows (a
                            // moved highlight kept its old style).
                            div {
                                key: "{index}",
                                class: "term-row",
                                dangerous_inner_html: row_html(row),
                            }
                        }
                    }
                    if current.exited {
                        div { class: "term-note term-exited", "{store.t(\"terminal.exited\")}" }
                    }
                } else if current_tabs.is_empty() {
                    div { class: "term-note", "{store.t(\"terminal.empty\")}" }
                }
            }
        }
    }
}

/// Fifty cells: the measured width / 50 is one column.
const MEASURE_TEXT: &str = "WWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWWW";

/// Focuses the hidden input once the current render has landed.
const FOCUS_SCRIPT: &str = r#"setTimeout(() => { const el = document.getElementById('INPUT_ID'); if (el) el.focus({ preventScroll: true }); }, 0);"#;

fn focus_input(input_id: &str) {
    dioxus::document::eval(&FOCUS_SCRIPT.replace("INPUT_ID", input_id));
}

fn refresh_tabs(mut tabs: Signal<Vec<TerminalInfo>>, project_dir: Signal<Option<String>>) {
    let Some(dir) = project_dir.peek().clone() else { return };
    let open = terminal::list(Some(&dir));
    if *tabs.peek() != open {
        tabs.set(open);
    }
}

fn close_tab(
    id: &str,
    tabs: Signal<Vec<TerminalInfo>>,
    mut active: Signal<Option<String>>,
    project_dir: Signal<Option<String>>,
) {
    let position = tabs.peek().iter().position(|tab| tab.id == id);
    terminal::close(id);
    refresh_tabs(tabs, project_dir);
    if active.peek().as_deref() == Some(id) {
        let remaining = tabs.peek();
        let next = position
            .and_then(|at| remaining.get(at).or_else(|| remaining.last()))
            .map(|tab| tab.id.clone());
        drop(remaining);
        active.set(next);
    }
}

fn handle_view_event(
    event: ViewEvent,
    active: Signal<Option<String>>,
    screen: Signal<Option<TerminalScreen>>,
    tabs: Signal<Vec<TerminalInfo>>,
    mut size: Signal<(u16, u16)>,
) {
    let id = active.peek().clone();
    let (application_cursor, bracketed_paste, alternate_screen) = screen
        .peek()
        .as_ref()
        .map(|s| (s.application_cursor, s.bracketed_paste, s.alternate_screen))
        .unwrap_or_default();
    match event {
        ViewEvent::Key { key, ctrl, alt, shift } => {
            if let (Some(id), Some(bytes)) = (id, key_bytes(&key, ctrl, alt, shift, application_cursor)) {
                let _ = terminal::write(&id, &bytes);
            }
        }
        ViewEvent::Text { text } => {
            if let Some(id) = id.filter(|_| !text.is_empty()) {
                let _ = terminal::write(&id, text.as_bytes());
            }
        }
        ViewEvent::Paste { text } => {
            if let Some(id) = id.filter(|_| !text.is_empty()) {
                let _ = terminal::write(&id, &paste_bytes(&text, bracketed_paste));
            }
        }
        ViewEvent::Copy { text } => {
            let text = trim_copied(&text);
            if !text.is_empty() {
                spawn(async move {
                    crate::clipboard::copy_text(&text).await;
                });
            }
        }
        ViewEvent::Wheel { lines } => {
            let Some(id) = id else { return };
            if alternate_screen {
                // Full-screen programs scroll themselves: send arrows.
                let key = ["ArrowDown", "ArrowUp"][usize::from(lines > 0)];
                if let Some(bytes) = key_bytes(key, false, false, false, application_cursor) {
                    let presses = lines.abs().min(MAX_WHEEL_ARROWS) as usize;
                    let _ = terminal::write(&id, &bytes.repeat(presses));
                }
            } else {
                terminal::scroll(&id, lines);
            }
        }
        ViewEvent::Resize { cols, rows } => {
            if cols == 0 || rows == 0 || *size.peek() == (cols, rows) {
                return;
            }
            size.set((cols, rows));
            for tab in tabs.peek().iter() {
                let _ = terminal::resize(&tab.id, cols, rows);
            }
        }
    }
}

/// Bytes for a key press, xterm-style; `None` for keys the terminal leaves
/// alone. `application_cursor` (DECCKM) switches arrows / Home / End to `ESC O`.
fn key_bytes(key: &str, ctrl: bool, alt: bool, shift: bool, application_cursor: bool) -> Option<Vec<u8>> {
    let modifier = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let cursor_key = |letter: char| -> Vec<u8> {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{letter}").into_bytes()
        } else if application_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let tilde_key = |code: u8| -> Vec<u8> {
        if modifier > 1 {
            format!("\x1b[{code};{modifier}~").into_bytes()
        } else {
            format!("\x1b[{code}~").into_bytes()
        }
    };
    let named = match key {
        "Enter" => Some(if alt { b"\x1b\r".to_vec() } else { b"\r".to_vec() }),
        "Backspace" => Some(match (ctrl, alt) {
            (true, _) => b"\x08".to_vec(),
            (false, true) => b"\x1b\x7f".to_vec(),
            (false, false) => b"\x7f".to_vec(),
        }),
        "Tab" => Some(if shift { b"\x1b[Z".to_vec() } else { b"\t".to_vec() }),
        "Escape" => Some(b"\x1b".to_vec()),
        "ArrowUp" => Some(cursor_key('A')),
        "ArrowDown" => Some(cursor_key('B')),
        "ArrowRight" => Some(cursor_key('C')),
        "ArrowLeft" => Some(cursor_key('D')),
        "Home" => Some(cursor_key('H')),
        "End" => Some(cursor_key('F')),
        "Insert" => Some(tilde_key(2)),
        "Delete" => Some(tilde_key(3)),
        "PageUp" => Some(tilde_key(5)),
        "PageDown" => Some(tilde_key(6)),
        "F1" => Some(b"\x1bOP".to_vec()),
        "F2" => Some(b"\x1bOQ".to_vec()),
        "F3" => Some(b"\x1bOR".to_vec()),
        "F4" => Some(b"\x1bOS".to_vec()),
        "F5" => Some(tilde_key(15)),
        "F6" => Some(tilde_key(17)),
        "F7" => Some(tilde_key(18)),
        "F8" => Some(tilde_key(19)),
        "F9" => Some(tilde_key(20)),
        "F10" => Some(tilde_key(21)),
        "F11" => Some(tilde_key(23)),
        "F12" => Some(tilde_key(24)),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else { return None };
    // Ctrl+Alt is AltGr on Windows layouts (`@`, `{` ...): the plain char.
    if ctrl && !alt {
        let control = match c.to_ascii_lowercase() {
            letter @ 'a'..='z' => letter as u8 - b'a' + 1,
            '@' | ' ' | '2' => 0,
            '[' | '3' => 0x1b,
            '\\' | '4' => 0x1c,
            ']' | '5' => 0x1d,
            '^' | '6' => 0x1e,
            '_' | '-' | '7' => 0x1f,
            '?' | '8' => 0x7f,
            _ => return None,
        };
        return Some(vec![control]);
    }
    let mut bytes = Vec::with_capacity(5);
    if alt && !ctrl {
        bytes.push(0x1b);
    }
    let mut utf8 = [0u8; 4];
    bytes.extend_from_slice(c.encode_utf8(&mut utf8).as_bytes());
    Some(bytes)
}

/// Pasted text with newlines as Enter, wrapped for bracketed paste.
fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        // A pasted end marker must not end the paste early.
        let inner = normalized.replace("\x1b[201~", "");
        format!("\x1b[200~{inner}\x1b[201~").into_bytes()
    } else {
        normalized.into_bytes()
    }
}

/// A screen selection with the grid's trailing blanks removed per line.
fn trim_copied(text: &str) -> String {
    text.lines().map(str::trim_end).collect::<Vec<_>>().join("\n").trim_end().to_string()
}

/// A screen row as escaped `<span>` markup.
fn row_html(row: &[TerminalSpan]) -> String {
    let mut html = String::new();
    for span in row {
        let (class, style) = span_style(span);
        html.push_str("<span");
        if !class.is_empty() {
            html.push_str(&format!(" class=\"{class}\""));
        }
        if !style.is_empty() {
            html.push_str(&format!(" style=\"{style}\""));
        }
        html.push('>');
        for c in span.text.chars() {
            match c {
                '&' => html.push_str("&amp;"),
                '<' => html.push_str("&lt;"),
                '>' => html.push_str("&gt;"),
                '"' => html.push_str("&quot;"),
                _ => html.push(c),
            }
        }
        html.push_str("</span>");
    }
    html
}

/// CSS class and inline style of a span (colours via the `--term-*` palette).
fn span_style(span: &TerminalSpan) -> (String, String) {
    let mut fg = color_css(span.fg, span.bold);
    let mut bg = color_css(span.bg, false);
    if span.inverse {
        let new_fg = bg.unwrap_or_else(|| "var(--term-bg)".to_string());
        let new_bg = fg.unwrap_or_else(|| "var(--term-fg)".to_string());
        fg = Some(new_fg);
        bg = Some(new_bg);
    }
    let mut style = String::new();
    if let Some(fg) = fg {
        style.push_str(&format!("color:{fg};"));
    }
    if let Some(bg) = bg {
        style.push_str(&format!("background:{bg};"));
    }
    let mut class = String::new();
    for (on, name) in [
        (span.bold, "b"),
        (span.dim, "d"),
        (span.italic, "i"),
        (span.underline, "u"),
        (span.cursor, "term-cursor"),
    ] {
        if on {
            if !class.is_empty() {
                class.push(' ');
            }
            class.push_str(name);
        }
    }
    (class, style)
}

/// `None` for the default colour; bold lifts the 8 base colours to bright.
fn color_css(color: TerminalColor, bold: bool) -> Option<String> {
    match color {
        TerminalColor::Default => None,
        TerminalColor::Indexed(index) if index < 8 && bold => Some(format!("var(--term-{})", index + 8)),
        TerminalColor::Indexed(index) if index < 16 => Some(format!("var(--term-{index})")),
        TerminalColor::Indexed(index) if index < 232 => {
            let cube = index - 16;
            let level = |step: u8| if step == 0 { 0 } else { 55 + step * 40 };
            Some(format!("rgb({},{},{})", level(cube / 36), level((cube / 6) % 6), level(cube % 6)))
        }
        TerminalColor::Indexed(index) => {
            let gray = 8 + (index - 232) * 10;
            Some(format!("rgb({gray},{gray},{gray})"))
        }
        TerminalColor::Rgb(r, g, b) => Some(format!("rgb({r},{g},{b})")),
    }
}

/// Wires the view: keyboard (capture phase, so app shortcuts do not fire
/// while typing in the shell), paste / IME on the hidden textarea, copy,
/// wheel and a ResizeObserver that fits the grid. Strings use single quotes
/// only (the i18n scanner reads double-quoted literals).
const VIEW_SCRIPT: &str = r#"
const mac = navigator.platform.toUpperCase().startsWith('MAC');
const named = new Set(['Enter','Backspace','Tab','Escape','ArrowUp','ArrowDown','ArrowLeft','ArrowRight',
  'Home','End','PageUp','PageDown','Delete','Insert','F1','F2','F3','F4','F5','F6','F7','F8','F9','F10','F11','F12']);
let tries = 0;
const wire = () => {
  const view = document.getElementById('VIEW_ID');
  const input = document.getElementById('INPUT_ID');
  const measure = document.getElementById('MEASURE_ID');
  if (!view || !input || !measure) { if (tries++ < 200) setTimeout(wire, 50); return; }
  const selectionText = () => (window.getSelection() || '').toString();
  view.addEventListener('keydown', (e) => {
    if (e.isComposing || e.keyCode === 229) return;
    const mod = mac ? e.metaKey : e.ctrlKey;
    const key = e.key || '';
    const lower = key.toLowerCase();
    const selected = selectionText();
    if (mod && lower === 'c' && (e.shiftKey || mac || selected)) {
      e.preventDefault(); e.stopPropagation();
      if (selected) { dioxus.send({ t: 'copy', text: selected }); window.getSelection().removeAllRanges(); }
      return;
    }
    if (mod && lower === 'v') {
      if (document.activeElement !== input) input.focus({ preventScroll: true });
      e.stopPropagation();
      return;
    }
    if (e.metaKey) return;
    let take = named.has(key);
    if (!take && [...key].length === 1) {
      take = !(e.ctrlKey && !e.altKey) || /^[a-z@\[\\\]\^_ ?2-8\-]$/i.test(key);
    }
    if (!take) return;
    e.preventDefault(); e.stopPropagation();
    dioxus.send({ t: 'key', key: key, ctrl: e.ctrlKey, alt: e.altKey, shift: e.shiftKey });
  }, true);
  input.addEventListener('paste', (e) => {
    e.preventDefault();
    const text = (e.clipboardData && e.clipboardData.getData('text/plain')) || '';
    if (text) dioxus.send({ t: 'paste', text: text });
  });
  input.addEventListener('compositionend', (e) => {
    if (e.data) dioxus.send({ t: 'text', text: e.data });
    input.value = '';
  });
  input.addEventListener('input', (e) => {
    if (e.isComposing) return;
    if (input.value) { dioxus.send({ t: 'text', text: input.value }); input.value = ''; }
  });
  view.addEventListener('mouseup', () => {
    if (!selectionText()) input.focus({ preventScroll: true });
  });
  let lineHeight = 17;
  let wheelRest = 0;
  view.addEventListener('wheel', (e) => {
    e.preventDefault();
    const lines = e.deltaMode === 1 ? e.deltaY : e.deltaY / lineHeight;
    wheelRest -= lines;
    const whole = Math.trunc(wheelRest);
    if (whole !== 0) { wheelRest -= whole; dioxus.send({ t: 'wheel', lines: whole }); }
  }, { passive: false });
  let last = '';
  const fit = () => {
    const cell = measure.getBoundingClientRect();
    if (!cell.width || !cell.height) return;
    lineHeight = cell.height;
    const style = getComputedStyle(view);
    const padX = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
    const padY = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
    const cols = Math.max(1, Math.floor((view.clientWidth - padX) / (cell.width / 50)));
    const rows = Math.max(1, Math.floor((view.clientHeight - padY) / cell.height));
    const next = cols + 'x' + rows;
    if (next !== last && view.clientWidth > 0 && view.clientHeight > 0) {
      last = next;
      dioxus.send({ t: 'resize', cols: cols, rows: rows });
    }
  };
  new ResizeObserver(fit).observe(view);
  if (document.fonts && document.fonts.ready) document.fonts.ready.then(fit);
  fit();
};
wire();
"#;

/// Terminal styles; the 16 ANSI colours follow the app's light / dark
/// scheme through `light-dark()`.
const TERMINAL_CSS: &str = r#"
.terminal-panel {
  --term-fg: var(--text);
  --term-bg: var(--bg-panel);
  --term-cursor: var(--text);
  --term-0: light-dark(#2a2a2e, #3a3a40);
  --term-1: light-dark(#b8433b, #e0756c);
  --term-2: light-dark(#2f7d4f, #8cc49e);
  --term-3: light-dark(#8a6a12, #d9b46a);
  --term-4: light-dark(#2f62c0, #6fa0e8);
  --term-5: light-dark(#8a4bb0, #c49ae0);
  --term-6: light-dark(#1f7f86, #6fc3c0);
  --term-7: light-dark(#7a7a80, #c8c8ce);
  --term-8: light-dark(#66666c, #6a6a72);
  --term-9: light-dark(#d0554c, #f0928a);
  --term-10: light-dark(#3a9a62, #a5dbb5);
  --term-11: light-dark(#a38317, #ead08a);
  --term-12: light-dark(#3f78e0, #94bcf5);
  --term-13: light-dark(#a25fcc, #d8b5f0);
  --term-14: light-dark(#23969e, #8fdcd8);
  --term-15: light-dark(#3a3a40, #f2f2f5);
  display: flex; flex-direction: column; height: 100%; min-height: 0; min-width: 0;
  background: var(--term-bg); color: var(--term-fg);
}
.term-tabs {
  display: flex; align-items: center; gap: 2px; flex: none;
  padding: 4px 6px; border-bottom: 1px solid var(--border);
  overflow-x: auto; scrollbar-width: none;
}
.term-tab {
  display: flex; align-items: center; gap: 4px; flex: none; max-width: 160px;
  height: 24px; padding: 0 4px 0 8px; border-radius: 6px;
  font-size: 12px; color: var(--text-3); cursor: pointer; user-select: none;
}
.term-tab:hover { background: var(--hover); color: var(--text-2); }
.term-tab.active { background: var(--active); color: var(--text); }
.term-tab.exited .term-tab-title { opacity: 0.6; }
.term-tab-title { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.term-tab-close, .term-new {
  display: inline-flex; align-items: center; justify-content: center; flex: none;
  border: 0; background: transparent; color: var(--text-3); cursor: pointer; padding: 0; border-radius: 4px;
}
.term-tab-close { width: 16px; height: 16px; opacity: 0; }
.term-tab:hover .term-tab-close, .term-tab.active .term-tab-close { opacity: 1; }
.term-tab-close:hover, .term-new:hover { background: var(--hover); color: var(--text); }
.term-tab-close svg { width: 11px; height: 11px; }
.term-new { width: 24px; height: 24px; }
.term-new svg { width: 14px; height: 14px; }
.term-view {
  position: relative; flex: 1; min-height: 0; overflow: hidden; outline: none;
  padding: 6px 8px; cursor: text;
  font-family: var(--font-mono); font-size: calc(12.5px * var(--font-scale, 1)); line-height: 1.35;
  font-variant-ligatures: none; font-feature-settings: 'calt' 0, 'liga' 0;
}
.term-rows { user-select: text; }
.term-row { white-space: pre; overflow: hidden; height: 1.35em; }
.term-row span.b { font-weight: 600; }
.term-row span.d { opacity: 0.6; }
.term-row span.i { font-style: italic; }
.term-row span.u { text-decoration: underline; }
.term-cursor { box-shadow: inset 0 0 0 1px var(--term-cursor); }
.term-view:focus-within .term-cursor { background: var(--term-cursor); color: var(--term-bg); box-shadow: none; }
.term-input {
  position: absolute; left: 0; bottom: 0; width: 1px; height: 1px; opacity: 0;
  padding: 0; border: 0; resize: none; overflow: hidden; pointer-events: none;
}
.term-measure { position: absolute; visibility: hidden; white-space: pre; top: 0; left: 0; }
.term-note { color: var(--text-3); font-family: var(--font-ui); font-size: 12.5px; padding: 6px 0; }
.term-exited { border-top: 1px solid var(--border); margin-top: 6px; }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_render_as_escaped_spans_with_their_style() {
        let span = TerminalSpan { text: "<a & b>".into(), inverse: true, ..Default::default() };
        let html = row_html(&[span]);
        assert!(html.contains("&lt;a &amp; b&gt;"), "{html}");
        assert!(html.contains("background:"), "{html}");
    }

    #[test]
    fn keys_translate_to_xterm_bytes() {
        assert_eq!(key_bytes("a", false, false, false, false).unwrap(), b"a");
        assert_eq!(key_bytes("ş", false, false, false, false).unwrap(), "ş".as_bytes());
        assert_eq!(key_bytes("Enter", false, false, false, false).unwrap(), b"\r");
        assert_eq!(key_bytes("Backspace", false, false, false, false).unwrap(), b"\x7f");
        assert_eq!(key_bytes("c", true, false, false, false).unwrap(), b"\x03");
        assert_eq!(key_bytes("L", true, false, true, false).unwrap(), b"\x0c");
        assert_eq!(key_bytes("[", true, false, false, false).unwrap(), b"\x1b");
        assert_eq!(key_bytes("b", false, true, false, false).unwrap(), b"\x1bb");
        // AltGr (Ctrl+Alt) types the character itself.
        assert_eq!(key_bytes("@", true, true, false, false).unwrap(), b"@");
        assert_eq!(key_bytes("ArrowUp", false, false, false, false).unwrap(), b"\x1b[A");
        assert_eq!(key_bytes("ArrowUp", false, false, false, true).unwrap(), b"\x1bOA");
        assert_eq!(key_bytes("ArrowLeft", true, false, false, false).unwrap(), b"\x1b[1;5D");
        assert_eq!(key_bytes("Delete", false, false, false, false).unwrap(), b"\x1b[3~");
        assert_eq!(key_bytes("Tab", false, false, true, false).unwrap(), b"\x1b[Z");
        assert!(key_bytes("Shift", false, false, true, false).is_none());
        assert!(key_bytes("=", true, false, false, false).is_none());
    }

    #[test]
    fn pastes_use_carriage_returns_and_brackets() {
        assert_eq!(paste_bytes("a\r\nb\nc", false), b"a\rb\rc");
        assert_eq!(paste_bytes("x\x1b[201~y", true), b"\x1b[200~xy\x1b[201~");
    }

    #[test]
    fn copies_drop_trailing_blanks() {
        assert_eq!(trim_copied("ls   \nfoo  bar   \n   \n"), "ls\nfoo  bar");
    }

    #[test]
    fn colors_map_to_palette_and_rgb() {
        assert_eq!(color_css(TerminalColor::Default, false), None);
        assert_eq!(color_css(TerminalColor::Indexed(1), false).unwrap(), "var(--term-1)");
        assert_eq!(color_css(TerminalColor::Indexed(1), true).unwrap(), "var(--term-9)");
        assert_eq!(color_css(TerminalColor::Indexed(196), false).unwrap(), "rgb(255,0,0)");
        assert_eq!(color_css(TerminalColor::Indexed(232), false).unwrap(), "rgb(8,8,8)");
        assert_eq!(color_css(TerminalColor::Rgb(1, 2, 3), false).unwrap(), "rgb(1,2,3)");
        let inverse = TerminalSpan { text: "x".into(), inverse: true, cursor: true, ..Default::default() };
        let (class, style) = span_style(&inverse);
        assert_eq!(class, "term-cursor");
        assert_eq!(style, "color:var(--term-bg);background:var(--term-fg);");
    }

    #[test]
    fn view_events_parse() {
        let key: ViewEvent = serde_json::from_str(r#"{"t":"key","key":"a","ctrl":false,"alt":false,"shift":false}"#).unwrap();
        assert_eq!(key, ViewEvent::Key { key: "a".into(), ctrl: false, alt: false, shift: false });
        let resize: ViewEvent = serde_json::from_str(r#"{"t":"resize","cols":90,"rows":30}"#).unwrap();
        assert_eq!(resize, ViewEvent::Resize { cols: 90, rows: 30 });
    }
}
