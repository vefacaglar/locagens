//! App-wide keyboard layer (UX-019). One document-level listener maps keys
//! to actions and sends them to Rust; `Ctrl` reads as `Cmd` on macOS.
//!
//! | Key | Action |
//! |-----|--------|
//! | `Esc` | close the open popover (the a11y script), else leave Settings / Usage, else stop the run |
//! | `Ctrl+N` | new chat in the current project |
//! | `Ctrl+K` | command palette |
//! | `Ctrl+L` | focus the composer |
//! | `Ctrl+M` (`Cmd+Shift+M`) | model picker |
//! | `Ctrl+B` / `Ctrl+Alt+B` | toggle sidebar / inspector |
//! | `Ctrl+.` | cycle the mode (Build ⇄ Plan) |
//! | `Ctrl+,` | settings |
//! | `Ctrl+/` | shortcuts and commands |

use crate::state_ui::{next_mode, run_is_active, AppPage, Overlay, SettingsTab, UiStore};
use dioxus::prelude::*;

/// The listener. `Esc` is ignored when something already handled it (a
/// popover closed, a card answered) or while a modal / popover is open.
pub const KEY_SCRIPT: &str = r#"
const mac = navigator.platform.toUpperCase().startsWith('MAC');
document.addEventListener('keydown', (e) => {
  const mod = mac ? e.metaKey : e.ctrlKey;
  const key = (e.key || '').toLowerCase();
  let action = null;
  if (e.key === 'Escape') {
    if (e.defaultPrevented) return;
    if (document.querySelector('.modal-backdrop, .popover-backdrop')) return;
    if (e.target && e.target.closest && e.target.closest('.permission-card, .question-card')) return;
    action = 'escape';
  } else if (mod && e.altKey && key === 'b') {
    action = 'inspector';
  } else if (mod && e.shiftKey && key === 'm' && mac) {
    action = 'model';
  } else if (mod && !e.altKey && !e.shiftKey) {
    action = ({ n: 'new', k: 'palette', l: 'focus', b: 'sidebar', '.': 'mode', ',': 'settings', '/': 'help', m: mac ? null : 'model' })[key] || null;
  }
  if (action) { e.preventDefault(); dioxus.send(action); }
});
"#;

/// Installs the listener and runs its actions; call once from `App`.
pub fn use_keyboard_layer(store: UiStore) {
    use_future(move || async move {
        let mut keys = dioxus::document::eval(KEY_SCRIPT);
        while let Ok(action) = keys.recv::<String>().await {
            run_action(store, &action);
        }
    });
}

fn run_action(store: UiStore, action: &str) {
    let mut store = store;
    match action {
        "escape" => {
            if *store.overlay.peek() != Overlay::None {
                store.overlay.set(Overlay::None);
            } else if *store.page.peek() != AppPage::Chat {
                store.page.set(AppPage::Chat);
            } else if let Some(run) = store.active_run.peek().clone().filter(|run| run_is_active(&run.status)) {
                spawn(async move {
                    let _ = locagens_core::api::runs::cancel_run(&crate::state_ui::core(), &run.id).await;
                });
            }
        }
        "new" => {
            super::sidebar::new_chat(&store);
            crate::state_ui::focus_composer();
        }
        "palette" => {
            let next = if *store.overlay.peek() == Overlay::None { Overlay::Palette { chats_only: false } } else { Overlay::None };
            store.overlay.set(next);
        }
        "focus" => {
            store.page.set(AppPage::Chat);
            crate::state_ui::focus_composer();
        }
        "model" => {
            store.page.set(AppPage::Chat);
            store.model_picker_open.set(true);
        }
        "sidebar" => {
            let open = *store.sidebar_open.peek();
            store.sidebar_open.set(!open);
        }
        "inspector" => {
            let open = *store.right_panel_open.peek();
            store.right_panel_open.set(!open);
        }
        "mode" => {
            let mode = next_mode(&store.selected_mode.peek());
            store.pick_mode(mode);
        }
        "settings" => store.page.set(AppPage::Settings(SettingsTab::Providers)),
        "help" => {
            let next = if *store.overlay.peek() == Overlay::Shortcuts { Overlay::None } else { Overlay::Shortcuts };
            store.overlay.set(next);
        }
        _ => {}
    }
}
