//! Platform-native theming. Detects the runtime OS and generates a complete
//! stylesheet in that platform's visual language:
//! * Windows — Windows 11 Fluent Design (Segoe UI, Mica-like layers, `#0078d4` accent)
//! * macOS — native macOS vibrancy (SF Pro, hairlines, `#0a84ff` accent)
//! * everything else — a neutral dark-first Linux style

/// The CSS class applied to `.app-shell` for dark/light scoping.
pub fn theme_class(dark_mode: bool) -> &'static str {
    if dark_mode {
        "theme-dark"
    } else {
        "theme-light"
    }
}

/// Builds the full stylesheet for the detected OS and the given dark mode.
pub fn build_stylesheet(dark_mode: bool) -> String {
    build_stylesheet_for(std::env::consts::OS, dark_mode)
}

/// Builds the full stylesheet for an explicit platform (`"windows"`,
/// `"macos"`, anything else = Linux/other) and the given dark mode. Split out
/// of [`build_stylesheet`] so every platform's output is unit-testable.
pub fn build_stylesheet_for(platform: &str, dark_mode: bool) -> String {
    let (font_ui, font_mono, accent, accent_text, bg_app, bg_sidebar, bg_card, bg_hover, border, radius_ctrl, radius_card) = if platform == "macos" {
        // macOS: obsidian/zinc layered surfaces over the system base and
        // system blue accent (`bg_app` stays the platform baseline token).
        (
            "-apple-system, BlinkMacSystemFont, \"SF Pro Text\", \"SF Pro Display\", system-ui, sans-serif",
            "\"SF Mono\", \"Menlo\", monospace",
            if dark_mode { "#0a84ff" } else { "#007aff" },
            if dark_mode { "#ffffff" } else { "#ffffff" },
            if dark_mode { "#1c1c1e" } else { "#f5f5f7" },
            if dark_mode { "#0f1013" } else { "#f8f8fa" },
            if dark_mode { "#1f2128" } else { "#ffffff" },
            if dark_mode { "#272a33" } else { "#eceef2" },
            if dark_mode { "1px solid rgba(255, 255, 255, 0.1)" } else { "1px solid rgba(0, 0, 0, 0.08)" },
            "8px",
            "10px",
        )
    } else if platform == "windows" {
        // Windows: Fluent Design / Mica over the obsidian/zinc layer palette.
        (
            "\"Segoe UI Variable Text\", \"Segoe UI\", system-ui, sans-serif",
            "\"Cascadia Code\", \"Consolas\", monospace",
            if dark_mode { "#60cdff" } else { "#0067c0" },
            if dark_mode { "#001a33" } else { "#ffffff" },
            if dark_mode { "#191919" } else { "#f3f3f3" },
            if dark_mode { "#0f1013" } else { "#f8f8fa" },
            if dark_mode { "#1f2128" } else { "#ffffff" },
            if dark_mode { "#272a33" } else { "#eceef2" },
            if dark_mode { "1px solid rgba(255, 255, 255, 0.08)" } else { "1px solid rgba(0, 0, 0, 0.08)" },
            "6px",
            "8px",
        )
    } else {
        // Linux / other: neutral dark-first style over the shared palette.
        (
            "system-ui, \"Ubuntu\", \"Cantarell\", sans-serif",
            "\"JetBrains Mono\", \"DejaVu Sans Mono\", monospace",
            if dark_mode { "#4493f8" } else { "#1a73e8" },
            "#ffffff",
            if dark_mode { "#191919" } else { "#f3f3f3" },
            if dark_mode { "#0f1013" } else { "#f8f8fa" },
            if dark_mode { "#1f2128" } else { "#ffffff" },
            if dark_mode { "#272a33" } else { "#eceef2" },
            if dark_mode { "1px solid rgba(255, 255, 255, 0.08)" } else { "1px solid rgba(0, 0, 0, 0.08)" },
            "6px",
            "8px",
        )
    };

    let color_scheme = if dark_mode { "dark" } else { "light" };

    let (text, text_secondary, text_tertiary, shadow) = if dark_mode {
        ("#f3f3f3", "#c8c8c8", "#8a8a8a", "0 4px 14px rgba(0, 0, 0, 0.35)")
    } else {
        ("#1a1a1a", "#4a4a4a", "#777777", "0 4px 14px rgba(0, 0, 0, 0.08)")
    };

    // Modern conversation column tokens: obsidian canvas (dark) / airy paper
    // (light) plus the user bubble surface.
    let (chat_canvas, _user_bubble_bg, _user_bubble_border, _reasoning_border, _reasoning_bg) = if dark_mode {
        (
            "#111317".to_string(),
            "#222733".to_string(),
            "1px solid rgba(255, 255, 255, 0.1)".to_string(),
            "1px solid rgba(255, 255, 255, 0.06)".to_string(),
            "rgba(255, 255, 255, 0.02)".to_string(),
        )
    } else {
        (
            "#f6f7f9".to_string(),
            format!("color-mix(in srgb, {accent} 10%, #ffffff)"),
            format!("1px solid color-mix(in srgb, {accent} 25%, transparent)"),
            "1px solid rgba(0, 0, 0, 0.06)".to_string(),
            "rgba(0, 0, 0, 0.02)".to_string(),
        )
    };

    let _focus_ring = if platform == "windows" {
        format!("0 0 0 2px {bg_app}, 0 0 0 4px {accent}")
    } else {
        format!("0 0 0 3px color-mix(in srgb, {accent} 40%, transparent)")
    };

    // On macOS the traffic lights sit at x: 14..72 inside the full-size
    // content view, so the brand strip's left padding must clear them.
    let sidebar_brand_padding = if platform == "macos" {
        "0 10px 0 80px"
    } else {
        "0 12px 0 14px"
    };

    format!(r#"
* {{ margin: 0; padding: 0; box-sizing: border-box; user-select: none; -webkit-user-select: none; }}
/* Native scrollbars / form controls follow the app theme (WebView2 dark Fluent scrollbars). */
:root {{ color-scheme: {color_scheme}; }}
html, body {{ height: 100%; overflow: hidden; }}
body {{
  font-family: {font_ui};
  font-size: 13.5px;
  color: {text};
  background: {bg_app};
  -webkit-font-smoothing: antialiased;
  user-select: none;
  -webkit-user-select: none;
  cursor: default;
}}
#main {{ height: 100vh; width: 100vw; display: flex; overflow: hidden; }}
code, pre, .mono {{ font-family: {font_mono}; }}

/* Selectable text: chat content, alerts and inputs only. */
.msg-user .bubble, .msg-user .bubble *,
.msg-assistant .bubble, .msg-assistant .bubble *,
.reasoning-panel .collapsible-body,
.tool-output, .diff-view, .diff-line,
.toast,
.permission-card .pc-detail, .permission-card .pc-pathbox,
.question-card .qc-question,
.badge.err, .badge.warn {{
  user-select: text;
  -webkit-user-select: text;
}}
input, textarea {{
  user-select: text;
  -webkit-user-select: text;
  cursor: text;
}}

/* ---------------- shell layout ---------------- */
.app-shell {{
  display: flex; height: 100vh; width: 100vw;
  background: {bg_app}; color: {text};
  overflow: hidden;
}}
.sidebar {{
  width: 264px; min-width: 264px; height: 100%;
  background: {bg_sidebar}; border-right: none;
  display: flex; flex-direction: column;
}}
.main-column {{
  flex: 1 1 auto; min-width: 0; height: 100%;
  display: flex; flex-direction: column;
  background: {bg_app};
}}
.right-panel {{
  width: 340px; min-width: 340px; height: 100%;
  background: {bg_sidebar}; border-left: none;
  display: flex; flex-direction: column;
  overflow: hidden;
}}

/* ---------------- generic controls ---------------- */
button, .btn {{
  font: inherit; color: {text_secondary}; background: transparent;
  border: none; border-radius: 6px;
  -webkit-appearance: none; appearance: none;
  padding: 5px 10px;
  cursor: pointer;
  transition: background 0.12s ease, color 0.12s ease, opacity 0.12s ease;
}}
button:hover {{ background: transparent; color: {text}; }}
button:active {{ opacity: 0.8; }}
button:focus-visible, input:focus-visible, textarea:focus-visible, select:focus-visible {{
  outline: none; box-shadow: none;
}}
button.primary {{
  background: {accent}; color: {accent_text}; border: none;
  border-radius: 8px; padding: 5px 13px; font-weight: 500;
}}
button.primary:hover {{ background: {accent}; color: {accent_text}; filter: brightness(1.08); }}
button.subtle {{ background: transparent; border-color: transparent; color: {text_secondary}; }}
button.subtle:hover {{ background: transparent; color: {text}; }}
button.danger {{ color: #ff8a8a; }}
button.icon-btn {{ padding: 2px 6px; line-height: 1; display: inline-flex; align-items: center; justify-content: center; }}
button:disabled {{ opacity: 0.45; cursor: default; }}

input, textarea, select {{
  font: inherit; color: {text};
  background: transparent; border: none; border-radius: 6px;
  padding: 5px 8px;
  outline: none;
  -webkit-appearance: none; appearance: none;
}}
input::placeholder, textarea::placeholder {{ color: {text_tertiary}; }}
select {{
  appearance: none; -webkit-appearance: none;
  color: {text_tertiary}; font-size: 12px;
  padding: 3px 6px; cursor: pointer;
}}
select:hover {{ color: {text}; background: transparent; }}
label {{ color: {text_secondary}; font-size: 12px; }}

/* ---------------- badges & pills ---------------- */
.badge {{
  display: inline-flex; align-items: center; gap: 4px;
  background: transparent; border: none;
  padding: 1px 4px;
  font-size: 11px; font-weight: 500;
  color: {text_tertiary};
  white-space: nowrap;
}}
.badge.accent {{ background: color-mix(in srgb, {accent} 18%, transparent); color: {accent}; border-color: transparent; }}
.badge.ok {{ color: #4ecf7a; }}
.badge.warn {{ color: #e6b455; }}
.badge.err {{ color: #ff8a8a; }}
.badge.run {{ color: {accent}; }}
.pill {{
  display: inline-flex; align-items: center; gap: 4px;
  background: transparent; border: none;
  padding: 1px 4px;
  font-size: 11px; font-weight: 500; color: {text_tertiary};
  max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}}
.status-dot {{
  display: inline-block; width: 8px; height: 8px; border-radius: 50%;
  background: {text_tertiary}; flex: none;
}}
.status-dot.generating, .status-dot.created {{ background: {accent}; animation: pulse 1.2s ease-in-out infinite; }}
.status-dot.awaiting_permission, .status-dot.awaiting_input {{ background: #e6b455; }}
.status-dot.done {{ background: #4ecf7a; }}
.status-dot.failed {{ background: #ff8a8a; }}
.status-dot.cancelled {{ background: {text_tertiary}; }}
@keyframes pulse {{ 0%, 100% {{ opacity: 1; }} 50% {{ opacity: 0.35; }} }}

/* ---------------- sidebar ---------------- */
.sidebar-brand {{
  height: 44px; padding: {sidebar_brand_padding};
  display: flex; align-items: center; justify-content: space-between;
  border-bottom: none;
  flex: none;
}}
.brand-mark {{ font-size: 13px; color: {accent}; line-height: 1; flex: none; }}
.brand-title {{ font-size: 14px; font-weight: 600; letter-spacing: -0.2px; color: {text}; flex: 1; min-width: 0; }}
.sidebar-toolbar {{
  display: flex; align-items: center; gap: 6px; margin-left: auto;
  flex: none;
}}
.sidebar-toolbar button {{
  width: 26px; height: 26px; padding: 0;
  display: inline-flex; align-items: center; justify-content: center;
  color: {text_tertiary}; flex: none;
}}
.sidebar-toolbar button:hover {{ background: transparent; color: {text}; }}
.sidebar-toolbar button:disabled {{ opacity: 0.35; cursor: default; }}
.sidebar-top-controls {{
  padding: 6px 12px 8px;
  display: flex; flex-direction: column; gap: 2px;
  flex: none;
}}
.sidebar .new-conv-btn {{
  width: 100%; height: 34px; min-height: 34px; max-height: 34px;
  padding: 0 12px; margin-bottom: 6px;
  display: flex; align-items: center; justify-content: flex-start; gap: 8px;
  border-radius: 8px; font-size: 13px; font-weight: 500; color: {text};
  background: color-mix(in srgb, {text} 5%, transparent) !important;
  border: 1px solid color-mix(in srgb, {text} 8%, transparent) !important;
}}
.sidebar .new-conv-btn:hover {{
  background: color-mix(in srgb, {text} 8%, transparent) !important;
  color: {text};
}}
.sidebar .sidebar-nav-item, .sidebar-footer button {{
  width: 100%; height: 30px; min-height: 30px; max-height: 30px;
  padding: 0 10px;
  display: flex; align-items: center; justify-content: flex-start; gap: 10px;
  font-size: 13px; font-weight: 500; color: {text_secondary};
  background: transparent !important; border: none !important; border-radius: 8px;
  text-align: left; white-space: nowrap;
}}
.sidebar .sidebar-nav-item:hover, .sidebar-footer button:hover {{
  background: transparent !important; color: {text};
}}
.sidebar .sidebar-nav-item.active, .sidebar-footer button.active {{
  color: {text}; font-weight: 600;
}}
.sidebar-nav-item.active .sidebar-nav-icon {{ color: {accent}; }}
.sidebar-nav-icon {{
  width: 16px; display: inline-flex; align-items: center; justify-content: center;
  color: {text_tertiary}; flex: none;
}}
.sidebar-search {{ flex: none; }}
.sidebar-search input {{
  width: 100%; height: 28px; font-size: 12.5px;
  background: transparent; border: none; padding: 0 8px; border-radius: 0;
  color: {text};
}}
.sidebar-search input:focus {{ background: transparent; border: none; }}
.sidebar-tree {{
  flex: 1; overflow-y: auto; padding: 0 8px 12px;
  scrollbar-width: thin;
}}
.sidebar-section-header {{
  height: 28px; padding: 0 10px; margin-top: 8px;
  display: flex; align-items: center; justify-content: space-between;
  font-size: 12px; font-weight: 500; color: {text_tertiary};
  flex: none;
}}
.sidebar-section-actions {{ display: flex; align-items: center; gap: 2px; }}
.sidebar-section-header .icon-btn {{
  width: 22px; height: 22px; padding: 0;
  display: inline-flex; align-items: center; justify-content: center;
  color: {text_tertiary}; flex: none;
}}
.sidebar-section-header .icon-btn:hover {{ background: transparent; color: {text}; }}
.project-group {{ margin-top: 0; }}
.project-row {{
  position: relative;
  height: 30px; min-height: 30px; max-height: 30px;
  padding: 0 8px 0 10px; border-radius: 8px;
  display: flex; align-items: center; gap: 8px;
  color: {text_secondary}; font-size: 13px; font-weight: 500;
  cursor: pointer; background: transparent;
}}
.project-row:hover {{ background: transparent; color: {text}; }}
.project-row.selected {{ background: transparent; color: {text}; font-weight: 500; }}
.project-folder-icon {{
  width: 16px; height: 16px; display: inline-flex; align-items: center; justify-content: center;
  color: {text_tertiary}; flex: none;
}}
.project-row .project-name {{
  flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}}
.project-menu-btn, button.project-menu-btn,
.project-new-btn, button.project-new-btn {{
  width: 22px; height: 22px; padding: 0;
  display: inline-flex; align-items: center; justify-content: center;
  color: {text_tertiary}; opacity: 0; flex: none;
  background: transparent; border: none;
}}
.project-row:hover .project-menu-btn,
.project-row:hover .project-new-btn {{ color: {text_tertiary}; }}
.project-row:hover .project-menu-btn:hover,
.project-row:hover .project-new-btn:hover {{ color: {text}; }}
.project-row:hover .project-menu-btn,
.project-row:hover .project-new-btn,
.project-row.menu-open .project-menu-btn,
.project-row.menu-open .project-new-btn {{ opacity: 1; }}
.project-runs {{
  margin-left: 0; padding-left: 0;
  display: flex; flex-direction: column; gap: 1px;
  margin-top: 0;
}}
.run-item {{
  position: relative;
  height: 32px; min-height: 32px; max-height: 32px;
  padding: 0 8px 0 28px; border-radius: 8px;
  display: flex; align-items: center; gap: 6px;
  cursor: pointer; font-size: 13px; color: {text_secondary};
  background: transparent;
}}
.run-item:hover {{ background: color-mix(in srgb, {text} 4%, transparent); color: {text}; }}
.run-item.active {{
  background: color-mix(in srgb, {text} 9%, transparent) !important;
  color: {text}; font-weight: 500;
}}
.run-item.active .run-title {{ color: {text}; font-weight: 500; }}
.run-item .status-dot {{ width: 6px; height: 6px; flex: none; }}
.run-item .run-title {{
  flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 13px;
}}
.run-item .run-age {{
  font-size: 11.5px; color: {text_tertiary}; flex: none; padding-right: 4px;
}}
.run-item .run-action, button.run-action {{
  display: none; width: 22px; height: 22px; padding: 0;
  align-items: center; justify-content: center;
  color: {text_tertiary}; flex: none;
  background: transparent; border: none;
}}
.run-item:hover .run-age, .run-item.active .run-age, .run-item.menu-open .run-age {{
  display: none;
}}
.run-item:hover .run-action, .run-item.active .run-action, .run-item.menu-open .run-action {{
  display: inline-flex;
}}
.run-item:hover .run-action:hover {{ color: {text}; }}
.see-all-btn {{
  height: 28px; min-height: 28px; padding: 0 8px 0 28px;
  display: flex; align-items: center; justify-content: flex-start;
  font-size: 12px; color: {text_tertiary};
  background: transparent !important; border: none !important;
  text-align: left;
}}
.see-all-btn:hover {{ background: transparent !important; color: {text_secondary}; }}
.row-context-menu {{
  position: absolute; right: 6px; top: 26px; z-index: 50;
  min-width: 128px; padding: 4px;
  background: {bg_card}; border: {border};
  border-radius: 8px; box-shadow: {shadow};
  display: flex; flex-direction: column; gap: 1px;
}}
.sidebar .row-context-menu button {{
  width: 100%; height: 26px; padding: 0 10px;
  display: flex; align-items: center; justify-content: flex-start;
  font-size: 12px; font-weight: 500; text-align: left;
  background: transparent !important; border: none !important; border-radius: 5px;
  color: {text};
}}
.sidebar .row-context-menu button:hover {{
  background: {bg_hover} !important;
}}
.sidebar .row-context-menu button.danger {{
  color: #ff8a8a;
}}
.sidebar button, .sidebar input {{
  background: transparent !important;
  border: none !important;
  box-shadow: none !important;
  -webkit-appearance: none;
  appearance: none;
}}
.sidebar-footer {{
  border-top: none; padding: 8px 12px 12px;
  display: flex; flex-direction: column;
  flex: none;
}}

/* ---------------- chat header ---------------- */
.chat-header {{
  height: 44px; padding: 0 16px;
  border-bottom: none;
  background: transparent;
  display: flex; align-items: center; justify-content: space-between; gap: 10px;
  flex: none;
}}
.chat-title {{ font-size: 14.5px; font-weight: 600; flex: 0 1 auto; min-width: 60px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }}
.chat-header-meta {{ display: flex; align-items: center; gap: 8px; flex: 1; min-width: 0; overflow: hidden; }}
.chat-header-meta .pill {{ max-width: 220px; flex-shrink: 1; }}
.chat-header-actions {{ display: flex; align-items: center; gap: 4px; flex: none; }}
.chat-header-actions button {{ flex: none; white-space: nowrap; }}

/* ---------------- chat thread (centered conversation column) ---------------- */
.chat-area {{
  flex: 1; display: flex; flex-direction: column; min-height: 0;
  background: {chat_canvas};
}}
#chat-scroll {{
  flex: 1; overflow-y: auto;
  padding: 24px 28px 16px;
  scrollbar-width: thin;
  display: flex; flex-direction: column; gap: 18px;
}}
#chat-scroll > * {{
  flex-shrink: 0; max-width: 800px; width: 100%;
  margin-left: auto; margin-right: auto;
}}
.msg-user {{ display: flex; justify-content: flex-end; }}
.msg-user .bubble {{
  max-width: 78%;
  background: color-mix(in srgb, {text} 6%, transparent);
  border: none;
  border-radius: 14px;
  padding: 8px 14px;
  font-size: 13.5px;
  line-height: 1.55;
  white-space: pre-wrap;
  word-break: break-word;
}}
.msg-assistant {{ display: flex; flex-direction: column; gap: 8px; }}
.msg-assistant .bubble {{
  background: transparent; border: none;
  padding: 2px 0;
  font-size: 14px;
  line-height: 1.65;
  word-break: break-word;
}}
.msg-meta {{ display: flex; align-items: center; gap: 6px; font-size: 10.5px; color: {text_tertiary}; padding: 0 2px; }}
.msg-empty {{
  margin: auto; text-align: center; color: {text_tertiary};
  display: flex; flex-direction: column; gap: 10px;
  padding: 36px 0; align-items: center;
}}
.msg-empty .big {{ font-size: 20px; font-weight: 700; color: {text}; letter-spacing: -0.2px; }}
.starter-grid {{
  display: flex; flex-direction: column; gap: 2px;
  margin-top: 8px; max-width: 400px; width: 100%;
}}
.starter-card {{
  text-align: left; padding: 6px 8px; border-radius: 7px;
  background: transparent; border: none;
  font-size: 12.5px; color: {text_secondary}; cursor: pointer;
  transition: border-color 0.12s ease, background 0.12s ease;
}}
.starter-card:hover {{
  background: transparent; color: {text};
  border: none;
}}

/* markdown content */
.bubble.md p {{ margin: 0 0 8px; }}
.bubble.md p:last-child {{ margin-bottom: 0; }}
.bubble.md h1, .bubble.md h2, .bubble.md h3 {{ margin: 10px 0 6px; }}
.bubble.md ul, .bubble.md ol {{ margin: 4px 0 8px 22px; }}
.bubble.md code {{ background: {bg_hover}; border-radius: 4px; padding: 1px 5px; font-size: 12px; }}
.bubble.md pre {{
  background: {bg_sidebar}; border: {border};
  border-radius: {radius_ctrl};
  padding: 10px 12px; overflow-x: auto; margin: 8px 0;
  font-size: 12px;
}}
.bubble.md pre code {{ background: none; padding: 0; }}
.bubble.md table {{ border-collapse: collapse; margin: 8px 0; }}
.bubble.md th, .bubble.md td {{ border: {border}; padding: 4px 10px; font-size: 12px; }}
.bubble.md blockquote {{ border-left: 3px solid {accent}; margin: 8px 0; padding: 2px 12px; color: {text_secondary}; }}
.bubble.md a {{ color: {accent}; }}

/* collapsible panels */
.collapsible, .reasoning-panel, .tool-card {{ border: none; background: transparent; border-radius: 0; overflow: visible; }}
.collapsible > .collapsible-header, .tool-card .tool-head {{
  height: 24px; min-height: 24px; width: auto;
  display: inline-flex; align-items: center; gap: 6px;
  background: transparent; border: none;
  padding: 0; font-size: 12px; color: {text_tertiary};
  cursor: pointer;
}}
.collapsible > .collapsible-header:hover, .tool-card .tool-head:hover {{ background: transparent; color: {text_secondary}; }}
.collapsible > .collapsible-body, .tool-card .tool-body {{ border-top: none; padding: 4px 0 6px 12px; }}
.reasoning-panel .collapsible-body {{ font-size: 12.5px; color: {text_secondary}; white-space: pre-wrap; max-height: 260px; overflow-y: auto; }}

/* tool groups */
.tool-group {{ display: flex; flex-direction: column; gap: 6px; }}
.tool-card .tool-name {{ font-family: {font_mono}; font-size: 11.5px; font-weight: 600; }}
.tool-card .tool-args {{ color: {text_tertiary}; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: {font_mono}; font-size: 11px; }}
.tool-status {{ font-size: 11px; color: {text_tertiary}; }}
.tool-output {{
  font-family: {font_mono}; font-size: 11.5px;
  background: transparent; border: none;
  padding: 4px 0; max-height: 240px; overflow: auto;
  white-space: pre-wrap; word-break: break-word; color: {text_secondary};
}}
.coder-group {{ border: 1px solid color-mix(in srgb, {accent} 35%, transparent); border-radius: {radius_card}; background: {bg_card}; overflow: hidden; }}
.coder-group > .coder-head {{
  display: flex; align-items: center; gap: 8px;
  padding: 8px 12px; background: color-mix(in srgb, {accent} 10%, transparent);
  cursor: pointer; font-size: 12.5px; font-weight: 600;
}}
.coder-group > .coder-body {{ padding: 10px 12px; display: flex; flex-direction: column; gap: 8px; border-top: {border}; }}

/* diffs */
.diff-view {{
  font-family: {font_mono}; font-size: 11.5px;
  border: {border}; border-radius: {radius_ctrl};
  overflow: hidden; max-height: 300px; overflow-y: auto;
}}
.diff-line {{ display: flex; padding: 1px 8px; white-space: pre-wrap; word-break: break-word; }}
.diff-line.add {{ background: color-mix(in srgb, #2ea043 18%, transparent); color: #7ee2a8; }}
.diff-line.del {{ background: color-mix(in srgb, #f85149 14%, transparent); color: #ffa198; }}
.diff-line.hunk {{ color: {text_tertiary}; background: {bg_hover}; }}
.diff-line.ctx {{ color: {text_secondary}; }}
.diff-line.plain {{ color: {text_secondary}; }}

/* ---------------- permission & question cards ---------------- */
.permission-card, .question-card {{
  flex: none; max-width: 800px; width: calc(100% - 40px); margin: 0 auto 12px;
  border: 1px solid color-mix(in srgb, {accent} 45%, transparent);
  border-radius: {radius_card};
  background: {bg_card};
  box-shadow: {shadow};
  padding: 14px 16px;
  display: flex; flex-direction: column; gap: 10px;
}}
.permission-card .pc-title, .question-card .qc-title {{
  display: flex; align-items: center; gap: 8px; font-weight: 700; font-size: 13.5px;
}}
.permission-card .pc-detail, .question-card .qc-question {{ color: {text_secondary}; font-size: 13px; }}
.permission-card .pc-pathbox {{
  font-family: {font_mono}; font-size: 12px;
  background: {bg_sidebar}; border: {border};
  border-radius: {radius_ctrl}; padding: 8px 10px;
  word-break: break-all; max-height: 130px; overflow: auto; white-space: pre-wrap;
}}
.pc-buttons {{ display: flex; gap: 8px; flex-wrap: wrap; }}
.qc-options {{ display: flex; flex-direction: column; gap: 6px; }}
.qc-option {{
  text-align: left; padding: 8px 12px; border-radius: {radius_ctrl};
}}
.qc-option.selected {{ border-color: {accent}; background: color-mix(in srgb, {accent} 12%, transparent); }}
.qc-note input {{ width: 100%; }}

/* ---------------- floating composer card ---------------- */
.composer {{
  position: relative; flex: none; max-width: 800px; width: calc(100% - 40px);
  margin: 0 auto 18px; padding: 12px 14px 10px;
  background: color-mix(in srgb, {text} 3.5%, transparent);
  border: 1px solid color-mix(in srgb, {text} 7%, transparent);
  border-radius: 16px; box-shadow: none;
  display: flex; flex-direction: column; gap: 8px;
}}
.composer:focus-within {{
  border: 1px solid color-mix(in srgb, {text} 12%, transparent);
  background: color-mix(in srgb, {text} 4.5%, transparent);
}}
.composer textarea {{
  flex: 1; background: transparent; border: none; box-shadow: none;
  padding: 4px 2px; min-height: 44px; max-height: 180px;
  font-size: 14px; line-height: 1.5; resize: none; width: 100%;
}}
.composer textarea:focus-visible {{ box-shadow: none; }}
.composer-row-bottom {{
  position: relative; display: flex; gap: 8px; align-items: center;
  min-height: 30px; flex-wrap: nowrap;
}}
.composer-row-top {{
  display: flex; gap: 8px; align-items: center;
}}
.composer-row-top input {{
  flex: 1; min-width: 0;
}}
.composer-row-top > button {{
  flex: none; white-space: nowrap;
}}
.composer-plus-btn {{
  width: 26px; height: 26px; padding: 0; border-radius: 6px;
  display: inline-flex; align-items: center; justify-content: center;
  color: {text_secondary}; background: transparent; flex: none;
}}
.composer-plus-btn:hover {{
  color: {text}; background: color-mix(in srgb, {text} 6%, transparent);
}}
.popover-backdrop {{
  position: fixed; inset: 0; z-index: 45;
  background: transparent;
}}
.composer-context-menu {{
  position: absolute; bottom: calc(100% + 10px); left: 0; z-index: 60;
  width: 210px; padding: 6px;
  background: {bg_card}; border: {border}; border-radius: 12px; box-shadow: {shadow};
  display: flex; flex-direction: column; gap: 2px;
}}
.ccm-header {{
  font-size: 11.5px; font-weight: 500; color: {text_tertiary};
  padding: 4px 10px 6px;
  display: flex; align-items: center; justify-content: space-between;
}}
.ccm-header button {{ padding: 0; font-size: 11.5px; color: {text_secondary}; }}
.ccm-search {{
  width: 100%; height: 28px; padding: 0 10px; margin-bottom: 4px;
  font-size: 12.5px; color: {text};
  background: color-mix(in srgb, {text} 5%, transparent); border-radius: 6px;
}}
.ccm-list {{
  display: flex; flex-direction: column; gap: 2px;
  max-height: 200px; overflow-y: auto; scrollbar-width: thin;
}}
.ccm-empty {{ padding: 8px 10px; font-size: 12px; color: {text_tertiary}; }}
.ccm-item {{
  width: 100%; min-height: 30px; padding: 0 10px; border-radius: 7px;
  display: flex; align-items: center; gap: 10px;
  font-size: 13px; font-weight: 400; color: {text_secondary};
  background: transparent; border: none; text-align: left;
  overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}}
.ccm-item:hover {{ background: {bg_hover}; color: {text}; }}
.composer-dropdown {{
  position: relative; display: inline-flex; align-items: center;
}}
.composer-dropdown.open {{
  z-index: 55;
}}
.composer-select-btn {{
  height: 26px; padding: 0 7px; border-radius: 6px;
  display: inline-flex; align-items: center; gap: 5px;
  font-size: 12px; font-weight: 500; color: {text_secondary};
  background: transparent; border: none; white-space: nowrap;
  max-width: 220px; overflow: hidden; text-overflow: ellipsis;
}}
.composer-select-btn:hover, .composer-select-btn.open {{
  color: {text}; background: color-mix(in srgb, {text} 6%, transparent);
}}
.composer-context-menu.align-right {{
  left: auto; right: 0;
}}
.ccm-item.active {{
  color: {text}; font-weight: 600;
  background: color-mix(in srgb, {text} 6%, transparent);
}}
.ccm-item-label {{
  flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}}
.ccm-divider {{
  height: 1px; margin: 4px 6px;
  background: color-mix(in srgb, {text} 8%, transparent);
}}
.ccm-submenu-row {{
  position: relative;
}}
.composer-context-menu.model-menu {{
  width: 280px;
}}
.ccm-list.model-list {{
  max-height: min(320px, calc(100vh - 180px));
  overflow-y: auto;
  overflow-x: hidden;
  scrollbar-width: thin;
}}
.ccm-list::-webkit-scrollbar {{
  width: 5px;
}}
.ccm-list::-webkit-scrollbar-thumb {{
  background: color-mix(in srgb, {text} 18%, transparent);
  border-radius: 999px;
}}
.ccm-model-name {{
  color: {text}; font-weight: 500;
}}
.ccm-model-effort {{
  margin-left: 8px; font-size: 12px; font-weight: 400; color: {text_tertiary};
}}
.ccm-row-chevron {{
  display: inline-flex; align-items: center; color: {text_tertiary}; opacity: 0.75;
}}
.composer-flyout-right {{
  position: fixed; z-index: 65;
  padding-left: 6px;
}}
.composer-flyout-card {{
  width: 150px; padding: 6px;
  background: {bg_card}; border: {border}; border-radius: 12px; box-shadow: {shadow};
  display: flex; flex-direction: column; gap: 2px;
}}
.composer-send-btn {{
  width: 30px; height: 30px; min-width: 30px; min-height: 30px; padding: 0;
  border-radius: 50%; background: {accent}; color: {accent_text}; border: none;
  display: inline-flex; align-items: center; justify-content: center; flex: none;
}}
.composer-send-btn:hover:not(:disabled) {{ background: {accent}; filter: brightness(1.08); }}
.composer-send-btn:disabled {{ opacity: 0.35; cursor: default; }}
.composer-spacer {{ flex: 1; }}
.composer-hint {{ font-size: 11px; color: {text_tertiary}; }}

/* ---------------- modals ---------------- */
.modal-backdrop {{
  position: fixed; inset: 0; z-index: 60;
  background: rgba(0, 0, 0, 0.45);
  display: flex; align-items: center; justify-content: center;
}}
.modal {{
  width: 440px; max-width: calc(100vw - 40px);
  background: {bg_card}; border: {border};
  border-radius: {radius_card}; box-shadow: {shadow};
  padding: 18px; display: flex; flex-direction: column; gap: 12px;
}}
.modal h3 {{ font-size: 15px; }}
.modal input {{ width: 100%; }}
.modal-actions {{ display: flex; justify-content: flex-end; gap: 8px; }}

/* ---------------- right panel ---------------- */
.right-panel-tabs {{
  height: 44px; display: flex; align-items: stretch; border-bottom: none; flex: none;
}}
.right-panel-tabs button {{
  flex: 1; border: none; border-radius: 0;
  background: transparent; padding: 10px 4px;
  font-size: 12px; color: {text_secondary};
  border-bottom: 2px solid transparent;
}}
.right-panel-tabs button:hover {{ background: transparent; color: {text}; }}
.right-panel-tabs button.active {{ color: {accent}; border-bottom-color: {accent}; font-weight: 600; }}
.right-panel-body {{ flex: 1; overflow-y: auto; padding: 14px; display: flex; flex-direction: column; gap: 10px; scrollbar-width: thin; }}
.panel-card {{ background: transparent; border: none; padding: 4px 0; font-size: 12.5px; display: flex; flex-direction: column; gap: 8px; min-width: 0; overflow: hidden; }}
.panel-card .mono {{ font-size: 11.5px; color: {text_secondary}; }}
.panel-section-title {{ font-size: 11px; font-weight: 700; text-transform: uppercase; letter-spacing: 0.6px; color: {text_tertiary}; display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }}
.plan-task {{ display: flex; gap: 8px; align-items: flex-start; padding: 3px 0; }}
.plan-task .task-status {{ font-size: 11px; color: {text_tertiary}; flex: none; width: 78px; }}
.plan-task .task-text {{ flex: 1; }}
.task-completed .task-text {{ text-decoration: line-through; color: {text_tertiary}; }}
.task-in_progress .task-text {{ color: {accent}; }}

/* ---------------- settings & usage ---------------- */
.page-header {{
  height: 44px; min-height: 44px; max-height: 44px; padding: 0 18px;
  border-bottom: none;
  background: transparent;
  display: flex; align-items: center; gap: 10px;
  flex: none;
}}
.page-header h2 {{ font-size: 16px; flex: 1; }}
.settings-layout {{ flex: 1; display: flex; min-height: 0; }}
.settings-nav {{
  width: 180px; border-right: none; padding: 12px 8px;
  display: flex; flex-direction: column; gap: 2px; flex: none;
}}
.settings-nav button {{
  height: 28px; min-height: 28px; max-height: 28px;
  padding: 0 10px;
  display: flex; align-items: center; width: 100%; text-align: left;
  background: transparent; border: none; font-size: 12.5px; color: {text_secondary};
  white-space: nowrap;
}}
.settings-nav button:hover {{ background: transparent; color: {text}; }}
.settings-nav button.active {{ background: transparent; color: {accent}; font-weight: 600; }}
.settings-content {{ flex: 1; overflow-y: auto; padding: 18px 24px; display: flex; flex-direction: column; gap: 12px; scrollbar-width: thin; }}
.usage-layout {{ flex: 1; display: flex; flex-direction: column; min-height: 0; padding: 18px 24px; gap: 14px; overflow-y: auto; }}
.usage-metrics {{ display: flex; gap: 12px; flex-wrap: wrap; }}
.metric-card {{
  flex: 1; min-width: 150px;
  background: transparent; border: none;
  padding: 6px 4px;
  display: flex; flex-direction: column; gap: 4px;
}}
.metric-card .metric-value {{ font-size: 20px; font-weight: 700; }}
.metric-card .metric-label {{ font-size: 11px; color: {text_tertiary}; text-transform: uppercase; letter-spacing: 0.5px; }}
table.data-table {{ width: 100%; border-collapse: collapse; font-size: 12.5px; }}
table.data-table th, table.data-table td {{
  text-align: left; padding: 8px 10px; border-bottom: {border};
  white-space: nowrap; max-width: 260px; overflow: hidden; text-overflow: ellipsis;
}}
table.data-table th {{ color: {text_tertiary}; font-size: 11px; text-transform: uppercase; letter-spacing: 0.5px; position: sticky; top: 0; background: {bg_app}; }}
table.data-table tr:hover td {{ background: {bg_hover}; }}
.settings-row {{ display: flex; align-items: center; gap: 10px; padding: 6px 8px; border: none; border-radius: 7px; background: transparent; min-width: 0; }}
.settings-row:hover {{ background: transparent; }}
.settings-row > button {{ flex: none; white-space: nowrap; }}
.settings-row .grow {{ flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; }}
.settings-empty {{ color: {text_tertiary}; text-align: center; padding: 30px 0; }}

/* ---------------- toast ---------------- */
.toast {{
  position: fixed; bottom: 20px; left: 50%; transform: translateX(-50%);
  background: {bg_card}; border: {border};
  border-radius: {radius_ctrl}; box-shadow: {shadow};
  padding: 10px 18px; font-size: 13px; z-index: 100;
  max-width: 70vw; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}}
"#,
        font_ui = font_ui,
        font_mono = font_mono,
        accent = accent,
        accent_text = accent_text,
        bg_app = bg_app,
        bg_sidebar = bg_sidebar,
        bg_card = bg_card,
        bg_hover = bg_hover,
        border = border,
        radius_ctrl = radius_ctrl,
        radius_card = radius_card,
        sidebar_brand_padding = sidebar_brand_padding,
        text = text,
        text_secondary = text_secondary,
        text_tertiary = text_tertiary,
        shadow = shadow,
        chat_canvas = chat_canvas,
    )
}

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
    fn windows_uses_fluent_design_fonts_and_accent() {
        let dark = build_stylesheet_for("windows", true);
        let light = build_stylesheet_for("windows", false);
        for style in [&dark, &light] {
            assert!(style.contains("Segoe UI"), "windows font missing");
            assert!(style.contains("Cascadia Code"), "windows mono font missing");
        }
        assert!(dark.contains("#60cdff"), "windows dark accent missing");
        assert!(light.contains("#0067c0"), "windows light accent missing");
        assert!(dark.contains("background: #191919"), "windows dark bg missing");
        assert!(light.contains("background: #f3f3f3"), "windows light bg missing");
    }

    #[test]
    fn macos_uses_native_fonts_and_accent() {
        let dark = build_stylesheet_for("macos", true);
        let light = build_stylesheet_for("macos", false);
        for style in [&dark, &light] {
            assert!(style.contains("-apple-system"), "macos font missing");
            assert!(style.contains("SF Mono"), "macos mono font missing");
        }
        assert!(dark.contains("#0a84ff"), "macos dark accent missing");
        assert!(light.contains("#007aff"), "macos light accent missing");
        assert!(dark.contains("background: #1c1c1e"), "macos dark bg missing");
        assert!(light.contains("background: #f5f5f7"), "macos light bg missing");
    }

    #[test]
    fn linux_falls_back_to_neutral_style() {
        let dark = build_stylesheet_for("linux", true);
        let light = build_stylesheet_for("linux", false);
        for style in [&dark, &light] {
            assert!(style.contains("Ubuntu"), "linux font missing");
        }
        assert!(dark.contains("#4493f8"), "linux dark accent missing");
        assert!(light.contains("#1a73e8"), "linux light accent missing");
    }

    #[test]
    fn styles_cover_the_core_layout_sections() {
        for platform in PLATFORMS {
            for dark_mode in [true, false] {
                let style = build_stylesheet_for(platform, dark_mode);
                for section in [
                    ".app-shell", ".sidebar", ".chat-area", ".composer", ".right-panel",
                    ".settings-layout", ".permission-card", ".diff-view", ".toast",
                ] {
                    assert!(style.contains(section), "{platform} missing {section}");
                }
            }
        }
    }

    #[test]
    fn detected_os_build_matches_explicit_platform() {
        assert_eq!(
            build_stylesheet(true),
            build_stylesheet_for(std::env::consts::OS, true)
        );
        assert_eq!(
            build_stylesheet(false),
            build_stylesheet_for(std::env::consts::OS, false)
        );
    }
}
