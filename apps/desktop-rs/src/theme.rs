//! Theming: one neutral, layered design-token set (dark + light) with a single
//! restrained indigo accent, plus per-platform fonts.
//!
//! * Windows — Segoe UI Variable / Cascadia Code
//! * macOS — SF Pro / SF Mono (native traffic lights, so no window controls)
//! * everything else — system UI fonts
//!
//! `build_stylesheet_for` emits a `:root` block of CSS custom properties for
//! the chosen mode and appends the static [`BASE_CSS`], which only reads
//! tokens through `var(--…)`. Components never hard-code colours.

use crate::prefs::{Appearance, CodeFont, ThemeChoice, UiFont};

/// The CSS class applied to `.app-shell` for dark/light scoping.
pub fn theme_class(dark_mode: bool) -> &'static str {
    if dark_mode {
        "theme-dark"
    } else {
        "theme-light"
    }
}

/// Builds the default-font stylesheet for the detected OS (tests only; the
/// app uses [`build_stylesheet_appearance`]).
#[cfg(test)]
pub fn build_stylesheet(dark_mode: bool) -> String {
    build_stylesheet_for(std::env::consts::OS, dark_mode)
}

/// Dark palette: near-black layered surfaces, hairline borders, indigo accent.
const DARK_TOKENS: &str = "
  --bg-app: #0e0e10;
  --bg-chat: #0e0e10;
  --bg-sidebar: #111113;
  --bg-panel: #0e0e10;
  --bg-elevated: #18181b;
  --bg-bubble: #242427;
  --bg-popover: #1d1d21;
  --bg-input: rgba(255, 255, 255, 0.04);
  --hover: rgba(255, 255, 255, 0.055);
  --active: rgba(255, 255, 255, 0.07);
  --border: rgba(255, 255, 255, 0.06);
  --border-strong: rgba(255, 255, 255, 0.13);
  --text: #ececef;
  --text-2: #a4a4ad;
  --text-3: #6f6f79;
  --accent: #6d78e8;
  --accent-hover: #7b86ef;
  --accent-fg: #ffffff;
  --accent-soft: rgba(109, 120, 232, 0.16);
  --accent-ring: rgba(125, 136, 240, 0.55);
  --ok: #62a884;
  --warn: #c7a35e;
  --err: #d2766f;
  --diff-add-bg: rgba(98, 168, 132, 0.14);
  --diff-add-fg: #8fcfae;
  --diff-del-bg: rgba(210, 118, 111, 0.13);
  --diff-del-fg: #e49e98;
  --shadow-card: 0 1px 2px rgba(0, 0, 0, 0.4);
  --shadow-pop: 0 8px 28px rgba(0, 0, 0, 0.5), 0 0 0 1px rgba(255, 255, 255, 0.06);
  --win-close: #c42b1c;
  --code-bg: #141416;
  --code-border: rgba(255, 255, 255, 0.08);
  --chip-bg: rgba(255, 255, 255, 0.06);
  --rule: rgba(255, 255, 255, 0.08);
  --rule-strong: rgba(255, 255, 255, 0.14);
  --syn-keyword: #e0a07a;
  --syn-function: #7aa8e0;
  --syn-type: #6fbfb3;
  --syn-string: #8fb58a;
  --syn-number: #b59ae0;
  --syn-comment: #6f6f79;
";

/// Light palette: airy paper surfaces with the same structure.
const LIGHT_TOKENS: &str = "
  --bg-app: #fafafa;
  --bg-chat: #ffffff;
  --bg-sidebar: #f3f3f5;
  --bg-panel: #f7f7f8;
  --bg-elevated: #ffffff;
  --bg-bubble: #ececef;
  --bg-popover: #ffffff;
  --bg-input: rgba(0, 0, 0, 0.035);
  --hover: rgba(0, 0, 0, 0.045);
  --active: rgba(0, 0, 0, 0.075);
  --border: rgba(0, 0, 0, 0.08);
  --border-strong: rgba(0, 0, 0, 0.15);
  --text: #18181b;
  --text-2: #52525b;
  --text-3: #8b8b95;
  --accent: #4f5bd5;
  --accent-hover: #4350c4;
  --accent-fg: #ffffff;
  --accent-soft: rgba(79, 91, 213, 0.11);
  --accent-ring: rgba(79, 91, 213, 0.5);
  --ok: #2f7d5b;
  --warn: #9a6a12;
  --err: #c0392b;
  --diff-add-bg: rgba(47, 125, 91, 0.12);
  --diff-add-fg: #1f6244;
  --diff-del-bg: rgba(192, 57, 43, 0.1);
  --diff-del-fg: #a3302a;
  --shadow-card: 0 1px 2px rgba(0, 0, 0, 0.06);
  --shadow-pop: 0 8px 28px rgba(0, 0, 0, 0.14), 0 0 0 1px rgba(0, 0, 0, 0.06);
  --win-close: #c42b1c;
  --code-bg: #f6f6f8;
  --code-border: rgba(0, 0, 0, 0.09);
  --chip-bg: rgba(0, 0, 0, 0.055);
  --rule: rgba(0, 0, 0, 0.09);
  --rule-strong: rgba(0, 0, 0, 0.16);
  --syn-keyword: #b4541a;
  --syn-function: #2a63b0;
  --syn-type: #1f7a70;
  --syn-string: #3f7a3a;
  --syn-number: #7b4fb3;
  --syn-comment: #8b8b95;
";

/// Builds the stylesheet for an explicit platform (`"windows"`, `"macos"`,
/// anything else = Linux/other) and a fixed dark/light mode with the default
/// fonts. Split out so every platform's output is unit-testable.
#[cfg(test)]
pub fn build_stylesheet_for(platform: &str, dark_mode: bool) -> String {
    let appearance = Appearance {
        theme: if dark_mode { ThemeChoice::Dark } else { ThemeChoice::Light },
        ..Appearance::default()
    };
    build_stylesheet_with(platform, &appearance)
}

/// Builds the stylesheet for the detected OS and the user's appearance.
pub fn build_stylesheet_appearance(appearance: &Appearance) -> String {
    build_stylesheet_with(std::env::consts::OS, appearance)
}

/// The full stylesheet: a `:root` block of tokens + font variables, then the
/// static [`BASE_CSS`] with its pixel font sizes routed through `--font-scale`.
/// `ThemeChoice::System` emits the dark tokens plus a
/// `prefers-color-scheme: light` override, so the webview follows the OS
/// without any script.
pub fn build_stylesheet_with(platform: &str, appearance: &Appearance) -> String {
    let platform_ui = match platform {
        "macos" => "-apple-system, BlinkMacSystemFont, \"SF Pro Text\", \"SF Pro Display\", system-ui, sans-serif",
        "windows" => "\"Segoe UI Variable Text\", \"Segoe UI\", system-ui, sans-serif",
        _ => "system-ui, \"Ubuntu\", \"Cantarell\", sans-serif",
    };
    let platform_mono = match platform {
        "macos" => "\"SF Mono\", \"Menlo\", ",
        "windows" => "",
        _ => "\"JetBrains Mono\", \"DejaVu Sans Mono\", ",
    };
    let font_ui = ui_font_stack(appearance, platform_ui);
    let font_code = code_font_stack(appearance, platform_mono);
    let mono_ui = appearance.ui_font == UiFont::CascadiaCode;
    // A monospace UI font reads larger than a proportional one: shrink it.
    let scale = format_scale(appearance.font_size.factor() * if mono_ui { 0.94 } else { 1.0 });
    let ui_ligatures = if mono_ui { "none" } else { "normal" };
    let (code_ligatures, code_features) = if appearance.code_ligatures {
        ("normal", "\"calt\" 1, \"liga\" 1")
    } else {
        ("none", "\"calt\" 0, \"liga\" 0")
    };
    // macOS traffic lights sit at x: 14..72 inside the full-size content view,
    // so the brand strip must clear them; other platforms draw their own
    // window controls (3 x 46px) at the top right.
    let (brand_pad, controls_w) = if platform == "macos" {
        ("0 10px 0 80px", "0px")
    } else {
        ("0 12px 0 14px", "144px")
    };
    let vars = format!(
        "  --font-ui: {font_ui};\n  --font-code: {font_code};\n  --font-mono: var(--font-code);\n  --font-scale: {scale};\n  --ui-ligatures: {ui_ligatures};\n  --code-ligatures: {code_ligatures};\n  --code-features: {code_features};\n  --brand-pad: {brand_pad};\n  --controls-w: {controls_w};"
    );
    let (scheme, tokens) = match appearance.theme {
        ThemeChoice::Light => ("light", LIGHT_TOKENS),
        ThemeChoice::Dark | ThemeChoice::System => ("dark", DARK_TOKENS),
    };
    let system_override = if appearance.theme == ThemeChoice::System {
        format!("@media (prefers-color-scheme: light) {{\n:root {{\n  color-scheme: light;{LIGHT_TOKENS}}}\n}}\n")
    } else {
        String::new()
    };
    format!(
        ":root {{\n  color-scheme: {scheme};\n{vars}{tokens}}}\n{system_override}{}\n{}",
        scale_font_sizes(BASE_CSS),
        scale_font_sizes(crate::components::review::REVIEW_CSS)
    )
}

/// `0.94`, `1`, `1.1` — no trailing zeros.
fn format_scale(scale: f32) -> String {
    let text = format!("{scale:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Keeps only characters that are safe inside a quoted CSS font-family name.
fn sanitize_family(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
        .collect::<String>()
        .trim()
        .to_string()
}

fn ui_font_stack(appearance: &Appearance, platform_ui: &str) -> String {
    match appearance.ui_font {
        UiFont::System => platform_ui.to_string(),
        UiFont::Inter => format!("\"Inter\", \"Inter Variable\", {platform_ui}"),
        UiFont::Geist => format!("\"Geist\", \"Geist Variable\", {platform_ui}"),
        UiFont::CascadiaCode => "\"Cascadia Code\", \"Cascadia Mono\", Consolas, monospace".to_string(),
        UiFont::Custom => match sanitize_family(&appearance.ui_font_custom) {
            name if name.is_empty() => platform_ui.to_string(),
            name => format!("\"{name}\", {platform_ui}"),
        },
    }
}

fn code_font_stack(appearance: &Appearance, platform_mono: &str) -> String {
    let tail = format!("\"Cascadia Mono\", Consolas, {platform_mono}monospace");
    match appearance.code_font {
        CodeFont::CascadiaCode => format!("\"Cascadia Code\", {tail}"),
        CodeFont::JetBrainsMono => format!("\"JetBrains Mono\", {tail}"),
        CodeFont::Consolas => format!("Consolas, \"Cascadia Mono\", {platform_mono}monospace"),
        CodeFont::Custom => match sanitize_family(&appearance.code_font_custom) {
            name if name.is_empty() => format!("\"Cascadia Code\", {tail}"),
            name => format!("\"{name}\", {tail}"),
        },
    }
}

/// Rewrites every `font-size: <n>px` into `calc(<n>px * var(--font-scale))`
/// so the whole UI follows the font-size setting.
fn scale_font_sizes(css: &str) -> String {
    const KEY: &str = "font-size:";
    let mut out = String::with_capacity(css.len() + 4096);
    let mut rest = css;
    while let Some(pos) = rest.find(KEY) {
        let (before, after) = rest.split_at(pos + KEY.len());
        out.push_str(before);
        let trimmed = after.trim_start();
        let skipped = after.len() - trimmed.len();
        let number_len = trimmed
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(trimmed.len());
        if number_len > 0 && trimmed[number_len..].starts_with("px") {
            out.push_str(&after[..skipped]);
            out.push_str(&format!("calc({}px * var(--font-scale))", &trimmed[..number_len]));
            rest = &trimmed[number_len + 2..];
        } else {
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Static rules; every colour comes from the `:root` tokens above.
const BASE_CSS: &str = r#"
* { margin: 0; padding: 0; box-sizing: border-box; user-select: none; -webkit-user-select: none; }
html, body { height: 100%; overflow: hidden; }
body {
  font-family: var(--font-ui);
  font-size: 13px;
  font-variant-ligatures: var(--ui-ligatures);
  line-height: 1.4;
  color: var(--text);
  background: var(--bg-app);
  -webkit-font-smoothing: antialiased;
  cursor: default;
}
#main { height: 100vh; width: 100vw; display: flex; overflow: hidden; }
code, pre, .mono {
  font-family: var(--font-code);
  font-variant-ligatures: var(--code-ligatures);
  font-feature-settings: var(--code-features);
}
.font-preview { padding: 10px 12px; border: 1px solid var(--border); border-radius: 8px; background: var(--bg-input); color: var(--text); }
.seg { display: inline-flex; border: 1px solid var(--border-strong); border-radius: 8px; overflow: hidden; flex: none; }
.seg button { border: 0; border-radius: 0; background: transparent; padding: 5px 12px; color: var(--text-2); }
.seg button + button { border-left: 1px solid var(--border); }
.seg button.active { background: var(--accent-soft); color: var(--text); font-weight: 500; }
.appearance-row { display: flex; align-items: center; justify-content: space-between; gap: 24px; padding: 14px 0; border-bottom: 1px solid var(--border); }
.appearance-row .ar-label { font-weight: 500; color: var(--text); }
.appearance-row .ar-desc { font-size: 12px; color: var(--text-3); margin-top: 2px; }
.appearance-row .ar-controls { display: flex; flex-direction: column; align-items: flex-end; gap: 8px; }
.appearance-row .ar-controls input { width: 220px; }
.appearance-preview-row { padding: 6px 0 14px; }

/* Selectable text: chat content, alerts and inputs only. */
.msg-user .bubble, .msg-user .bubble *,
.msg-assistant .bubble, .msg-assistant .bubble *,
.reasoning-panel .collapsible-body,
.tool-output, .diff-view, .diff-line,
.toast,
.permission-card .pc-detail, .permission-card .pc-pathbox,
.question-card .qc-question,
.badge.err, .badge.warn {
  user-select: text;
  -webkit-user-select: text;
}
input, textarea { user-select: text; -webkit-user-select: text; cursor: text; }

/* ---------------- icons ---------------- */
.icon { width: 16px; height: 16px; flex: none; display: block; }
.ccm-item .icon, .tool-head .icon, .collapsible-header .icon, .composer-select-btn .icon,
.project-menu-btn .icon, .project-new-btn .icon, .run-action .icon,
.file-kind .icon, .badge .icon { width: 14px; height: 14px; }

/* ---------------- shell layout ---------------- */
.app-shell {
  display: flex; height: 100vh; width: 100vw;
  background: var(--bg-app); color: var(--text);
  overflow: hidden;
}
.sidebar {
  width: 264px; min-width: 264px; height: 100%;
  background: var(--bg-sidebar);
  border-right: 1px solid var(--border);
  display: flex; flex-direction: column;
}
.main-column {
  flex: 1 1 auto; min-width: 0; height: 100%;
  display: flex; flex-direction: column;
  background: var(--bg-chat);
}
.right-panel {
  width: 380px; min-width: 380px; height: 100%;
  background: var(--bg-panel);
  border-left: 1px solid var(--border);
  display: flex; flex-direction: column;
  overflow: hidden;
}

/* Header strips share one 44px height and a hairline underneath. When no
   inspector is open the main column is the rightmost one and clears the
   window controls. */
.chat-header, .page-header, .right-panel-head, .sidebar-brand {
  height: 44px; min-height: 44px; max-height: 44px;
  display: flex; align-items: center; gap: 12px;
  border-bottom: 1px solid var(--border);
  flex: none;
}
.app-shell.no-inspector .chat-header,
.app-shell.no-inspector .page-header { padding-right: var(--controls-w); }

/* ---------------- window controls ---------------- */
.window-controls {
  position: fixed; top: 0; right: 0; z-index: 200;
  display: flex; height: 32px;
}
.win-btn {
  width: 46px; height: 32px; padding: 0; border-radius: 0;
  display: inline-flex; align-items: center; justify-content: center;
  color: var(--text-2); background: transparent;
}
.win-btn:hover { background: var(--hover); color: var(--text); }
.win-btn:active { background: var(--active); opacity: 1; }
.win-btn.win-close:hover, .win-btn.win-close:active { background: var(--win-close); color: #ffffff; }
.win-glyph { width: 10px; height: 10px; display: block; }

/* ---------------- generic controls ---------------- */
button, .btn {
  font: inherit; color: var(--text-2); background: transparent;
  border: none; border-radius: 6px;
  -webkit-appearance: none; appearance: none;
  padding: 5px 10px;
  cursor: pointer;
  transition: background 0.12s ease, color 0.12s ease, opacity 0.12s ease, border-color 0.12s ease;
}
button:hover { background: var(--hover); color: var(--text); }
button:active { background: var(--active); }
button:focus-visible, .row-focusable:focus-visible, [role="button"]:focus-visible, [tabindex]:focus-visible {
  outline: 2px solid var(--accent-ring); outline-offset: 1px;
}
input:focus-visible, textarea:focus-visible, select:focus-visible { outline: none; }
button.primary {
  background: var(--accent); color: var(--accent-fg);
  padding: 5px 12px; font-weight: 500;
}
button.primary:hover { background: var(--accent-hover); color: var(--accent-fg); }
button.subtle { color: var(--text-2); }
button.danger { color: var(--err); }
button.danger:hover { background: color-mix(in srgb, var(--err) 14%, transparent); color: var(--err); }
button.icon-btn {
  min-width: 28px; height: 28px; padding: 0 6px; line-height: 1;
  display: inline-flex; align-items: center; justify-content: center; gap: 6px;
  flex: none;
}
button.small { font-size: 11px; padding: 3px 8px; height: 24px; }
button:disabled { opacity: 0.4; cursor: default; background: transparent; color: var(--text-2); }
button.primary:disabled { background: var(--accent); color: var(--accent-fg); }

input, textarea, select {
  font: inherit; color: var(--text);
  background: var(--bg-input); border: 1px solid var(--border); border-radius: 6px;
  padding: 5px 8px;
  outline: none;
  -webkit-appearance: none; appearance: none;
  transition: border-color 0.12s ease, box-shadow 0.12s ease;
}
input:hover, select:hover { border-color: var(--border-strong); }
input:focus, textarea:focus, select:focus {
  border-color: var(--accent-ring);
  box-shadow: 0 0 0 3px var(--accent-soft);
}
input::placeholder, textarea::placeholder { color: var(--text-3); }
select { color: var(--text-2); font-size: 12px; padding: 4px 8px; cursor: pointer; }
label { color: var(--text-2); font-size: 12px; }

/* ---------------- badges & pills ---------------- */
.badge, .pill {
  display: inline-flex; align-items: center; gap: 4px;
  background: var(--hover); border: none; border-radius: 4px;
  padding: 1px 6px;
  font-size: 11px; font-weight: 500; line-height: 16px;
  color: var(--text-3);
  white-space: nowrap;
}
.badge.warn { color: var(--warn); }
.badge.err { color: var(--err); }
.badge.ok { color: var(--ok); }
.pill { max-width: 100%; overflow: hidden; text-overflow: ellipsis; }
.status-dot {
  display: inline-block; width: 6px; height: 6px; border-radius: 50%;
  background: var(--text-3); flex: none;
}
.status-dot.generating, .status-dot.created { background: var(--accent); animation: pulse 1.2s ease-in-out infinite; }
.status-dot.awaiting_permission, .status-dot.awaiting_input { background: var(--warn); }
.status-dot.done { background: var(--ok); }
.status-dot.failed { background: var(--err); }
.status-dot.cancelled { background: var(--text-3); }
@keyframes pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.35; } }

/* ---------------- sidebar ---------------- */
.sidebar-brand { padding: var(--brand-pad); justify-content: space-between; border-bottom-color: transparent; }
.brand { display: flex; align-items: center; gap: 8px; min-width: 0; }
.logo-mark { display: block; flex: none; }
.logo-mark .lm-bg { fill: #131317; stroke: rgba(255, 255, 255, 0.14); stroke-width: 3; }
.logo-mark .lm-fg { fill: #e6e6ea; }
.logo-mark .lm-node { fill: #7f89c8; }
.brand-title { font-size: 14px; font-weight: 600; letter-spacing: -0.01em; color: var(--text); }
.sidebar-toolbar { display: flex; align-items: center; gap: 2px; margin-left: auto; flex: none; }
.sidebar-toolbar button {
  width: 28px; height: 28px; padding: 0;
  display: inline-flex; align-items: center; justify-content: center;
  color: var(--text-2); flex: none;
}
.sidebar-toolbar button:hover { color: var(--text); }
.sidebar-toolbar button:disabled { opacity: 0.3; }
.sidebar-top-controls {
  padding: 4px 10px 8px;
  display: flex; flex-direction: column; gap: 2px;
  flex: none;
}
.sidebar .new-conv-btn {
  width: 100%; height: 32px; padding: 0 10px; margin-bottom: 6px;
  display: flex; align-items: center; justify-content: flex-start; gap: 8px;
  border-radius: 8px; font-size: 13px; font-weight: 500; color: var(--text);
  background: var(--bg-elevated); border: 1px solid var(--border);
}
.sidebar .new-conv-btn:hover { background: var(--bg-popover); border-color: var(--border-strong); }
.sidebar .sidebar-nav-item, .sidebar-footer button {
  width: 100%; height: 30px; padding: 0 10px;
  display: flex; align-items: center; justify-content: flex-start; gap: 10px;
  font-size: 13px; font-weight: 400; color: var(--text-2);
  border-radius: 8px; text-align: left; white-space: nowrap;
}
.sidebar .sidebar-nav-item:hover, .sidebar-footer button:hover { background: var(--hover); color: var(--text); }
.sidebar .sidebar-nav-item.active, .sidebar-footer button.active {
  background: var(--active); color: var(--text); font-weight: 500;
}
.sidebar-nav-icon { width: 16px; height: 16px; display: inline-flex; align-items: center; justify-content: center; color: inherit; flex: none; }
.sidebar-search { flex: none; padding: 0 2px 6px; }
.sidebar-search input { width: 100%; height: 28px; font-size: 12.5px; padding: 0 8px; }
.sidebar-tree { flex: 1; overflow-y: auto; padding: 0 10px 12px; scrollbar-width: thin; }
.sidebar-section-header {
  height: 28px; padding: 0 4px 0 10px; margin-top: 8px;
  display: flex; align-items: center; justify-content: space-between;
  font-size: 11px; font-weight: 500; letter-spacing: 0.04em; text-transform: uppercase; color: var(--text-3);
  flex: none;
}
.sidebar-section-actions { display: flex; align-items: center; gap: 2px; }
.sidebar-section-header .icon-btn { width: 24px; height: 24px; min-width: 24px; padding: 0; color: var(--text-3); }
.sidebar-section-header .icon-btn:hover { color: var(--text); }
.project-group { margin-top: 0; }
.project-row {
  position: relative; height: 30px; min-height: 30px;
  padding: 0 6px 0 10px; border-radius: 8px;
  display: flex; align-items: center; gap: 8px;
  color: var(--text-2); font-size: 13px; font-weight: 500;
  cursor: pointer;
}
.project-row:hover { background: var(--hover); color: var(--text); }
.project-row.selected { color: var(--text); }
.project-folder-icon { width: 16px; height: 16px; display: inline-flex; align-items: center; justify-content: center; color: var(--text-3); flex: none; }
.project-row:hover .project-folder-icon, .project-row.selected .project-folder-icon { color: var(--text-2); }
.project-row .project-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.project-menu-btn, button.project-menu-btn, .project-new-btn, button.project-new-btn {
  width: 22px; height: 22px; padding: 0;
  display: inline-flex; align-items: center; justify-content: center;
  color: var(--text-3); opacity: 0; flex: none;
}
.project-row:hover .project-menu-btn, .project-row:hover .project-new-btn,
.project-row.menu-open .project-menu-btn, .project-row.menu-open .project-new-btn { opacity: 1; }
.project-menu-btn:hover, .project-new-btn:hover { background: var(--active); color: var(--text); }
.project-runs { display: flex; flex-direction: column; gap: 1px; }
.run-item {
  position: relative; height: 30px; min-height: 30px;
  padding: 0 6px 0 30px; border-radius: 8px;
  display: flex; align-items: center; gap: 8px;
  cursor: pointer; font-size: 13px; color: var(--text-2);
}
.run-item:hover { background: var(--hover); color: var(--text); }
.run-item.active { background: var(--active); color: var(--text); }
.run-item .run-title { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.run-item .run-age { font-size: 11px; color: var(--text-3); flex: none; padding-right: 4px; font-variant-numeric: tabular-nums; }
.run-item .run-action, button.run-action {
  display: none; width: 22px; height: 22px; padding: 0;
  align-items: center; justify-content: center;
  color: var(--text-3); flex: none;
}
.run-item:hover .run-age, .run-item.active .run-age, .run-item.menu-open .run-age { display: none; }
.run-item:hover .run-action, .run-item.active .run-action, .run-item.menu-open .run-action { display: inline-flex; }
.run-item .run-action:hover { background: var(--active); color: var(--text); }
.see-all-btn {
  height: 26px; padding: 0 8px 0 30px;
  display: flex; align-items: center; justify-content: flex-start;
  font-size: 12px; color: var(--text-3); text-align: left;
}
.see-all-btn:hover { color: var(--text-2); }
.row-context-menu {
  position: absolute; right: 6px; top: 26px; z-index: 50;
  min-width: 140px; padding: 4px;
  background: var(--bg-popover); border-radius: 8px; box-shadow: var(--shadow-pop);
  display: flex; flex-direction: column; gap: 1px;
}
.sidebar .row-context-menu button {
  width: 100%; height: 28px; padding: 0 10px;
  display: flex; align-items: center; justify-content: flex-start;
  font-size: 12px; font-weight: 400; text-align: left; border-radius: 5px; color: var(--text);
}
.sidebar .row-context-menu button.danger { color: var(--err); }
.sidebar-footer { border-top: 1px solid var(--border); padding: 8px 10px 10px; display: flex; flex-direction: column; flex: none; }

/* ---------------- chat header ---------------- */
.chat-header { padding: 0 12px 0 20px; justify-content: space-between; }
.chat-header-meta { display: flex; align-items: center; gap: 10px; flex: 1; min-width: 0; overflow: hidden; }
.chat-title { font-size: 13.5px; font-weight: 500; flex: 0 1 auto; min-width: 60px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.crumb {
  display: inline-flex; align-items: center; gap: 6px; min-width: 0; max-width: 220px;
  font-size: 12px; color: var(--text-3);
}
.crumb span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.crumb .icon { width: 14px; height: 14px; }
.crumb .crumb-sep { color: var(--text-3); opacity: 0.6; margin-left: 4px; }
.usage-chip {
  font-family: var(--font-mono); font-size: 11px; color: var(--text-3);
  background: var(--hover); border-radius: 4px; padding: 2px 7px; white-space: nowrap; flex: none;
}
.chat-header-actions { display: flex; align-items: center; gap: 2px; flex: none; }
.chat-header-actions button { flex: none; white-space: nowrap; }
button.icon-btn.on { color: var(--text); background: var(--active); }
.count-chip {
  display: inline-flex; align-items: center; justify-content: center;
  min-width: 18px; height: 16px; padding: 0 5px; border-radius: 8px;
  background: var(--active); color: var(--text-2);
  font-size: 10.5px; font-weight: 500; font-variant-numeric: tabular-nums; flex: none;
}

/* ---------------- chat thread ---------------- */
.chat-area { flex: 1; display: flex; flex-direction: column; min-height: 0; background: var(--bg-chat); }
#chat-scroll {
  flex: 1; overflow-y: auto; padding: 24px 28px 16px;
  scrollbar-width: thin;
  display: flex; flex-direction: column; gap: 20px;
}
#chat-scroll > * { flex-shrink: 0; max-width: 800px; width: 100%; margin-left: auto; margin-right: auto; }
/* Grows/shrinks from JS (window.lcgnChat) to let a sent message sit at the top. */
#chat-scroll > .chat-tail-spacer { height: 0; margin-top: -20px; pointer-events: none; }
.msg-user { display: flex; justify-content: flex-end; }
.msg-user .bubble {
  max-width: 78%;
  background: var(--bg-elevated);
  border-radius: 12px; padding: 8px 14px;
  font-size: 13.5px; line-height: 1.55;
  white-space: pre-wrap; word-break: break-word;
}
.msg-assistant { display: flex; flex-direction: column; gap: 8px; }
.msg-assistant .bubble { padding: 2px 0; font-size: 14px; line-height: 1.65; word-break: break-word; }
.msg-meta { display: flex; align-items: center; gap: 8px; font-size: 11px; font-weight: 500; color: var(--text-3); padding: 0 2px; }
.msg-meta .model-label { font-family: var(--font-mono); font-weight: 400; }
.msg-empty {
  margin: auto; text-align: center; color: var(--text-3);
  display: flex; flex-direction: column; gap: 10px;
  padding: 36px 0; align-items: center;
}
.msg-empty .logo-mark { width: 40px; height: 40px; margin-bottom: 6px; }
.msg-empty .big { font-size: 20px; font-weight: 600; color: var(--text); letter-spacing: -0.01em; }
.starter-grid { display: flex; flex-direction: column; gap: 2px; margin-top: 8px; max-width: 420px; width: 100%; }
.starter-card {
  text-align: left; padding: 8px 12px; border-radius: 8px;
  font-size: 13px; color: var(--text-2);
}
.starter-card:hover { background: var(--hover); color: var(--text); }

/* markdown content */
.bubble.md p { margin: 0 0 10px; }
.bubble.md p:last-child { margin-bottom: 0; }
.bubble.md h1, .bubble.md h2, .bubble.md h3 { margin: 14px 0 6px; font-weight: 600; }
.bubble.md ul, .bubble.md ol { margin: 4px 0 10px 22px; }
.bubble.md li { margin: 2px 0; }
.bubble.md code { background: var(--chip-bg); border-radius: 4px; padding: 1px 5px; font-size: 0.9em; }
.bubble.md blockquote { border-left: 2px solid var(--border-strong); margin: 8px 0; padding: 2px 12px; color: var(--text-2); }
.bubble.md a { color: var(--accent); }

/* tables: full width, hairline rows only */
.bubble.md .table-wrap { width: 100%; overflow-x: auto; margin: 10px 0; scrollbar-width: thin; }
.bubble.md table { width: 100%; border-collapse: collapse; border: none; }
.bubble.md th, .bubble.md td {
  text-align: left; border: none; border-bottom: 1px solid var(--rule);
  padding: 8px 16px 8px 0; font-size: 13px; line-height: 1.5; vertical-align: top;
}
.bubble.md th { font-weight: 600; color: var(--text); border-bottom-color: var(--rule-strong); white-space: nowrap; }
.bubble.md td { font-weight: 400; }
.bubble.md tr:last-child td { border-bottom: none; }

/* code blocks: bordered card, scrolls inside, copy pinned on the wrapper */
.bubble.md .code-block {
  display: block; width: fit-content; max-width: 100%; margin: 10px 0;
  background: var(--code-bg); border: 1px solid var(--code-border); border-radius: 10px;
}
.bubble.md .code-block pre {
  margin: 0; padding: 14px 40px 14px 16px; overflow-x: auto; scrollbar-width: thin;
  font-size: 12.5px; line-height: 1.5; background: none; border: none;
}
.bubble.md .code-block pre code { background: none; padding: 0; font-size: inherit; color: var(--text); }
.code-copy {
  position: sticky; top: 8px; float: right; z-index: 1;
  width: 24px; height: 24px; padding: 0; margin: 8px 8px -32px 0; border-radius: 6px;
  display: inline-flex; align-items: center; justify-content: center; color: var(--text-3);
}
.code-copy:hover { background: var(--active); color: var(--text); }
.code-copy .ico-check, .code-copy.copied .ico-copy { display: none; }
.code-copy.copied .ico-check { display: inline-flex; color: var(--ok); }
.code-copy .ico-copy { display: inline-flex; }
.code-block .s-keyword, .code-block .s-storage { color: var(--syn-keyword); }
.code-block .s-entity.s-name.s-function, .code-block .s-support.s-function, .code-block .s-variable.s-function { color: var(--syn-function); }
.code-block .s-entity.s-name.s-type, .code-block .s-entity.s-name.s-class, .code-block .s-entity.s-name.s-struct,
.code-block .s-entity.s-name.s-enum, .code-block .s-support.s-type, .code-block .s-support.s-class, .code-block .s-storage.s-type { color: var(--syn-type); }
.code-block .s-string { color: var(--syn-string); }
.code-block .s-constant { color: var(--syn-number); }
.code-block .s-comment { color: var(--syn-comment); font-style: italic; }
.code-block .s-punctuation { color: var(--text-2); }
.code-block .s-comment .s-punctuation { color: var(--syn-comment); }
.msg-footer { display: flex; align-items: center; gap: 6px; padding: 0 2px; opacity: 0; transition: opacity 0.12s ease; }
.msg-assistant:hover .msg-footer, .msg-footer:focus-within { opacity: 1; }
.msg-footer .msg-time { font-size: 11px; color: var(--text-3); }
.msg-footer button.icon-btn { width: 24px; min-width: 24px; height: 24px; color: var(--text-3); }
.msg-footer button.icon-btn:hover { color: var(--text); }

/* disclosures: reasoning + tool cards */
.collapsible > .collapsible-header {
  height: 24px; display: inline-flex; align-items: center; gap: 4px;
  padding: 0 6px 0 2px; margin-left: -2px; font-size: 12px; color: var(--text-3);
}
.collapsible > .collapsible-header:hover { background: var(--hover); color: var(--text-2); }
.chev { display: inline-flex; transition: transform 0.12s ease; }
.chev.open { transform: rotate(90deg); }
.collapsible > .collapsible-body { padding: 4px 0 6px 14px; border-left: 1px solid var(--border); margin-left: 6px; }
.reasoning-panel .collapsible-body { font-size: 12.5px; color: var(--text-2); white-space: pre-wrap; max-height: 260px; overflow-y: auto; }

.tool-group { display: flex; flex-direction: column; gap: 6px; }
.tool-card { border: 1px solid var(--border); background: var(--bg-elevated); border-radius: 8px; overflow: hidden; }
.tool-card .tool-head {
  min-height: 30px; display: flex; align-items: center; gap: 8px;
  padding: 0 10px; font-size: 12px; color: var(--text-3); cursor: pointer;
  transition: background 0.12s ease;
}
.tool-card .tool-head:hover { background: var(--hover); }
.tool-card .tool-name { font-family: var(--font-mono); font-size: 11.5px; font-weight: 500; color: var(--text); flex: none; }
.tool-card .tool-args { flex: 1; min-width: 0; color: var(--text-3); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--font-mono); font-size: 11px; }
.tool-status { display: inline-flex; align-items: center; gap: 6px; font-size: 11px; color: var(--text-3); flex: none; }
.tool-card .tool-body { border-top: 1px solid var(--border); padding: 8px 10px; }
.tool-output {
  font-family: var(--font-mono); font-size: 11.5px; padding: 2px 0; max-height: 240px; overflow: auto;
  white-space: pre-wrap; word-break: break-word; color: var(--text-2);
}
.coder-group { border: 1px solid var(--border); border-radius: 10px; background: var(--bg-elevated); overflow: hidden; }
.coder-group > .coder-head {
  display: flex; align-items: center; gap: 8px; padding: 8px 12px;
  cursor: pointer; font-size: 12.5px; font-weight: 500;
}
.coder-group > .coder-head:hover { background: var(--hover); }
.coder-group > .coder-head .spacer { margin-left: auto; color: var(--text-3); }
.coder-group > .coder-body { padding: 10px 12px; display: flex; flex-direction: column; gap: 8px; border-top: 1px solid var(--border); }

/* diffs */
.diff-view {
  font-family: var(--font-mono); font-size: 11.5px;
  border: 1px solid var(--border); border-radius: 6px;
  overflow: hidden; max-height: 300px; overflow-y: auto;
}
.diff-line { display: flex; padding: 1px 8px; white-space: pre-wrap; word-break: break-word; }
.diff-line.add { background: var(--diff-add-bg); color: var(--diff-add-fg); }
.diff-line.del { background: var(--diff-del-bg); color: var(--diff-del-fg); }
.diff-line.hunk { color: var(--text-3); background: var(--hover); }
.diff-line.ctx, .diff-line.plain { color: var(--text-2); }

/* ---------------- permission & question cards ---------------- */
.permission-card, .question-card {
  flex: none; max-width: 800px; width: calc(100% - 40px); margin: 0 auto 12px;
  border: 1px solid var(--border-strong); border-radius: 12px;
  background: var(--bg-elevated); box-shadow: var(--shadow-card);
  padding: 14px 16px; display: flex; flex-direction: column; gap: 10px;
}
.permission-card .pc-title, .question-card .qc-title { display: flex; align-items: center; gap: 8px; font-weight: 600; font-size: 13px; }
.permission-card .pc-title .tool-name { font-family: var(--font-mono); font-size: 12px; font-weight: 500; }
.permission-card .pc-detail, .question-card .qc-question { color: var(--text-2); font-size: 13px; }
.pc-detail strong { color: var(--text); font-weight: 600; }
.permission-card .pc-pathbox {
  font-family: var(--font-mono); font-size: 12px;
  background: var(--bg-input); border: 1px solid var(--border);
  border-radius: 6px; padding: 8px 10px;
  word-break: break-all; max-height: 130px; overflow: auto; white-space: pre-wrap;
}
.pc-buttons { display: flex; gap: 6px; flex-wrap: wrap; }
.pc-buttons button:not(.primary):not(.danger) { background: var(--hover); color: var(--text); }
.pc-buttons button:not(.primary):not(.danger):hover { background: var(--active); }
.qc-options { display: flex; flex-direction: column; gap: 4px; }
.qc-option {
  text-align: left; padding: 8px 12px; border-radius: 8px;
  display: flex; align-items: center; gap: 10px;
  border: 1px solid var(--border); color: var(--text);
}
.qc-option .qc-mark {
  width: 14px; height: 14px; flex: none; border-radius: 50%;
  border: 1.5px solid var(--text-3);
  display: inline-flex; align-items: center; justify-content: center;
}
.qc-option .qc-mark.multi { border-radius: 4px; }
.qc-option.selected { border-color: var(--accent-ring); background: var(--accent-soft); }
.qc-option.selected .qc-mark { border-color: var(--accent); background: var(--accent); color: var(--accent-fg); }
.qc-option .qc-mark .icon { width: 10px; height: 10px; }
.qc-note input { width: 100%; }

/* ---------------- composer ---------------- */
.composer {
  position: relative; flex: none; max-width: 800px; width: calc(100% - 40px);
  margin: 0 auto 18px; padding: 12px 12px 10px;
  background: var(--bg-elevated);
  border: 1px solid var(--border-strong); border-radius: 14px; box-shadow: var(--shadow-card);
  display: flex; flex-direction: column; gap: 8px;
  transition: border-color 0.12s ease, box-shadow 0.12s ease;
}
.composer:focus-within { border-color: var(--accent-ring); box-shadow: 0 0 0 3px var(--accent-soft); }
.composer textarea {
  flex: 1; background: transparent; border: none; box-shadow: none;
  padding: 4px 4px; min-height: 44px; max-height: 180px;
  font-size: 14px; line-height: 1.5; resize: none; width: 100%;
}
.composer textarea:focus { border: none; box-shadow: none; }
.composer-row-bottom { position: relative; display: flex; gap: 4px; align-items: center; min-height: 30px; flex-wrap: nowrap; }
.composer-row-top { display: flex; gap: 8px; align-items: center; }
.composer-row-top input { flex: 1; min-width: 0; }
.composer-row-top > button { flex: none; white-space: nowrap; }
.composer-plus-btn {
  width: 28px; height: 28px; padding: 0; border-radius: 6px;
  display: inline-flex; align-items: center; justify-content: center; color: var(--text-2); flex: none;
}
.popover-backdrop { position: fixed; inset: 0; z-index: 45; background: transparent; }
.composer-context-menu {
  position: absolute; bottom: calc(100% + 10px); left: 0; z-index: 60;
  width: 210px; padding: 6px;
  background: var(--bg-popover); border-radius: 12px; box-shadow: var(--shadow-pop);
  display: flex; flex-direction: column; gap: 2px;
}
.ccm-header { font-size: 11px; font-weight: 500; letter-spacing: 0.04em; text-transform: uppercase; color: var(--text-3); padding: 4px 10px 6px; display: flex; align-items: center; justify-content: space-between; }
.ccm-header button { padding: 0; font-size: 11.5px; color: var(--text-2); }
.ccm-search { width: 100%; height: 28px; padding: 0 10px; margin-bottom: 4px; font-size: 12.5px; }
.ccm-list { display: flex; flex-direction: column; gap: 2px; max-height: 200px; overflow-y: auto; scrollbar-width: thin; }
.ccm-empty { padding: 8px 10px; font-size: 12px; color: var(--text-3); }
.ccm-item {
  width: 100%; min-height: 30px; padding: 0 10px; border-radius: 7px;
  display: flex; align-items: center; gap: 10px;
  font-size: 13px; font-weight: 400; color: var(--text-2);
  text-align: left; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
.ccm-item:hover { background: var(--hover); color: var(--text); }
.ccm-item.active { color: var(--text); background: var(--hover); }
.ccm-item.active .icon { color: var(--text); }
.composer-dropdown { position: relative; display: inline-flex; align-items: center; }
.composer-dropdown.open { z-index: 55; }
.composer-select-btn {
  height: 28px; padding: 0 8px; border-radius: 6px;
  display: inline-flex; align-items: center; gap: 6px;
  font-size: 12px; font-weight: 500; color: var(--text-2);
  white-space: nowrap; max-width: 220px; overflow: hidden; text-overflow: ellipsis;
}
.composer-select-btn:hover, .composer-select-btn.open { color: var(--text); background: var(--hover); }
.composer-context-menu.align-right { left: auto; right: 0; }
.ccm-item-label { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.ccm-divider { height: 1px; margin: 4px 6px; background: var(--border); }
.ccm-submenu-row { position: relative; }
.composer-context-menu.model-menu { width: 280px; }
.ccm-list.model-list { max-height: min(320px, calc(100vh - 180px)); overflow-y: auto; overflow-x: hidden; scrollbar-width: thin; }
.ccm-list::-webkit-scrollbar { width: 5px; }
.ccm-list::-webkit-scrollbar-thumb { background: var(--active); border-radius: 999px; }
.ccm-model-name { color: var(--text); font-weight: 500; }
.ccm-model-effort { margin-left: 8px; font-size: 12px; font-weight: 400; color: var(--text-3); }
.ccm-row-chevron { display: inline-flex; align-items: center; color: var(--text-3); }
.composer-flyout-right { position: fixed; z-index: 65; padding-left: 6px; }
.composer-flyout-card {
  width: 150px; padding: 6px;
  background: var(--bg-popover); border-radius: 12px; box-shadow: var(--shadow-pop);
  display: flex; flex-direction: column; gap: 2px;
}
.composer-send-btn {
  width: 30px; height: 30px; min-width: 30px; padding: 0;
  border-radius: 50%; background: var(--accent); color: var(--accent-fg);
  display: inline-flex; align-items: center; justify-content: center; flex: none;
}
.composer-send-btn:hover:not(:disabled) { background: var(--accent-hover); color: var(--accent-fg); }
.composer-send-btn:disabled { background: var(--active); color: var(--text-3); opacity: 1; }
.composer-spacer { flex: 1; }
.composer-hint { font-size: 11px; color: var(--text-3); }

/* ---------------- modals ---------------- */
.modal-backdrop { position: fixed; inset: 0; z-index: 90; background: rgba(0, 0, 0, 0.5); display: flex; align-items: center; justify-content: center; }
.modal {
  width: 440px; max-width: calc(100vw - 40px);
  background: var(--bg-popover); border-radius: 12px; box-shadow: var(--shadow-pop);
  padding: 18px; display: flex; flex-direction: column; gap: 12px;
}
.modal h3 { font-size: 15px; font-weight: 600; }
.modal input { width: 100%; }
.modal-actions { display: flex; justify-content: flex-end; gap: 8px; }

/* ---------------- right inspector ---------------- */
.right-panel-head { padding: 0 var(--controls-w) 0 16px; border-bottom-color: transparent; }
.head-title { font-size: 12px; font-weight: 500; color: var(--text-3); }
.right-panel-body { flex: 1; overflow-y: auto; padding: 4px 8px 16px; display: flex; flex-direction: column; gap: 4px; scrollbar-width: thin; }
.inspector-section { display: flex; flex-direction: column; padding: 4px 0; }
.inspector-section + .inspector-section { border-top: 1px solid var(--border); }
.section-label, .section-head {
  height: 28px; padding: 0 8px; display: flex; align-items: center; gap: 8px;
  font-size: 11px; font-weight: 500; letter-spacing: 0.04em; text-transform: uppercase; color: var(--text-3);
}
button.section-head { width: 100%; text-align: left; border-radius: 6px; }
button.section-head:hover { color: var(--text-2); }
.section-head .section-name { flex: 1; }
.section-head .icon { width: 14px; height: 14px; }
.section-body { display: flex; flex-direction: column; gap: 6px; padding: 4px 8px 8px; min-width: 0; }
.info-row, .list-row, .panel-row { display: flex; align-items: center; gap: 8px; min-width: 0; font-size: 12.5px; color: var(--text-2); }
.info-row .icon, .list-row .icon { color: var(--text-3); }
.info-row .grow, .list-row .grow, .panel-row .grow { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.list-row { padding: 6px 8px; margin: 0 -8px; border-radius: 6px; color: var(--text); }
.list-row:hover { background: var(--hover); }
.meter { height: 3px; border-radius: 2px; background: var(--active); overflow: hidden; }
.meter-fill { height: 100%; background: var(--text-3); border-radius: 2px; }
.empty-hint.inline { font-size: 12px; color: var(--text-3); padding: 2px 0; }
.clickable { cursor: pointer; }
.proc-command { font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text); }
.path-line { font-size: 10.5px; color: var(--text-3); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.log-view { max-height: 340px; }
.log-line { display: flex; gap: 8px; align-items: baseline; }
.log-line .mono { font-size: 11.5px; white-space: pre-wrap; word-break: break-word; }
.activity-row {
  align-self: flex-start; height: 24px; padding: 0 6px 0 2px; margin-left: -2px;
  display: inline-flex; align-items: center; gap: 4px; font-size: 12px; color: var(--text-3);
}
.activity-row:hover { background: var(--hover); color: var(--text-2); }
.panel-card {
  background: var(--bg-elevated); border: 1px solid var(--border); border-radius: 8px;
  padding: 10px 12px; font-size: 12.5px;
  display: flex; flex-direction: column; gap: 8px; min-width: 0; overflow: hidden;
}
.panel-card.md { font-size: 12.5px; line-height: 1.55; }
.panel-card .mono { font-size: 11.5px; color: var(--text-2); }
.panel-section-title {
  font-size: 11px; font-weight: 600; text-transform: uppercase; letter-spacing: 0.06em;
  color: var(--text-3); display: flex; align-items: center; gap: 6px; flex-wrap: wrap;
}
.panel-section-title.grow { flex: 1; }
.panel-actions { display: flex; gap: 8px; }
.plan-task { display: flex; gap: 10px; align-items: flex-start; padding: 3px 0; }
.plan-task .task-icon { color: var(--text-3); margin-top: 1px; }
.plan-task .task-text { flex: 1; line-height: 1.45; }
.task-in_progress .task-icon { color: var(--accent); }
.task-completed .task-icon { color: var(--ok); }
.task-completed .task-text { color: var(--text-3); }
.file-row { display: flex; align-items: center; gap: 8px; min-width: 0; padding: 4px 6px; margin: 0 -6px; border-radius: 6px; }
.file-row:hover { background: var(--hover); }
.file-row .file-main { flex: 1; min-width: 0; cursor: pointer; }
.file-row .file-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 12.5px; }
.file-kind {
  width: 14px; flex: none; text-align: center;
  font-family: var(--font-mono); font-size: 11px; font-weight: 600; color: var(--text-3);
}
.file-kind.created { color: var(--ok); }
.file-kind.modified { color: var(--warn); }
.file-kind.deleted { color: var(--err); }
.tree-state { display: inline-flex; align-items: center; gap: 6px; font-size: 12px; }
.tree-state.dirty { color: var(--warn); }
.tree-state.clean { color: var(--ok); }
.empty-state {
  flex: 1; min-height: 160px;
  display: flex; flex-direction: column; align-items: center; justify-content: center;
  gap: 6px; text-align: center; padding: 24px 16px;
}
.empty-state .icon { width: 22px; height: 22px; color: var(--text-3); margin-bottom: 6px; }
.empty-state .empty-title { font-size: 13px; font-weight: 500; color: var(--text-2); }
.empty-state .empty-hint { font-size: 12px; color: var(--text-3); max-width: 240px; line-height: 1.45; }

/* ---------------- settings & usage ---------------- */
.page-header { padding: 0 12px 0 20px; }
.page-header h2 { font-size: 14px; font-weight: 500; flex: 1; }
.settings-layout { flex: 1; display: flex; min-height: 0; }
.settings-nav { width: 188px; border-right: 1px solid var(--border); padding: 12px 10px; display: flex; flex-direction: column; gap: 2px; flex: none; }
.settings-nav button {
  height: 30px; padding: 0 10px; display: flex; align-items: center; width: 100%; text-align: left;
  font-size: 13px; color: var(--text-2); white-space: nowrap; border-radius: 8px;
}
.settings-nav button:hover { background: var(--hover); color: var(--text); }
.settings-nav button.active { background: var(--active); color: var(--text); font-weight: 500; }
.settings-content { flex: 1; overflow-y: auto; padding: 20px 24px; display: flex; flex-direction: column; gap: 12px; scrollbar-width: thin; }
.usage-layout { flex: 1; display: flex; flex-direction: column; min-height: 0; padding: 20px 24px; gap: 14px; overflow-y: auto; }
.usage-metrics { display: flex; gap: 12px; flex-wrap: wrap; }
.metric-card {
  flex: 1; min-width: 150px;
  background: var(--bg-elevated); border: 1px solid var(--border); border-radius: 10px;
  padding: 12px 14px; display: flex; flex-direction: column; gap: 4px;
}
.metric-card .metric-value { font-size: 20px; font-weight: 600; letter-spacing: -0.01em; }
.metric-card .metric-label { font-size: 11px; color: var(--text-3); text-transform: uppercase; letter-spacing: 0.06em; }
.usage-toolbar { display: flex; gap: 8px; align-items: center; justify-content: flex-end; }
table.data-table { width: 100%; border-collapse: collapse; font-size: 12.5px; }
table.data-table th, table.data-table td {
  text-align: left; padding: 8px 10px; border-bottom: 1px solid var(--border);
  white-space: nowrap; max-width: 260px; overflow: hidden; text-overflow: ellipsis;
}
table.data-table th { color: var(--text-3); font-size: 11px; font-weight: 500; text-transform: uppercase; letter-spacing: 0.06em; position: sticky; top: 0; background: var(--bg-chat); }
table.data-table tr:hover td { background: var(--hover); }
table.data-table td.empty-cell { text-align: center; color: var(--text-3); padding: 28px 0; }
.settings-row {
  display: flex; align-items: center; gap: 10px; padding: 6px 8px;
  border: 1px solid transparent; border-radius: 8px; min-width: 0;
  transition: background 0.12s ease, border-color 0.12s ease;
}
.settings-row:hover { background: var(--hover); }
.settings-row.selected { background: var(--hover); border-color: var(--border-strong); }
.settings-row > button { flex: none; white-space: nowrap; }
.settings-row .grow { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
.settings-empty { color: var(--text-3); text-align: center; padding: 30px 0; }

/* ---------------- toast ---------------- */
.toast {
  position: fixed; bottom: 20px; left: 50%; transform: translateX(-50%);
  background: var(--bg-popover); border-radius: 8px; box-shadow: var(--shadow-pop);
  padding: 10px 16px; font-size: 13px; z-index: 100;
  max-width: 70vw; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    const PLATFORMS: [&str; 3] = ["windows", "macos", "linux"];

    #[test]
    fn theme_class_matches_mode() {
        assert_eq!(theme_class(true), "theme-dark");
        assert_eq!(theme_class(false), "theme-light");
    }

    #[test]
    fn generates_distinct_dark_and_light_stylesheets_per_platform() {
        for platform in PLATFORMS {
            let dark = build_stylesheet_for(platform, true);
            let light = build_stylesheet_for(platform, false);
            assert!(!dark.is_empty() && !light.is_empty(), "{platform} styles empty");
            assert_ne!(dark, light, "{platform} dark and light styles differ");
        }
    }

    #[test]
    fn native_scrollbars_follow_the_theme() {
        for platform in PLATFORMS {
            assert!(build_stylesheet_for(platform, true).contains("color-scheme: dark;"));
            assert!(build_stylesheet_for(platform, false).contains("color-scheme: light;"));
        }
    }

    #[test]
    fn windows_uses_fluent_fonts() {
        for dark in [true, false] {
            let style = build_stylesheet_for("windows", dark);
            assert!(style.contains("Segoe UI"), "windows font missing");
            assert!(style.contains("Cascadia Code"), "windows mono font missing");
            assert!(style.contains("--controls-w: 144px;"), "window controls reserve missing");
        }
    }

    #[test]
    fn macos_uses_native_fonts_and_keeps_traffic_lights() {
        for dark in [true, false] {
            let style = build_stylesheet_for("macos", dark);
            assert!(style.contains("-apple-system"), "macos font missing");
            assert!(style.contains("SF Mono"), "macos mono font missing");
            assert!(style.contains("--controls-w: 0px;"), "macos has no custom controls");
            assert!(style.contains("0 10px 0 80px"), "traffic light inset missing");
        }
    }

    #[test]
    fn linux_falls_back_to_system_fonts() {
        for dark in [true, false] {
            assert!(build_stylesheet_for("linux", dark).contains("Ubuntu"), "linux font missing");
        }
    }

    #[test]
    fn one_restrained_accent_per_mode() {
        for platform in PLATFORMS {
            assert!(build_stylesheet_for(platform, true).contains("--accent: #6d78e8;"));
            assert!(build_stylesheet_for(platform, false).contains("--accent: #4f5bd5;"));
        }
    }

    #[test]
    fn layered_dark_surfaces_and_hairlines() {
        let dark = build_stylesheet_for("windows", true);
        assert!(dark.contains("--bg-app: #0e0e10;"));
        assert!(dark.contains("--border: rgba(255, 255, 255, 0.06);"));
        assert!(build_stylesheet_for("windows", false).contains("--bg-chat: #ffffff;"));
    }

    #[test]
    fn styles_cover_the_core_layout_sections() {
        for platform in PLATFORMS {
            for dark_mode in [true, false] {
                let style = build_stylesheet_for(platform, dark_mode);
                for section in [
                    ".app-shell", ".sidebar", ".chat-area", ".composer", ".right-panel",
                    ".settings-layout", ".permission-card", ".diff-view", ".toast",
                    ".window-controls", ".inspector-section", ".empty-state",
                ] {
                    assert!(style.contains(section), "{platform} missing {section}");
                }
            }
        }
    }

    #[test]
    fn base_rules_stay_on_tokens() {
        // Only the fixed-brand logo mark (3 colours) and the white close
        // glyph may hard-code a hex colour; everything else uses `var(--…)`.
        let hex_colours = BASE_CSS
            .split('#')
            .skip(1)
            .filter(|rest| rest.chars().take_while(|c| c.is_ascii_hexdigit()).count() >= 6)
            .count();
        assert!(hex_colours <= 4, "too many hard-coded colours in base rules: {hex_colours}");
    }

    fn with(change: impl FnOnce(&mut Appearance)) -> String {
        let mut appearance = Appearance::default();
        change(&mut appearance);
        build_stylesheet_with("windows", &appearance)
    }

    #[test]
    fn default_appearance_keeps_current_size_and_ligatures() {
        let style = with(|_| {});
        assert!(style.contains("--font-scale: 1;"));
        assert!(style.contains("--ui-ligatures: normal;"));
        assert!(style.contains("--code-ligatures: normal;"));
        assert!(style.contains("--font-code: \"Cascadia Code\", \"Cascadia Mono\", Consolas, monospace;"));
        assert!(style.contains("--font-mono: var(--font-code);"));
    }

    #[test]
    fn chosen_ui_and_code_fonts_land_in_the_stacks() {
        let style = with(|a| {
            a.ui_font = UiFont::Inter;
            a.code_font = CodeFont::JetBrainsMono;
        });
        assert!(style.contains("--font-ui: \"Inter\", \"Inter Variable\", \"Segoe UI Variable Text\""));
        assert!(style.contains("--font-code: \"JetBrains Mono\", \"Cascadia Mono\", Consolas, monospace;"));
        let style = with(|a| {
            a.ui_font = UiFont::Custom;
            a.ui_font_custom = "My Font\"; } body { x: y".into();
            a.code_font = CodeFont::Custom;
            a.code_font_custom = "Fira Code".into();
        });
        assert!(style.contains("--font-ui: \"My Font  body  x y\", "), "custom family must be sanitized");
        assert!(!style.contains("} body { x"));
        assert!(style.contains("--font-code: \"Fira Code\", \"Cascadia Mono\""));
    }

    #[test]
    fn monospace_ui_font_shrinks_and_drops_ligatures() {
        let style = with(|a| a.ui_font = UiFont::CascadiaCode);
        assert!(style.contains("--font-ui: \"Cascadia Code\", \"Cascadia Mono\", Consolas, monospace;"));
        assert!(style.contains("--font-scale: 0.94;"));
        assert!(style.contains("--ui-ligatures: none;"));
    }

    #[test]
    fn code_ligature_toggle_changes_the_rules() {
        let on = with(|a| a.code_ligatures = true);
        let off = with(|a| a.code_ligatures = false);
        assert!(on.contains("--code-features: \"calt\" 1, \"liga\" 1;"));
        assert!(off.contains("--code-ligatures: none;") && off.contains("--code-features: \"calt\" 0, \"liga\" 0;"));
        assert!(on.contains("font-variant-ligatures: var(--code-ligatures);"));
    }

    #[test]
    fn font_size_scales_root_variable_and_pixel_sizes() {
        assert!(with(|a| a.font_size = crate::prefs::FontSize::Small).contains("--font-scale: 0.92;"));
        let large = with(|a| a.font_size = crate::prefs::FontSize::Large);
        assert!(large.contains("--font-scale: 1.1;"));
        assert!(large.contains("font-size: calc(13px * var(--font-scale));"));
        assert!(!large.contains("font-size: 13px;"));
    }

    #[test]
    fn system_theme_follows_prefers_color_scheme() {
        let system = with(|a| a.theme = ThemeChoice::System);
        assert!(system.contains("@media (prefers-color-scheme: light)"));
        assert!(system.contains("--bg-chat: #ffffff;") && system.contains("--bg-app: #0e0e10;"));
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            assert!(!with(|a| a.theme = theme).contains("prefers-color-scheme"));
        }
    }

    #[test]
    fn detected_os_build_matches_explicit_platform() {
        assert_eq!(build_stylesheet(true), build_stylesheet_for(std::env::consts::OS, true));
        assert_eq!(build_stylesheet(false), build_stylesheet_for(std::env::consts::OS, false));
    }
}
