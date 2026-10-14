//! How the reference sizes an image for the vision encoder, so llama.cpp sees the same pixels.
//!
//! mtmd fixes the token budget per context and only rescales images outside its range, while the reference always
//! fills the budget. So images are resized here, and mtmd takes them as they are (the shim sets min tokens to 1).
//! Pillow's bicubic filter, ported exactly: the reference vectors depend on these pixels.

/// transformers' Gemma image processor: scale to fill `budget` soft tokens (each 3 × 3 patches of 16 px) keeping the
/// aspect ratio, each side rounded down to a multiple of 48 px. None: too thin to embed. Returns (height, width).
pub fn target_size(height: usize, width: usize, budget: usize) -> Option<(usize, usize)> {
    let (patch, pooling) = (16usize, 3usize);
    let max_patches = budget * pooling * pooling;
    let factor = ((max_patches * patch * patch) as f64 / (height * width) as f64).sqrt();
    let side = pooling * patch;
    let mut target_height = (factor * height as f64 / side as f64).floor() as usize * side;
    let mut target_width = (factor * width as f64 / side as f64).floor() as usize * side;
    if target_height == 0 && target_width == 0 {
        return None;
    }
    let max_side = (max_patches / (pooling * pooling)) * side;
    if target_height == 0 {
        target_height = side;
        target_width = (width / height * side).min(max_side);
    } else if target_width == 0 {
        target_width = side;
        target_height = (height / width * side).min(max_side);
    }
    Some((target_height, target_width))
}

const PRECISION: u32 = 22;

/// Pillow's bicubic resampling of packed RGB (a = −0.5, the kernel widened when shrinking, 22-bit fixed point).
pub fn resize_bicubic(
    source: &[u8],
    width: usize,
    height: usize,
    to_width: usize,
    to_height: usize,
) -> Vec<u8> {
    debug_assert_eq!(source.len(), width * height * 3);
    match (to_width != width, to_height != height) {
        (true, true) => {
            let pass = horizontal(source, width, height, to_width);
            vertical(&pass, height, to_width, to_height)
        }
        (true, false) => horizontal(source, width, height, to_width),
        (false, true) => vertical(source, height, width, to_height),
        (false, false) => source.to_vec(),
    }
}

fn filter(value: f64) -> f64 {
    let x = value.abs();
    let a = -0.5;
    if x < 1.0 {
        ((a + 2.0) * x - (a + 3.0)) * x * x + 1.0
    } else if x < 2.0 {
        (((x - 5.0) * x + 8.0) * x - 4.0) * a
    } else {
        0.0
    }
}

/// For each output pixel: the first contributing input pixel and how many contribute (`bounds`, pairs), and their
/// weights (`size` per output pixel).
fn coefficients(in_size: usize, out_size: usize) -> (Vec<usize>, Vec<i32>, usize) {
    let scale = in_size as f64 / out_size as f64;
    let filter_scale = scale.max(1.0);
    let support = 2.0 * filter_scale;
    let size = support.ceil() as usize * 2 + 1;
    let mut bounds = vec![0usize; out_size * 2];
    let mut weights = vec![0i32; out_size * size];
    let mut row = vec![0f64; size];
    for out in 0..out_size {
        let center = (out as f64 + 0.5) * scale;
        let first = ((center - support + 0.5) as i64).max(0) as usize;
        let last = ((center + support + 0.5) as i64).min(in_size as i64) as usize;
        let count = last - first;
        let mut total = 0.0;
        for (x, value) in row.iter_mut().enumerate() {
            *value = if x < count {
                filter(((x + first) as f64 - center + 0.5) / filter_scale)
            } else {
                0.0
            };
            total += *value;
        }
        for (x, &value) in row.iter().enumerate() {
            let weight = if x < count && total != 0.0 {
                value / total
            } else {
                value
            };
            let rounding = if weight < 0.0 { -0.5 } else { 0.5 };
            weights[out * size + x] = (weight * (1u32 << PRECISION) as f64 + rounding) as i32;
        }
        bounds[out * 2] = first;
        bounds[out * 2 + 1] = count;
    }
    (bounds, weights, size)
}

fn clip(value: i32) -> u8 {
    (value >> PRECISION).clamp(0, 255) as u8
}

fn horizontal(src: &[u8], in_width: usize, rows: usize, out_width: usize) -> Vec<u8> {
    let (bounds, weights, size) = coefficients(in_width, out_width);
    let mut out = vec![0u8; out_width * rows * 3];
    for y in 0..rows {
        for x in 0..out_width {
            let mut p = (y * in_width + bounds[x * 2]) * 3;
            let (mut r, mut g, mut b) = (
                1i32 << (PRECISION - 1),
                1i32 << (PRECISION - 1),
                1i32 << (PRECISION - 1),
            );
            for k in 0..bounds[x * 2 + 1] {
                let w = weights[x * size + k];
                r = r.wrapping_add((src[p] as i32).wrapping_mul(w));
                g = g.wrapping_add((src[p + 1] as i32).wrapping_mul(w));
                b = b.wrapping_add((src[p + 2] as i32).wrapping_mul(w));
                p += 3;
            }
            let o = (y * out_width + x) * 3;
            out[o] = clip(r);
            out[o + 1] = clip(g);
            out[o + 2] = clip(b);
        }
    }
    out
}

fn vertical(src: &[u8], in_height: usize, row_width: usize, out_height: usize) -> Vec<u8> {
    let (bounds, weights, size) = coefficients(in_height, out_height);
    let stride = row_width * 3;
    let mut out = vec![0u8; stride * out_height];
    let mut sums = vec![0i32; stride];
    for y in 0..out_height {
        sums.fill(1 << (PRECISION - 1));
        for k in 0..bounds[y * 2 + 1] {
            let row = &src[(bounds[y * 2] + k) * stride..][..stride];
            let w = weights[y * size + k];
            for (sum, &pixel) in sums.iter_mut().zip(row) {
                *sum = sum.wrapping_add((pixel as i32).wrapping_mul(w));
            }
        }
        for (o, &sum) in out[y * stride..][..stride].iter_mut().zip(&sums) {
            *o = clip(sum);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_fill_the_budget_in_multiples_of_48() {
        // 4:3 photo at 140 tokens: 140 × 9 × 256 px² ≈ 322,560 px² → 624 × 480 after rounding down to 48.
        assert_eq!(target_size(3000, 4000, 140), Some((480, 624)));
        // A square at 280 tokens: sqrt(280 × 9) × 16 ≈ 803 → 768.
        assert_eq!(target_size(1000, 1000, 280), Some((768, 768)));
        // Already the right size stays as it is.
        assert_eq!(target_size(768, 768, 280), Some((768, 768)));
        for budget in crate::BUDGETS {
            let (h, w) = target_size(1080, 1920, budget).unwrap();
            assert_eq!((h % 48, w % 48), (0, 0));
            assert!(h * w <= budget * 9 * 256);
        }
    }

    #[test]
    fn thin_images() {
        // A 1-pixel-tall strip still gets one row of patches, capped at the budget's longest side.
        let (h, w) = target_size(1, 100_000, 70).unwrap();
        assert_eq!(h, 48);
        assert_eq!(w, 70 * 48);
    }

    #[test]
    fn resizing_keeps_flat_colors() {
        let source: Vec<u8> = std::iter::repeat_n([200u8, 100, 50], 97 * 61)
            .flatten()
            .collect();
        let out = resize_bicubic(&source, 97, 61, 48, 96);
        assert_eq!(out.len(), 48 * 96 * 3);
        assert!(out.chunks(3).all(|p| p == [200, 100, 50]));
        assert_eq!(resize_bicubic(&source, 97, 61, 97, 61), source);
    }
}
