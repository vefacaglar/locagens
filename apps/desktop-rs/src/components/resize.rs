//! Drag handle on the inner edge of the sidebar and the right panel.

use crate::prefs::{self, Layout};
use crate::state_ui::UiStore;
use dioxus::prelude::*;

/// Which column a handle resizes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    /// Right edge of the left sidebar (dragging right widens it).
    Sidebar,
    /// Left edge of the right panel (dragging left widens it).
    Panel,
}

impl Edge {
    fn width(self, layout: &Layout) -> i32 {
        match self {
            Edge::Sidebar => layout.sidebar_width,
            Edge::Panel => layout.right_panel_width,
        }
    }

    fn with_width(self, layout: Layout, width: i32) -> Layout {
        match self {
            Edge::Sidebar => Layout { sidebar_width: width, ..layout },
            Edge::Panel => Layout { right_panel_width: width, ..layout },
        }
        .clamped()
    }

    fn range(self) -> (i32, i32) {
        match self {
            Edge::Sidebar => prefs::SIDEBAR_RANGE,
            Edge::Panel => prefs::PANEL_RANGE,
        }
    }

    fn default_width(self) -> i32 {
        match self {
            Edge::Sidebar => prefs::SIDEBAR_DEFAULT,
            Edge::Panel => prefs::PANEL_DEFAULT,
        }
    }

    /// +1 when moving the edge to the right makes the column wider.
    fn grow_sign(self) -> i32 {
        match self {
            Edge::Sidebar => 1,
            Edge::Panel => -1,
        }
    }

    fn label_key(self) -> &'static str {
        match self {
            Edge::Sidebar => "resize.sidebar",
            Edge::Panel => "resize.panel",
        }
    }
}

const KEY_STEP: i32 = 16;

fn set_width(store: UiStore, edge: Edge, width: i32) {
    let mut store = store;
    let next = edge.with_width(*store.layout.peek(), width);
    store.layout.set(next);
}

fn persist(store: UiStore) {
    prefs::save_layout(&store.layout.peek());
}

/// Zero-width flex child whose inner strip straddles the column edge.
#[component]
pub fn ResizeHandle(edge: Edge) -> Element {
    let store: UiStore = use_context();
    let width = edge.width(&store.layout.read());
    let (min, max) = edge.range();
    rsx! {
        div {
            class: "resize-anchor",
            div {
                class: "resize-handle",
                role: "separator",
                "aria-orientation": "vertical",
                "aria-label": store.t(edge.label_key()),
                "aria-valuenow": "{width}",
                "aria-valuemin": "{min}",
                "aria-valuemax": "{max}",
                tabindex: "0",
                onmousedown: move |event| {
                    event.prevent_default();
                    let start_x = event.client_coordinates().x;
                    let start_width = *store.layout.peek();
                    let start_width = edge.width(&start_width);
                    spawn(async move {
                        let mut drag = dioxus::document::eval(
                            r#"document.body.style.userSelect = 'none';
                               const move_ = (e) => dioxus.send(e.clientX);
                               const up = () => {
                                 document.removeEventListener('mousemove', move_);
                                 document.removeEventListener('mouseup', up);
                                 document.body.style.userSelect = '';
                                 dioxus.send(-1);
                               };
                               document.addEventListener('mousemove', move_);
                               document.addEventListener('mouseup', up);"#,
                        );
                        while let Ok(x) = drag.recv::<f64>().await {
                            if x < 0.0 {
                                break;
                            }
                            let delta = (x - start_x) as i32 * edge.grow_sign();
                            set_width(store, edge, start_width + delta);
                        }
                        persist(store);
                    });
                },
                ondoubleclick: move |_| {
                    set_width(store, edge, edge.default_width());
                    persist(store);
                },
                onkeydown: move |event: KeyboardEvent| {
                    let step = match event.key() {
                        Key::ArrowRight => KEY_STEP,
                        Key::ArrowLeft => -KEY_STEP,
                        _ => return,
                    };
                    event.prevent_default();
                    let current = edge.width(&store.layout.peek());
                    set_width(store, edge, current + step * edge.grow_sign());
                    persist(store);
                },
            }
        }
    }
}
