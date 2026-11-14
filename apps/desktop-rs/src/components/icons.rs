//! The one icon set of the desktop UI: lucide-style outline glyphs on a 24px
//! grid, 1.5 stroke, round caps and joins, `currentColor` only. Icons are 16px
//! by default (`.icon`); dense rows shrink them to 14px through CSS. The
//! window-control glyphs and the logo mark are drawn on their own grids.

use dioxus::prelude::*;

/// Raw SVG of the copy / check glyphs for HTML strings (markdown code blocks);
/// same paths and stroke settings as [`icon_copy`] and [`icon_check`].
pub const COPY_SVG: &str = "<svg class=\"icon\" width=\"14\" height=\"14\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><rect x=\"9\" y=\"9\" width=\"12\" height=\"12\" rx=\"2.5\"/><path d=\"M5 15V6a2 2 0 0 1 2-2h8\"/></svg>";
pub const CHECK_SVG: &str = "<svg class=\"icon\" width=\"14\" height=\"14\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M20 6 9 17l-5-5\"/></svg>";

/// Wraps the shapes of one glyph in the shared outline `<svg>`.
fn outline(shapes: Element) -> Element {
    rsx! {
        svg {
            class: "icon",
            width: "16", height: "16", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "1.5",
            stroke_linecap: "round", stroke_linejoin: "round",
            "aria-hidden": "true",
            {shapes}
        }
    }
}

pub fn icon_plus() -> Element {
    outline(rsx! { path { d: "M5 12h14" } path { d: "M12 5v14" } })
}

pub fn icon_copy() -> Element {
    outline(rsx! {
        rect { x: "9", y: "9", width: "12", height: "12", rx: "2.5" }
        path { d: "M5 15V6a2 2 0 0 1 2-2h8" }
    })
}

pub fn icon_x() -> Element {
    outline(rsx! { path { d: "M18 6 6 18" } path { d: "m6 6 12 12" } })
}

pub fn icon_check() -> Element {
    outline(rsx! { path { d: "M20 6 9 17l-5-5" } })
}

pub fn icon_chevron_down() -> Element {
    outline(rsx! { path { d: "m6 9 6 6 6-6" } })
}

pub fn icon_chevron_right() -> Element {
    outline(rsx! { path { d: "m9 18 6-6-6-6" } })
}

pub fn icon_arrow_up() -> Element {
    outline(rsx! { path { d: "m5 12 7-7 7 7" } path { d: "M12 19V5" } })
}

pub fn icon_arrow_left() -> Element {
    outline(rsx! { path { d: "M19 12H5" } path { d: "m12 19-7-7 7-7" } })
}

pub fn icon_arrow_right() -> Element {
    outline(rsx! { path { d: "M5 12h14" } path { d: "m12 5 7 7-7 7" } })
}

pub fn icon_search() -> Element {
    outline(rsx! { circle { cx: "11", cy: "11", r: "7" } path { d: "m21 21-4.3-4.3" } })
}

pub fn icon_filter() -> Element {
    outline(rsx! { path { d: "M4 6h16" } path { d: "M7 12h10" } path { d: "M10 18h4" } })
}

pub fn icon_clock() -> Element {
    outline(rsx! { circle { cx: "12", cy: "12", r: "9" } path { d: "M12 7v5l3.5 2" } })
}

pub fn icon_gear() -> Element {
    outline(rsx! {
        path { d: "M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" }
        circle { cx: "12", cy: "12", r: "3" }
    })
}

pub fn icon_dots_vertical() -> Element {
    outline(rsx! {
        circle { cx: "12", cy: "5", r: "1" }
        circle { cx: "12", cy: "12", r: "1" }
        circle { cx: "12", cy: "19", r: "1" }
    })
}

pub fn icon_dots_horizontal() -> Element {
    outline(rsx! {
        circle { cx: "5", cy: "12", r: "1" }
        circle { cx: "12", cy: "12", r: "1" }
        circle { cx: "19", cy: "12", r: "1" }
    })
}

/// Document outline (changed-file headers).
pub fn icon_file() -> Element {
    outline(rsx! {
        path { d: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z" }
        path { d: "M14 3v5h5" }
    })
}

/// Arrows pulling apart vertically: expand hidden lines.
pub fn icon_unfold() -> Element {
    outline(rsx! {
        path { d: "M12 22v-6" }
        path { d: "M12 8V2" }
        path { d: "m15 19-3 3-3-3" }
        path { d: "m15 5-3-3-3 3" }
    })
}

pub fn icon_sidebar() -> Element {
    outline(rsx! { rect { x: "3", y: "3", width: "18", height: "18", rx: "2.5" } path { d: "M9 3v18" } })
}

pub fn icon_panel_right() -> Element {
    outline(rsx! { rect { x: "3", y: "3", width: "18", height: "18", rx: "2.5" } path { d: "M15 3v18" } })
}

pub fn icon_sun() -> Element {
    outline(rsx! {
        circle { cx: "12", cy: "12", r: "4" }
        path { d: "M12 2v2" } path { d: "M12 20v2" }
        path { d: "m4.93 4.93 1.41 1.41" } path { d: "m17.66 17.66 1.41 1.41" }
        path { d: "M2 12h2" } path { d: "M20 12h2" }
        path { d: "m6.34 17.66-1.41 1.41" } path { d: "m19.07 4.93-1.41 1.41" }
    })
}

pub fn icon_moon() -> Element {
    outline(rsx! { path { d: "M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" } })
}

pub fn icon_folder() -> Element {
    outline(rsx! {
        path { d: "M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" }
    })
}

pub fn icon_folder_open() -> Element {
    outline(rsx! {
        path { d: "m6 14 1.5-2.9A2 2 0 0 1 9.24 10H20a2 2 0 0 1 1.94 2.5l-1.54 6a2 2 0 0 1-1.95 1.5H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.9a2 2 0 0 1 1.69.9l.81 1.2a2 2 0 0 0 1.67.9H18a2 2 0 0 1 2 2v2" }
    })
}

pub fn icon_folder_plus() -> Element {
    outline(rsx! {
        path { d: "M12 10v6" } path { d: "M9 13h6" }
        path { d: "M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z" }
    })
}

pub fn icon_paperclip() -> Element {
    outline(rsx! {
        path { d: "M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" }
    })
}

pub fn icon_at() -> Element {
    outline(rsx! {
        circle { cx: "12", cy: "12", r: "4" }
        path { d: "M16 8v5a3 3 0 0 0 6 0v-1a10 10 0 1 0-4 8" }
    })
}

pub fn icon_actions() -> Element {
    outline(rsx! {
        path { d: "M12 3H5a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" }
        path { d: "M18.375 2.625a2.121 2.121 0 1 1 3 3L12 15l-4 1 1-4Z" }
    })
}

pub fn icon_shield() -> Element {
    outline(rsx! { path { d: "M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" } })
}

pub fn icon_list_checks() -> Element {
    outline(rsx! {
        path { d: "m3 17 2 2 4-4" } path { d: "m3 7 2 2 4-4" }
        path { d: "M13 6h8" } path { d: "M13 12h8" } path { d: "M13 18h8" }
    })
}

pub fn icon_users() -> Element {
    outline(rsx! {
        path { d: "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" }
        circle { cx: "9", cy: "7", r: "4" }
        path { d: "M22 21v-2a4 4 0 0 0-3-3.87" }
        path { d: "M16 3.13a4 4 0 0 1 0 7.75" }
    })
}

pub fn icon_git_branch() -> Element {
    outline(rsx! {
        path { d: "M6 3v12" }
        circle { cx: "18", cy: "6", r: "3" }
        circle { cx: "6", cy: "18", r: "3" }
        path { d: "M18 9a9 9 0 0 1-9 9" }
    })
}

pub fn icon_terminal() -> Element {
    outline(rsx! { path { d: "m4 17 6-6-6-6" } path { d: "M12 19h8" } })
}

pub fn icon_wrench() -> Element {
    outline(rsx! {
        path { d: "M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z" }
    })
}

pub fn icon_stop() -> Element {
    outline(rsx! { rect { x: "6", y: "6", width: "12", height: "12", rx: "2.5", fill: "currentColor" } })
}

/// Plan task not started yet: empty ring.
pub fn icon_task_pending() -> Element {
    outline(rsx! { circle { cx: "12", cy: "12", r: "8.5" } })
}

/// Plan task in progress: ring with the right half filled.
pub fn icon_task_active() -> Element {
    outline(rsx! {
        circle { cx: "12", cy: "12", r: "8.5" }
        path { d: "M12 6.5a5.5 5.5 0 0 1 0 11Z", fill: "currentColor", stroke: "none" }
    })
}

/// Plan task completed: ring with a check.
pub fn icon_task_done() -> Element {
    outline(rsx! { circle { cx: "12", cy: "12", r: "8.5" } path { d: "m8.5 12.3 2.5 2.5 4.5-5" } })
}

/// Wraps a window-control glyph (10px grid, 1px stroke, like Windows 11).
fn window_glyph(shapes: Element) -> Element {
    rsx! {
        svg {
            class: "win-glyph",
            width: "10", height: "10", view_box: "0 0 10 10", fill: "none",
            stroke: "currentColor", stroke_width: "1",
            stroke_linecap: "round", stroke_linejoin: "round",
            "aria-hidden": "true",
            {shapes}
        }
    }
}

pub fn window_minimize() -> Element {
    window_glyph(rsx! { path { d: "M0.5 5h9" } })
}

pub fn window_maximize() -> Element {
    window_glyph(rsx! { rect { x: "0.5", y: "0.5", width: "9", height: "9", rx: "1.5" } })
}

pub fn window_restore() -> Element {
    window_glyph(rsx! {
        path { d: "M2.5 2.5V2A1.5 1.5 0 0 1 4 0.5h4A1.5 1.5 0 0 1 9.5 2v4A1.5 1.5 0 0 1 8 7.5h-0.5" }
        rect { x: "0.5", y: "2.5", width: "7", height: "7", rx: "1.5" }
    })
}

pub fn window_close() -> Element {
    window_glyph(rsx! { path { d: "M0.5 0.5l9 9" } path { d: "M9.5 0.5l-9 9" } })
}

/// The Locagens logo mark (same geometry as `assets/logo.svg`); the square
/// takes the `--brand` token so it follows the theme's accent.
pub fn logo_mark() -> Element {
    rsx! {
        svg {
            class: "logo-mark",
            width: "22", height: "22", view_box: "16 16 224 224",
            "aria-hidden": "true",
            rect { class: "lm-bg", x: "16", y: "16", width: "224", height: "224", rx: "56" }
            rect { class: "lm-fg", x: "78", y: "62", width: "36", height: "132", rx: "18" }
            rect { class: "lm-fg", x: "78", y: "158", width: "100", height: "36", rx: "18" }
            circle { class: "lm-fg lm-node", cx: "176", cy: "84", r: "18" }
        }
    }
}
