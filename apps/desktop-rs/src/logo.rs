//! The Locagens logo mark, rasterized in pure Rust for the window icon.
//!
//! The geometry mirrors `assets/logo.svg` (256x256 design grid): a graphite
//! rounded tile with a hairline edge, carrying an off-white "L" built from two
//! rounded bars and a muted indigo node above the corner. Edges are smoothed by 4x4
//! supersampling, so no image or SVG dependency is needed.

/// Graphite tile (`#131317`), off-white L (`#e6e6ea`), muted indigo node (`#7f89c8`).
const TILE: [f32; 3] = [19.0, 19.0, 23.0];
const FORM: [f32; 3] = [230.0, 230.0, 234.0];
const NODE: [f32; 3] = [127.0, 137.0, 200.0];
const WHITE: [f32; 3] = [255.0, 255.0, 255.0];
const GRID: f32 = 256.0;
const SAMPLES: u32 = 4;

/// Signed distance (negative inside) to a rounded rectangle.
fn rounded_rect(px: f32, py: f32, x: f32, y: f32, w: f32, h: f32, r: f32) -> f32 {
    let (hw, hh) = (w / 2.0, h / 2.0);
    let qx = (px - (x + hw)).abs() - hw + r;
    let qy = (py - (y + hh)).abs() - hh + r;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Straight-alpha RGBA of the mark at one point of the design grid.
fn sample(px: f32, py: f32) -> [f32; 4] {
    let mut rgba = [0.0_f32; 4];
    let mut paint = |inside: bool, color: [f32; 3], alpha: f32| {
        if inside {
            let out_alpha = alpha + rgba[3] * (1.0 - alpha);
            for channel in 0..3 {
                rgba[channel] =
                    (color[channel] * alpha + rgba[channel] * rgba[3] * (1.0 - alpha)) / out_alpha;
            }
            rgba[3] = out_alpha;
        }
    };
    let tile = rounded_rect(px, py, 16.0, 16.0, 224.0, 224.0, 56.0);
    paint(tile <= 0.0, TILE, 1.0);
    // Hairline lighter edge so the dark tile stays visible on dark surfaces.
    paint(tile <= 0.0 && tile > -3.0, WHITE, 0.12);
    paint(rounded_rect(px, py, 78.0, 62.0, 36.0, 132.0, 18.0) <= 0.0, FORM, 1.0);
    paint(rounded_rect(px, py, 78.0, 158.0, 100.0, 36.0, 18.0) <= 0.0, FORM, 1.0);
    paint((px - 176.0).hypot(py - 84.0) <= 18.0, NODE, 1.0);
    rgba
}

/// Renders the mark into a `size` x `size` straight-alpha RGBA buffer.
pub fn render_rgba(size: u32) -> Vec<u8> {
    let mut buffer = Vec::with_capacity((size * size * 4) as usize);
    let step = GRID / size as f32;
    let count = (SAMPLES * SAMPLES) as f32;
    for y in 0..size {
        for x in 0..size {
            // Average premultiplied samples, then un-premultiply.
            let mut sum = [0.0_f32; 4];
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px = (x as f32 + (sx as f32 + 0.5) / SAMPLES as f32) * step;
                    let py = (y as f32 + (sy as f32 + 0.5) / SAMPLES as f32) * step;
                    let [r, g, b, a] = sample(px, py);
                    sum[0] += r * a;
                    sum[1] += g * a;
                    sum[2] += b * a;
                    sum[3] += a;
                }
            }
            for channel in 0..3 {
                let value = if sum[3] > 0.0 { sum[channel] / sum[3] } else { 0.0 };
                buffer.push(value.round().clamp(0.0, 255.0) as u8);
            }
            buffer.push((sum[3] / count * 255.0).round().clamp(0.0, 255.0) as u8);
        }
    }
    buffer
}

/// Window / taskbar icon built at runtime from the rasterized mark.
pub fn window_icon() -> Option<dioxus::desktop::tao::window::Icon> {
    const SIZE: u32 = 256;
    dioxus::desktop::tao::window::Icon::from_rgba(render_rgba(SIZE), SIZE, SIZE).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_buffer_has_the_right_size() {
        for size in [16, 32, 256] {
            assert_eq!(render_rgba(size).len(), (size * size * 4) as usize);
        }
    }

    #[test]
    fn mark_has_opaque_centre_and_transparent_corners() {
        let size = 64;
        let rgba = render_rgba(size);
        let alpha_at = |x: u32, y: u32| rgba[((y * size + x) * 4 + 3) as usize];
        assert_eq!(alpha_at(0, 0), 0, "corner is transparent");
        assert_eq!(alpha_at(size / 2, size / 2), 255, "centre is opaque");
        let opaque = rgba.chunks(4).filter(|px| px[3] > 0).count();
        assert!(opaque > (size * size / 2) as usize);
    }

    #[test]
    fn window_icon_builds() {
        assert!(window_icon().is_some());
    }

    /// One-off helper for the .ico script: `LOGO_DUMP_DIR=<dir> cargo test
    /// -p locagens-desktop dump_icon_sources -- --ignored`.
    #[test]
    #[ignore]
    fn dump_icon_sources() {
        let dir = std::env::var("LOGO_DUMP_DIR").expect("set LOGO_DUMP_DIR");
        for size in [16u32, 24, 32, 48, 64, 128, 256] {
            std::fs::write(format!("{dir}/logo{size}.rgba"), render_rgba(size)).unwrap();
        }
    }
}
