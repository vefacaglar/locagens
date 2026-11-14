//! Clipboard writes (FR-060).
//!
//! On Windows the page origin (`http://dioxus.index.html/`) is not a secure
//! context, so `navigator.clipboard` is undefined there: the text is written
//! from Rust through the Win32 clipboard. Elsewhere the webview's clipboard
//! API is used, with the `execCommand('copy')` fallback. Callers show the
//! check mark only when this returns `true`.

/// Copies `text`; `true` when the clipboard now holds it.
pub async fn copy_text(text: &str) -> bool {
    #[cfg(windows)]
    {
        clipboard_win::set_clipboard_string(text).is_ok()
    }
    #[cfg(not(windows))]
    {
        copy_in_webview(text).await
    }
}

#[cfg(not(windows))]
async fn copy_in_webview(text: &str) -> bool {
    // The text travels as a JSON string literal, never concatenated raw.
    let literal = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
    let script = format!(
        r#"const text = {literal};
           let ok = false;
           try {{ await navigator.clipboard.writeText(text); ok = true; }} catch (e) {{
             const area = document.createElement('textarea');
             area.value = text; area.style.position = 'fixed'; area.style.opacity = '0';
             document.body.appendChild(area); area.select();
             try {{ ok = document.execCommand('copy'); }} catch (e2) {{ ok = false; }}
             area.remove();
           }}
           dioxus.send(ok);"#
    );
    let mut eval = dioxus::document::eval(&script);
    eval.recv::<bool>().await.unwrap_or(false)
}

/// Serves the copy buttons inside rendered Markdown (`locagensCopy(button)`
/// in [`crate::markdown::COPY_SCRIPT`]): each click sends the code here, the
/// answer flips that button to a check mark only when the copy worked.
pub async fn serve_code_copy_buttons() {
    let mut channel = dioxus::document::eval(crate::markdown::COPY_SCRIPT);
    while let Ok(code) = channel.recv::<String>().await {
        let ok = copy_text(&code).await;
        let _ = channel.send(ok);
    }
}
