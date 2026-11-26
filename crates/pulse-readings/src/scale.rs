//! Icon resizing with an area (box) filter. Pure code, so it is tested everywhere.

/// BGRA at (w, h) to straight RGBA at (size, size).
///
/// Each output pixel is the area-weighted average of the source pixels it
/// covers, with colour weighted by alpha (so transparent pixels do not bleed
/// their colour into the edge). The same size is a plain channel swap.
pub(crate) fn bgra_to_rgba(bgra: &[u8], w: u32, h: u32, size: u32) -> Vec<u8> {
    let (w, h, size) = (w as usize, h as usize, size as usize);
    let mut out = Vec::with_capacity(size * size * 4);
    if w == size && h == size {
        for p in bgra.chunks_exact(4) {
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
        return out;
    }
    let (sx, sy) = (w as f64 / size as f64, h as f64 / size as f64);
    for y in 0..size {
        let (y0, y1) = (y as f64 * sy, (y + 1) as f64 * sy);
        for x in 0..size {
            let (x0, x1) = (x as f64 * sx, (x + 1) as f64 * sx);
            let (mut r, mut g, mut b, mut a, mut area) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for py in (y0.floor() as usize)..(y1.ceil() as usize).min(h) {
                let wy = (y1.min((py + 1) as f64) - y0.max(py as f64)).max(0.0);
                for px in (x0.floor() as usize)..(x1.ceil() as usize).min(w) {
                    let wx = (x1.min((px + 1) as f64) - x0.max(px as f64)).max(0.0);
                    let wt = wx * wy;
                    let i = (py * w + px) * 4;
                    let pa = f64::from(bgra[i + 3]) / 255.0;
                    b += f64::from(bgra[i]) * pa * wt;
                    g += f64::from(bgra[i + 1]) * pa * wt;
                    r += f64::from(bgra[i + 2]) * pa * wt;
                    a += pa * wt;
                    area += wt;
                }
            }
            if a <= 0.0 || area <= 0.0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let q = |v: f64| (v / a).round().clamp(0.0, 255.0) as u8;
                out.extend_from_slice(&[q(r), q(g), q(b), (a / area * 255.0).round() as u8]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_size_swaps_channels_only() {
        let px = [10u8, 20, 30, 255, 1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 9];
        let out = bgra_to_rgba(&px, 2, 2, 2);
        assert_eq!(&out[..4], &[30, 20, 10, 255]);
        assert_eq!(&out[4..8], &[3, 2, 1, 4]);
    }

    #[test]
    fn halving_averages_each_block() {
        // 2x2: black, white, white, black (opaque) -> one mid-grey pixel.
        let px = [
            0u8, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255,
        ];
        let out = bgra_to_rgba(&px, 2, 2, 1);
        assert_eq!(out, [128, 128, 128, 255]);
    }

    #[test]
    fn transparent_pixels_do_not_tint_the_edge() {
        // Left opaque red, right fully transparent (with garbage colour).
        let px = [0u8, 0, 255, 255, 200, 200, 200, 0];
        let out = bgra_to_rgba(&px, 2, 1, 1);
        assert_eq!(out, [255, 0, 0, 128]);
    }

    #[test]
    fn odd_ratios_cover_every_source_pixel_once() {
        // 3 -> 2 on a flat colour stays flat and opaque.
        let px = vec![40u8; 3 * 3 * 4];
        let out = bgra_to_rgba(&px, 3, 3, 2);
        assert_eq!(out.len(), 16);
        assert!(out.chunks_exact(4).all(|p| p == [40, 40, 40, 40]));
    }

    #[test]
    fn upscaling_keeps_the_shape() {
        let px = [0u8, 0, 255, 255];
        let out = bgra_to_rgba(&px, 1, 1, 3);
        assert_eq!(out.len(), 36);
        assert!(out.chunks_exact(4).all(|p| p == [255, 0, 0, 255]));
    }
}
