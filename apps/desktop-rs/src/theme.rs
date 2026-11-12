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
    let (font_ui, font_mono, accent, accent_text, bg_app, bg_sidebar, bg_card, bg_hover, border, radius_ctrl, radius_card) = if cfg!(target_os = "macos") {
        // macOS: vibrancy-inspired dark surfaces and system blue.
        (
            "-apple-system, BlinkMacSystemFont, \"SF Pro Text\", \"SF Pro Display\", system-ui, sans-serif",
            "\"SF Mono\", \"Menlo\", monospace",
            if dark_mode { "#0a84ff" } else { "#007aff" },
            if dark_mode { "#ffffff" } else { "#ffffff" },
            if dark_mode { "#1c1c1e" } else { "#f5f5f7" },
            if dark_mode { "#242426" } else { "#ffffff" },
            if dark_mode { "#2c2c2e" } else { "#fbfbfd" },
            if dark_mode { "#323234" } else { "#e8e8ed" },
            if dark_mode { "1px solid rgba(255, 255, 255, 0.1)" } else { "1px solid rgba(0, 0, 0, 0.08)" },
            "8px",
            "10px",
        )
    } else if cfg!(target_os = "windows") {
        // Windows: Fluent Design / Mica.
        (
            "\"Segoe UI Variable Text\", \"Segoe UI\", system-ui, sans-serif",
            "\"Cascadia Code\", \"Consolas\", monospace",
            if dark_mode { "#60cdff" } else { "#0067c0" },
            if dark_mode { "#001a33" } else { "#ffffff" },
            if dark_mode { "#191919" } else { "#f3f3f3" },
            if dark_mode { "#202020" } else { "#fbfbfb" },
            if dark_mode { "#272727" } else { "#ffffff" },
            if dark_mode { "#2f2f2f" } else { "#eaeaea" },
            if dark_mode { "1px solid rgba(255, 255, 255, 0.08)" } else { "1px solid rgba(0, 0, 0, 0.08)" },
            "6px",
            "8px",
        )
    } else {
        // Linux / other: neutral dark-first style.
        (
            "system-ui, \"Ubuntu\", \"Cantarell\", sans-serif",
            "\"JetBrains Mono\", \"DejaVu Sans Mono\", monospace",
            if dark_mode { "#4493f8" } else { "#1a73e8" },
            "#ffffff",
            if dark_mode { "#191919" } else { "#f3f3f3" },
            if dark_mode { "#202020" } else { "#fbfbfb" },
            if dark_mode { "#272727" } else { "#ffffff" },
            if dark_mode { "#2f2f2f" } else { "#eaeaea" },
            if dark_mode { "1px solid rgba(255, 255, 255, 0.08)" } else { "1px solid rgba(0, 0, 0, 0.08)" },
            "6px",
            "8px",
        )
    };

    let (text, text_secondary, text_tertiary, shadow) = if dark_mode {
        ("#f3f3f3", "#c8c8c8", "#8a8a8a", "0 4px 14px rgba(0, 0, 0, 0.35)")
    } else {
        ("#1a1a1a", "#4a4a4a", "#777777", "0 4px 14px rgba(0, 0, 0, 0.08)")
    };

    let focus_ring = if cfg!(target_os = "windows") {
        format!("0 0 0 2px {bg_app}, 0 0 0 4px {accent}")
    } else {
        format!("0 0 0 3px color-mix(in srgb, {accent} 40%, transparent)")
    };

    format!(r#"
* {{ margin: 0; padding: 0; box-sizing: border-box; }}
html, body {{ height: 100%; overflow: hidden; }}
body {{
  font-family: {font_ui};
  font-size: 13.5px;
  color: {text};
  background: {bg_app};
  -webkit-font-smoothing: antialiased;
  user-select: text;
  cursor: default;
}}
code, pre, .mono {{ font-family: {font_mono}; }}

/* ---------------- shell layout ---------------- */
.app-shell {{
  display: flex; height: 100vh; width: 100vw;
  background: {bg_app}; color: {text};
  overflow: hidden;
}}
.sidebar {{
  width: 280px; min-width: 280px; height: 100%;
  background: {bg_sidebar}; border-right: {border};
  display: flex; flex-direction: column;
}}
.main-column {{
  flex: 1 1 auto; min-width: 0; height: 100%;
  display: flex; flex-direction: column;
  background: {bg_app};
}}
.right-panel {{
  width: 340px; min-width: 340px; height: 100%;
  background: {bg_sidebar}; border-left: {border};
  display: flex; flex-direction: column;
  overflow: hidden;
}}

/* ---------------- generic controls ---------------- */
button, .btn {{
  font: inherit; color: {text};
  background: {bg_card};
  border: {border};
  border-radius: {radius_ctrl};
  padding: 6px 12px;
  cursor: pointer;
  transition: background 0.12s ease, opacity 0.12s ease;
}}
button:hover {{ background: {bg_hover}; }}
button:active {{ opacity: 0.8; }}
button:focus-visible, input:focus-visible, textarea:focus-visible, select:focus-visible {{
  outline: none; box-shadow: {focus_ring};
}}
button.primary {{
  background: {accent}; color: {accent_text};
  border: 1px solid transparent; font-weight: 600;
}}
button.primary:hover {{ filter: brightness(1.08); }}
button.subtle {{ background: transparent; border-color: transparent; color: {text_secondary}; }}
button.subtle:hover {{ background: {bg_hover}; color: {text}; }}
button.danger {{ color: #ff8a8a; }}
button.icon-btn {{ padding: 4px 8px; line-height: 1; }}
button:disabled {{ opacity: 0.45; cursor: default; }}

input, textarea, select {{
  font: inherit; color: {text};
  background: {bg_card};
  border: {border};
  border-radius: {radius_ctrl};
  padding: 6px 10px;
  outline: none;
}}
input::placeholder, textarea::placeholder {{ color: {text_tertiary}; }}
select {{ padding: 5px 8px; cursor: pointer; }}
label {{ color: {text_secondary}; font-size: 12px; }}

/* ---------------- badges & pills ---------------- */
.badge {{
  display: inline-flex; align-items: center; gap: 4px;
  padding: 2px 8px; border-radius: 999px;
  font-size: 11px; font-weight: 600;
  background: {bg_hover}; color: {text_secondary};
  border: {border};
  white-space: nowrap;
}}
.badge.accent {{ background: color-mix(in srgb, {accent} 18%, transparent); color: {accent}; border-color: transparent; }}
.badge.ok {{ color: #4ecf7a; }}
.badge.warn {{ color: #e6b455; }}
.badge.err {{ color: #ff8a8a; }}
.badge.run {{ color: {accent}; }}
.pill {{
  display: inline-flex; align-items: center; gap: 6px;
  padding: 3px 10px; border-radius: 999px;
  background: {bg_hover}; border: {border};
  font-size: 11.5px; color: {text_secondary};
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
  padding: 14px 14px 10px;
  display: flex; flex-direction: column; gap: 10px;
  border-bottom: {border};
}}
.brand-row {{ display: flex; align-items: center; gap: 8px; }}
.brand-title {{ font-size: 17px; font-weight: 700; letter-spacing: 0.2px; flex: 1; }}
.os-badge {{ font-size: 10.5px; padding: 2px 8px; }}
.sidebar-actions {{ display: flex; gap: 8px; }}
.sidebar-actions button {{ flex: 1; }}
.sidebar-search {{ padding: 10px 12px 6px; }}
.sidebar-search input {{ width: 100%; }}
.sidebar-tree {{
  flex: 1; overflow-y: auto; padding: 4px 8px 12px;
  scrollbar-width: thin;
}}
.project-group {{ margin-top: 8px; }}
.project-row {{
  display: flex; align-items: center; gap: 6px;
  padding: 5px 6px; border-radius: {radius_ctrl};
  color: {text_secondary}; font-weight: 600; font-size: 12.5px;
  cursor: pointer;
}}
.project-row:hover {{ background: {bg_hover}; color: {text}; }}
.project-row .project-name {{
  flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}}
.run-item {{
  display: flex; align-items: center; gap: 8px;
  padding: 6px 6px 6px 16px; border-radius: {radius_ctrl};
  cursor: pointer; font-size: 12.5px; color: {text_secondary};
}}
.run-item:hover {{ background: {bg_hover}; color: {text}; }}
.run-item.active {{ background: color-mix(in srgb, {accent} 14%, transparent); color: {text}; }}
.run-item .run-title {{ flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }}
.run-item .run-model {{ font-size: 10px; color: {text_tertiary}; }}
.run-item .run-delete {{ visibility: hidden; }}
.run-item:hover .run-delete {{ visibility: visible; }}
.sidebar-footer {{
  border-top: {border}; padding: 8px 12px;
  display: flex; gap: 8px;
}}
.sidebar-footer button {{ flex: 1; font-size: 12.5px; }}
.sidebar-footer button.active {{ background: {bg_hover}; color: {accent}; font-weight: 600; }}

/* ---------------- chat header ---------------- */
.chat-header {{
  padding: 12px 18px;
  border-bottom: {border};
  display: flex; align-items: center; gap: 10px;
  background: {bg_sidebar};
  flex: none;
}}
.chat-title {{ font-size: 14.5px; font-weight: 600; flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }}
.chat-header-meta {{ display: flex; align-items: center; gap: 8px; }}

/* ---------------- chat thread ---------------- */
.chat-area {{ flex: 1; display: flex; flex-direction: column; min-height: 0; }}
#chat-scroll {{
  flex: 1; overflow-y: auto;
  padding: 18px 22px 12px;
  scrollbar-width: thin;
  display: flex; flex-direction: column; gap: 14px;
}}
.msg-user {{ align-self: flex-end; max-width: 78%; }}
.msg-user .bubble {{
  background: color-mix(in srgb, {accent} 16%, transparent);
  border: 1px solid color-mix(in srgb, {accent} 35%, transparent);
  border-radius: {radius_card};
  padding: 10px 14px;
  white-space: pre-wrap;
  word-break: break-word;
}}
.msg-assistant {{ align-self: stretch; max-width: 92%; display: flex; flex-direction: column; gap: 8px; }}
.msg-assistant .bubble {{
  background: {bg_card}; border: {border};
  border-radius: {radius_card};
  padding: 10px 14px;
  word-break: break-word;
}}
.msg-meta {{ display: flex; align-items: center; gap: 6px; font-size: 10.5px; color: {text_tertiary}; padding: 0 2px; }}
.msg-empty {{ margin: auto; text-align: center; color: {text_tertiary}; display: flex; flex-direction: column; gap: 8px; padding: 40px 0; }}
.msg-empty .big {{ font-size: 15px; font-weight: 600; color: {text_secondary}; }}

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
.collapsible {{ border: {border}; border-radius: {radius_ctrl}; background: {bg_sidebar}; overflow: hidden; }}
.collapsible > .collapsible-header {{
  display: flex; align-items: center; gap: 8px;
  width: 100%; text-align: left;
  background: transparent; border: none;
  padding: 7px 12px; font-size: 12px; font-weight: 600; color: {text_secondary};
  cursor: pointer;
}}
.collapsible > .collapsible-header:hover {{ background: {bg_hover}; color: {text}; }}
.collapsible > .collapsible-body {{ padding: 8px 12px 10px; border-top: {border}; }}
.reasoning-panel .collapsible-body {{ font-size: 12.5px; color: {text_secondary}; white-space: pre-wrap; max-height: 260px; overflow-y: auto; }}

/* tool groups */
.tool-group {{ display: flex; flex-direction: column; gap: 6px; }}
.tool-card {{ border: {border}; border-radius: {radius_ctrl}; background: {bg_sidebar}; }}
.tool-card .tool-head {{
  display: flex; align-items: center; gap: 8px;
  padding: 6px 10px; cursor: pointer; font-size: 12px;
}}
.tool-card .tool-head:hover {{ background: {bg_hover}; }}
.tool-card .tool-name {{ font-family: {font_mono}; font-size: 11.5px; font-weight: 600; }}
.tool-card .tool-args {{ flex: 1; color: {text_tertiary}; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: {font_mono}; font-size: 11px; }}
.tool-card .tool-body {{ padding: 6px 10px 10px; border-top: {border}; }}
.tool-output {{
  font-family: {font_mono}; font-size: 11.5px;
  background: {bg_app}; border: {border};
  border-radius: {radius_ctrl};
  padding: 8px 10px; max-height: 240px; overflow: auto;
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
  flex: none; margin: 0 22px 10px;
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

/* ---------------- composer ---------------- */
.composer {{
  flex: none; padding: 10px 22px 16px;
  border-top: {border};
  background: {bg_sidebar};
  display: flex; flex-direction: column; gap: 8px;
}}
.composer-row-top {{ display: flex; gap: 8px; align-items: flex-end; }}
.composer textarea {{
  flex: 1; resize: none; min-height: 46px; max-height: 180px;
  padding: 10px 12px; border-radius: {radius_ctrl};
  line-height: 1.45;
}}
.composer-quickbar {{ display: flex; gap: 6px; flex-wrap: wrap; align-items: center; }}
.composer-quickbar .chip {{
  font-size: 11px; padding: 3px 9px; border-radius: 999px;
  background: {bg_hover}; border: {border}; color: {text_secondary}; cursor: pointer;
}}
.composer-quickbar .chip:hover {{ color: {accent}; border-color: {accent}; }}
.composer-files {{ display: flex; gap: 6px; flex-wrap: wrap; }}
.composer-row-bottom {{ display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }}
.composer-row-bottom select {{ font-size: 12px; }}
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
  display: flex; border-bottom: {border}; flex: none;
}}
.right-panel-tabs button {{
  flex: 1; border: none; border-radius: 0;
  background: transparent; padding: 10px 4px;
  font-size: 12px; color: {text_secondary};
  border-bottom: 2px solid transparent;
}}
.right-panel-tabs button:hover {{ background: {bg_hover}; }}
.right-panel-tabs button.active {{ color: {accent}; border-bottom-color: {accent}; font-weight: 600; }}
.right-panel-body {{ flex: 1; overflow-y: auto; padding: 14px; display: flex; flex-direction: column; gap: 10px; scrollbar-width: thin; }}
.panel-card {{ background: {bg_card}; border: {border}; border-radius: {radius_ctrl}; padding: 10px 12px; font-size: 12.5px; }}
.panel-card .mono {{ font-size: 11.5px; color: {text_secondary}; }}
.panel-section-title {{ font-size: 11px; font-weight: 700; text-transform: uppercase; letter-spacing: 0.6px; color: {text_tertiary}; }}
.plan-task {{ display: flex; gap: 8px; align-items: flex-start; padding: 3px 0; }}
.plan-task .task-status {{ font-size: 11px; color: {text_tertiary}; flex: none; width: 78px; }}
.plan-task .task-text {{ flex: 1; }}
.task-completed .task-text {{ text-decoration: line-through; color: {text_tertiary}; }}
.task-in_progress .task-text {{ color: {accent}; }}

/* ---------------- settings & usage ---------------- */
.page-header {{
  padding: 14px 22px; border-bottom: {border};
  background: {bg_sidebar};
  display: flex; align-items: center; gap: 10px;
}}
.page-header h2 {{ font-size: 16px; flex: 1; }}
.settings-layout {{ flex: 1; display: flex; min-height: 0; }}
.settings-nav {{
  width: 180px; border-right: {border}; padding: 12px 8px;
  display: flex; flex-direction: column; gap: 2px; flex: none;
}}
.settings-nav button {{
  text-align: left; background: transparent; border: none;
  padding: 8px 12px; border-radius: {radius_ctrl}; font-size: 12.5px; color: {text_secondary};
}}
.settings-nav button:hover {{ background: {bg_hover}; color: {text}; }}
.settings-nav button.active {{ background: {bg_hover}; color: {accent}; font-weight: 600; }}
.settings-content {{ flex: 1; overflow-y: auto; padding: 18px 24px; display: flex; flex-direction: column; gap: 12px; scrollbar-width: thin; }}
.usage-layout {{ flex: 1; display: flex; flex-direction: column; min-height: 0; padding: 18px 24px; gap: 14px; overflow-y: auto; }}
.usage-metrics {{ display: flex; gap: 12px; flex-wrap: wrap; }}
.metric-card {{
  flex: 1; min-width: 150px;
  background: {bg_card}; border: {border};
  border-radius: {radius_card}; padding: 12px 16px;
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
.settings-row {{ display: flex; align-items: center; gap: 10px; padding: 8px 10px; border: {border}; border-radius: {radius_ctrl}; background: {bg_card}; }}
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
        text = text,
        text_secondary = text_secondary,
        text_tertiary = text_tertiary,
        shadow = shadow,
        focus_ring = focus_ring,
    )
}
