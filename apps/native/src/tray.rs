//! The tray icon's picture: two live bars (processor and memory) on a dark
//! rounded square, drawn for the size Windows asks for. Pure code, so the
//! quantising and the colors are unit tested.

use pulse_readings::Snapshot;

/// Drawing space, as in the reference page's tray icon.
const SPACE: f32 = 64.0;
const TRACK_TOP: f32 = 11.0;
const TRACK_H: f32 = 42.0;
const BAR_W: f32 = 15.0;
const BAR_X: [f32; 2] = [13.0, 37.0];
const MIN_FILL: f32 = 5.0;

/// The sizes the tray uses by DPI.
pub const SIZES: [u32; 4] = [16, 20, 24, 32];

/// The tray size for a DPI (96 -> 16, 120 -> 20, 144 -> 24, 192 and up -> 32).
pub fn size_for_dpi(dpi: u32) -> u32 {
    let want = 16.0 * dpi.max(96) as f32 / 96.0;
    SIZES
        .into_iter()
        .min_by(|a, b| {
            (*a as f32 - want)
                .abs()
                .total_cmp(&(*b as f32 - want).abs())
        })
        .unwrap_or(16)
}

/// Everything that changes the picture. Equal keys draw the same pixels, so
/// the icon is only redrawn when the key changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub size: u32,
    /// Bar heights in half pixels.
    pub cpu: u32,
    pub mem: u32,
    pub warn: bool,
}

impl Key {
    /// `cpu` and `mem` are 0 to 1.
    pub fn new(size: u32, cpu: f32, mem: f32, warn: bool) -> Self {
        let q = |f: f32| {
            let design = (TRACK_H * f.clamp(0.0, 1.0)).max(MIN_FILL);
            (design * size as f32 / SPACE * 2.0).round() as u32
        };
        Self {
            size,
            cpu: q(cpu),
            mem: q(mem),
            warn,
        }
    }
}

/// The picture a reading asks for, at the tray size for `dpi`.
#[allow(clippy::cast_precision_loss)]
pub fn key_for(snap: &Snapshot, dpi: u32) -> Key {
    let mem = if snap.mem_total == 0 {
        0.0
    } else {
        (snap.mem_used as f64 / snap.mem_total as f64) as f32
    };
    Key::new(
        size_for_dpi(dpi),
        snap.cpu_pct / 100.0,
        mem,
        crate::view::status(snap, false, true).warn,
    )
}

const WARN: [u8; 3] = [0xff, 0x9b, 0x6b];

fn colors(k: &Key) -> [[u8; 3]; 2] {
    // The panel's own accents: blue for the processor, violet for memory.
    let [cpu, mem] = [[0x60, 0xa5, 0xfa], [0xa7, 0x8b, 0xfa]];
    [if k.warn { WARN } else { cpu }, mem]
}

/// Straight RGBA, `size` x `size`, top-down.
pub fn render(k: &Key) -> Vec<u8> {
    let n = k.size as usize;
    let mut rgba = vec![0u8; n * n * 4];
    let s = SPACE / n as f32;
    let cover = |px: usize, py: usize, f: &dyn Fn(f32, f32) -> bool| -> f32 {
        let mut c = 0;
        for sy in 0..4 {
            for sx in 0..4 {
                let x = (px as f32 + (sx as f32 + 0.5) / 4.0) * s;
                let y = (py as f32 + (sy as f32 + 0.5) / 4.0) * s;
                c += usize::from(f(x, y));
            }
        }
        c as f32 / 16.0
    };
    let round_rect = |x: f32, y: f32| {
        let (cx, cy) = (x.clamp(16.0, 48.0), y.clamp(16.0, 48.0));
        (2.0..=62.0).contains(&x) && (2.0..=62.0).contains(&y) && (x - cx).hypot(y - cy) <= 14.0
    };
    let cols = colors(k);
    let heights = [k.cpu, k.mem].map(|q| q as f32 / 2.0 * s);
    for py in 0..n {
        for px in 0..n {
            let base = cover(px, py, &round_rect);
            if base == 0.0 {
                continue;
            }
            let mut col = [13.0f32, 15.0, 14.0];
            for i in 0..2 {
                let x0 = BAR_X[i];
                let h = heights[i];
                let track = cover(px, py, &|x, y| {
                    x >= x0 && x < x0 + BAR_W && (TRACK_TOP..TRACK_TOP + TRACK_H).contains(&y)
                });
                let fill = cover(px, py, &|x, y| {
                    x >= x0
                        && x < x0 + BAR_W
                        && y >= TRACK_TOP + TRACK_H - h
                        && y < TRACK_TOP + TRACK_H
                });
                for c in 0..3 {
                    col[c] += (f32::from(cols[i][c]) - col[c]) * (track * 0.22);
                    col[c] += (f32::from(cols[i][c]) - col[c]) * fill;
                }
            }
            let at = (py * n + px) * 4;
            rgba[at] = col[0] as u8;
            rgba[at + 1] = col[1] as u8;
            rgba[at + 2] = col[2] as u8;
            rgba[at + 3] = (base * 255.0) as u8;
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_follows_the_dpi() {
        for (dpi, want) in [(96, 16), (120, 20), (144, 24), (192, 32), (288, 32)] {
            assert_eq!(size_for_dpi(dpi), want, "dpi {dpi}");
        }
        assert_eq!(size_for_dpi(0), 16);
    }

    #[test]
    fn a_one_percent_move_does_not_change_the_key() {
        for size in SIZES {
            let a = Key::new(size, 0.24, 0.43, false);
            let b = Key::new(size, 0.245, 0.435, false);
            assert_eq!(a, b, "size {size}");
        }
        assert_ne!(Key::new(16, 0.2, 0.4, false), Key::new(16, 0.5, 0.4, false));
        // A warning redraws.
        assert_ne!(Key::new(16, 0.2, 0.4, false), Key::new(16, 0.2, 0.4, true));
    }

    #[test]
    fn a_reading_maps_to_a_key_and_a_warning_follows_the_status() {
        use pulse_readings::{Sample, Source};
        let mut snap = Sample::default().read(true);
        snap.cpu_pct = 10.0;
        snap.mem_used = snap.mem_total / 2;
        let calm = key_for(&snap, 96);
        assert_eq!(calm.size, 16);
        assert!(!calm.warn);
        assert_eq!(calm, Key::new(16, 0.1, 0.5, false));
        snap.cpu_pct = 90.0;
        let busy = key_for(&snap, 96);
        assert!(busy.warn);
        // Net rates change every tick but are not part of the picture.
        let mut other = snap.clone();
        other.net_down_bps += 12_345;
        assert_eq!(key_for(&snap, 96), key_for(&other, 96));
    }

    #[test]
    fn empty_bars_keep_a_visible_sliver() {
        assert!(Key::new(16, 0.0, 0.0, false).cpu >= 2);
    }

    #[test]
    fn pixels_have_the_right_size_and_corners_are_clear() {
        for size in SIZES {
            let px = render(&Key::new(size, 0.5, 0.5, false));
            assert_eq!(px.len(), (size * size * 4) as usize);
            assert_eq!(px[3], 0, "top-left corner is transparent at {size}");
        }
    }

    fn bar_pixel(k: &Key, bar: usize) -> [u8; 3] {
        // The middle of the bar's lowest row of pixels.
        let n = k.size as usize;
        let x = ((BAR_X[bar] + BAR_W / 2.0) / SPACE * n as f32) as usize;
        let y = ((TRACK_TOP + TRACK_H - 2.0) / SPACE * n as f32) as usize;
        let at = (y * n + x) * 4;
        let p = render(k);
        [p[at], p[at + 1], p[at + 2]]
    }

    #[test]
    fn accent_and_warning_colors() {
        let calm = Key::new(32, 0.9, 0.9, false);
        assert_eq!(bar_pixel(&calm, 0), [0x60, 0xa5, 0xfa]);
        assert_eq!(bar_pixel(&calm, 1), [0xa7, 0x8b, 0xfa]);
        let warn = Key::new(32, 0.9, 0.9, true);
        assert_eq!(bar_pixel(&warn, 0), [255, 155, 107]);
        assert_eq!(bar_pixel(&warn, 1), [0xa7, 0x8b, 0xfa]);
    }
}
