//! Mac Glow - an EffectCraft effect plug-in (WebAssembly ABI v1, no imports).
//!
//! How it works:
//!   1. Pick out the bright parts of the layer (Threshold) and keep only those.
//!   2. Blur them (Radius). The blur runs at reduced resolution for big radii,
//!      which keeps it fast inside EffectCraft's fuel-limited interpreter.
//!   3. Add the blurred light back on top (Intensity), optionally tinted.
//!
//! Parameters arrive flattened to f64 in the order they are declared in MANIFEST:
//!   0 threshold   1 radius   2 intensity   3..=6 tint (RGBA)   7 tint_amount   8 glow_only

use std::cell::RefCell;

const MANIFEST: &str = r#"{
  "api": 1,
  "id": "com.macstudios.mac-glow",
  "name": "Mac Glow",
  "category": "Stylize",
  "version": "1.0.0",
  "author": "Mac Studios",
  "description": "Soft glow from the bright parts of a layer, with optional tint.",
  "params": [
    {"id": "threshold", "name": "Glow Threshold", "type": "slider", "default": 60, "min": 0, "max": 99, "decimals": 0},
    {"id": "radius", "name": "Glow Radius", "type": "slider", "default": 40, "min": 0, "max": 1000, "sliderMax": 200, "decimals": 0},
    {"id": "intensity", "name": "Glow Intensity", "type": "slider", "default": 100, "min": 0, "max": 1000, "sliderMax": 400, "decimals": 0},
    {"id": "tint", "name": "Glow Color", "type": "color", "default": [1.0, 0.75, 0.35, 1.0]},
    {"id": "tint_amount", "name": "Color Amount", "type": "slider", "default": 0, "min": 0, "max": 100, "decimals": 0},
    {"id": "glow_only", "name": "Glow Only", "type": "checkbox", "default": false}
  ]
}"#;

const N_PARAMS: usize = 9;

// ---------------------------------------------------------------------------
// ABI exports
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn ec_api_version() -> i32 {
    1
}

#[no_mangle]
pub extern "C" fn ec_manifest_ptr() -> i32 {
    MANIFEST.as_ptr() as usize as i32
}

#[no_mangle]
pub extern "C" fn ec_manifest_len() -> i32 {
    MANIFEST.len() as i32
}

thread_local! {
    // u64 storage guarantees the 8-byte alignment the host expects.
    static BUF: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

#[no_mangle]
pub extern "C" fn ec_alloc(bytes: i32) -> i32 {
    let words = ((bytes.max(0) as usize) + 7) / 8;
    BUF.with(|b| {
        let mut b = b.borrow_mut();
        if b.len() < words {
            *b = vec![0u64; words];
        }
        b.as_mut_ptr() as usize as i32
    })
}

#[no_mangle]
pub extern "C" fn ec_render(
    pixels: i32,
    width: i32,
    height: i32,
    params: i32,
    nparams: i32,
    _time: f64,
    scale: f64,
) -> i32 {
    if width <= 0 || height <= 0 || (nparams as usize) < N_PARAMS {
        return 1;
    }
    let w = width as usize;
    let h = height as usize;
    let p = unsafe { std::slice::from_raw_parts(params as usize as *const f64, nparams as usize) };
    let px = unsafe { std::slice::from_raw_parts_mut(pixels as usize as *mut f32, w * h * 4) };
    glow(px, w, h, p, scale);
    0
}

// ---------------------------------------------------------------------------
// The effect
// ---------------------------------------------------------------------------

fn luma(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn glow(px: &mut [f32], w: usize, h: usize, p: &[f64], scale: f64) {
    let thr = ((p[0] / 100.0) as f32).clamp(0.0, 0.99);
    let radius = (p[1] * scale).max(0.0) as f32; // in buffer pixels
    let intensity = (p[2] / 100.0).max(0.0) as f32;
    let tint = [p[3] as f32, p[4] as f32, p[5] as f32];
    let tint_amount = ((p[7] / 100.0) as f32).clamp(0.0, 1.0);
    let glow_only = p[8] >= 0.5;

    if radius < 0.5 || intensity <= 0.0 {
        if glow_only {
            px.iter_mut().for_each(|v| *v = 0.0);
        }
        return;
    }

    // Three box passes approximate a Gaussian whose sigma is about the box half-width.
    let half = radius * 0.5;
    // Work at reduced resolution when the blur is wide.
    let ds = ((half / 3.0) as usize).clamp(1, 16);
    let lw = (w + ds - 1) / ds;
    let lh = (h + ds - 1) / ds;
    let b_low = ((half / ds as f32).round() as usize).max(1);

    // 1. Bright-pass, averaged down into the low-res buffer.
    let mut low = vec![0.0f32; lw * lh * 4];
    let mut count = vec![0.0f32; lw * lh];
    for y in 0..h {
        let ly = y / ds;
        for x in 0..w {
            let i = (y * w + x) * 4;
            let (r, g, b, a) = (px[i], px[i + 1], px[i + 2], px[i + 3]);
            let li = ly * lw + x / ds;
            count[li] += 1.0;
            if a <= 1e-6 {
                continue;
            }
            let t = ((luma(r, g, b) / a - thr) / (1.0 - thr)).clamp(0.0, 1.0);
            let k = t * t * (3.0 - 2.0 * t);
            if k > 0.0 {
                let o = li * 4;
                low[o] += r * k;
                low[o + 1] += g * k;
                low[o + 2] += b * k;
                low[o + 3] += a * k;
            }
        }
    }
    for (li, c) in count.iter().enumerate() {
        if *c > 0.0 {
            let inv = 1.0 / *c;
            let o = li * 4;
            low[o] *= inv;
            low[o + 1] *= inv;
            low[o + 2] *= inv;
            low[o + 3] *= inv;
        }
    }
    drop(count);

    // 2. Blur: three box passes per axis, transparent outside the buffer.
    let mut tmp = vec![0.0f32; lw.max(lh)];
    for _ in 0..3 {
        for y in 0..lh {
            for c in 0..4 {
                box_line(&mut low, &mut tmp, y * lw * 4 + c, 4, lw, b_low);
            }
        }
        for x in 0..lw {
            for c in 0..4 {
                box_line(&mut low, &mut tmp, x * 4 + c, lw * 4, lh, b_low);
            }
        }
    }
    drop(tmp);

    // 3. Upsample (bilinear) and add back.
    let inv_ds = 1.0 / ds as f32;
    for y in 0..h {
        let fy = ((y as f32 + 0.5) * inv_ds - 0.5).max(0.0);
        let y0 = (fy as usize).min(lh - 1);
        let y1 = (y0 + 1).min(lh - 1);
        let wy = fy - y0 as f32;
        for x in 0..w {
            let fx = ((x as f32 + 0.5) * inv_ds - 0.5).max(0.0);
            let x0 = (fx as usize).min(lw - 1);
            let x1 = (x0 + 1).min(lw - 1);
            let wx = fx - x0 as f32;

            let a00 = (y0 * lw + x0) * 4;
            let a10 = (y0 * lw + x1) * 4;
            let a01 = (y1 * lw + x0) * 4;
            let a11 = (y1 * lw + x1) * 4;
            let mut gl = [0.0f32; 4];
            for c in 0..4 {
                let top = low[a00 + c] * (1.0 - wx) + low[a10 + c] * wx;
                let bot = low[a01 + c] * (1.0 - wx) + low[a11 + c] * wx;
                gl[c] = (top * (1.0 - wy) + bot * wy) * intensity;
            }

            if tint_amount > 0.0 {
                let l = luma(gl[0], gl[1], gl[2]);
                for c in 0..3 {
                    gl[c] = gl[c] * (1.0 - tint_amount) + tint[c] * l * tint_amount;
                }
            }
            let ga = gl[3].clamp(0.0, 1.0);

            let i = (y * w + x) * 4;
            if glow_only {
                px[i] = gl[0];
                px[i + 1] = gl[1];
                px[i + 2] = gl[2];
                px[i + 3] = ga;
            } else {
                let a = px[i + 3];
                px[i] += gl[0];
                px[i + 1] += gl[1];
                px[i + 2] += gl[2];
                px[i + 3] = a + ga - a * ga;
            }
        }
    }
}

/// Box blur of one line of `n` samples starting at `start`, `step` apart.
/// Samples outside the line count as zero.
fn box_line(buf: &mut [f32], tmp: &mut [f32], start: usize, step: usize, n: usize, b: usize) {
    for j in 0..n {
        tmp[j] = buf[start + j * step];
    }
    let inv = 1.0 / (2 * b + 1) as f32;
    let mut sum = 0.0f32;
    for j in 0..=b.min(n - 1) {
        sum += tmp[j];
    }
    for i in 0..n {
        buf[start + i * step] = sum * inv;
        let add = i + b + 1;
        if add < n {
            sum += tmp[add];
        }
        if i >= b {
            sum -= tmp[i - b];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // threshold, radius, intensity, tint r g b a, tint_amount, glow_only
    fn params(thr: f64, radius: f64, intensity: f64, tint_amount: f64, only: bool) -> Vec<f64> {
        vec![thr, radius, intensity, 1.0, 0.0, 0.0, 1.0, tint_amount, if only { 1.0 } else { 0.0 }]
    }

    /// Dark opaque frame with a bright white 8x8 square in the middle.
    fn dot_frame(w: usize, h: usize) -> Vec<f32> {
        let mut px = vec![0.0f32; w * h * 4];
        for i in 0..w * h {
            px[i * 4 + 3] = 1.0; // opaque black
        }
        for dy in -4i32..4 {
            for dx in -4i32..4 {
                let x = (w / 2) as i32 + dx;
                let y = (h / 2) as i32 + dy;
                if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                    let c = (y as usize * w + x as usize) * 4;
                    px[c..c + 4].copy_from_slice(&[1.0, 1.0, 1.0, 1.0]);
                }
            }
        }
        px
    }

    #[test]
    fn glow_spreads_from_bright_pixel() {
        let (w, h) = (64, 64);
        let mut px = dot_frame(w, h);
        glow(&mut px, w, h, &params(50.0, 20.0, 400.0, 0.0, false), 1.0);
        let near = ((h / 2) * w + w / 2 + 3) * 4;
        let far = (2 * w + 2) * 4;
        assert!(px[near] > 0.01, "expected glow next to the dot, got {}", px[near]);
        assert!(px[far] < px[near], "glow should fall off with distance");
        assert!(px.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn nothing_bright_means_no_change() {
        let (w, h) = (32, 32);
        let mut px = vec![0.0f32; w * h * 4];
        for i in 0..w * h {
            px[i * 4] = 0.2;
            px[i * 4 + 1] = 0.2;
            px[i * 4 + 2] = 0.2;
            px[i * 4 + 3] = 1.0;
        }
        let before = px.clone();
        glow(&mut px, w, h, &params(60.0, 30.0, 100.0, 0.0, false), 1.0);
        assert_eq!(before, px);
    }

    #[test]
    fn glow_only_leaves_dark_areas_transparent() {
        let (w, h) = (64, 64);
        let mut px = dot_frame(w, h);
        glow(&mut px, w, h, &params(50.0, 8.0, 100.0, 0.0, true), 1.0);
        let corner = 0;
        assert!(px[corner + 3] < 0.001, "corner alpha {}", px[corner + 3]);
        let mid = ((h / 2) * w + w / 2) * 4;
        assert!(px[mid + 3] > 0.0);
    }

    #[test]
    fn tint_changes_glow_colour() {
        let (w, h) = (64, 64);
        let mut px = dot_frame(w, h);
        // tint is pure red
        glow(&mut px, w, h, &params(50.0, 20.0, 400.0, 100.0, true), 1.0);
        let near = ((h / 2) * w + w / 2 + 4) * 4;
        assert!(px[near] > 0.0 && px[near + 1] == 0.0 && px[near + 2] == 0.0);
    }

    #[test]
    fn odd_sizes_and_huge_radius_do_not_panic() {
        for &(w, h) in &[(1usize, 1usize), (3, 200), (37, 23), (200, 3), (101, 77)] {
            let mut px = dot_frame(w, h);
            glow(&mut px, w, h, &params(10.0, 1000.0, 1000.0, 50.0, false), 2.0);
            assert!(px.iter().all(|v| v.is_finite()), "{}x{}", w, h);
            let mut px = dot_frame(w, h);
            glow(&mut px, w, h, &params(10.0, 3.0, 100.0, 0.0, false), 0.5);
            assert!(px.iter().all(|v| v.is_finite()), "{}x{}", w, h);
        }
    }

    #[test]
    fn zero_radius_or_intensity_is_a_no_op() {
        let (w, h) = (16, 16);
        let mut px = dot_frame(w, h);
        let before = px.clone();
        glow(&mut px, w, h, &params(50.0, 0.0, 100.0, 0.0, false), 1.0);
        glow(&mut px, w, h, &params(50.0, 30.0, 0.0, 0.0, false), 1.0);
        assert_eq!(before, px);
    }

    #[test]
    fn manifest_matches_param_layout() {
        assert!(MANIFEST.contains("\"api\": 1"));
        assert_eq!(MANIFEST.matches("\"type\"").count(), 6);
        assert_eq!(N_PARAMS, 1 + 1 + 1 + 4 + 1 + 1);
    }
}
