//! Packed 8-bit RGB in memory: what the decoders hand over and the vision encoder takes.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbImage {
    pub width: usize,
    pub height: usize,
    /// width × height × 3
    pub pixels: Vec<u8>,
}

impl RgbImage {
    pub fn new(width: usize, height: usize, pixels: Vec<u8>) -> RgbImage {
        assert_eq!(pixels.len(), width * height * 3, "RGB size");
        RgbImage {
            width,
            height,
            pixels,
        }
    }

    /// Turned the way the camera held it (EXIF orientation 1–8).
    pub fn oriented(self, orientation: u16) -> RgbImage {
        if !(2..=8).contains(&orientation) {
            return self;
        }
        let (width, height) = (self.width, self.height);
        let (w, h) = if orientation >= 5 {
            (height, width)
        } else {
            (width, height)
        };
        let mut out = vec![0u8; self.pixels.len()];
        for y in 0..h {
            for x in 0..w {
                // The source pixel that lands at (x, y).
                let (sx, sy) = match orientation {
                    2 => (width - 1 - x, y),              // mirrored
                    3 => (width - 1 - x, height - 1 - y), // upside down
                    4 => (x, height - 1 - y),             // flipped
                    5 => (y, x),                          // transposed
                    6 => (y, height - 1 - x),             // 90° clockwise
                    7 => (width - 1 - y, height - 1 - x), // transverse
                    _ => (width - 1 - y, x),              // 8: 90° counterclockwise
                };
                let s = (sy * width + sx) * 3;
                let d = (y * w + x) * 3;
                out[d..d + 3].copy_from_slice(&self.pixels[s..s + 3]);
            }
        }
        RgbImage {
            width: w,
            height: h,
            pixels: out,
        }
    }

    /// From 32-bit BGRA with premultiplied alpha (what WIC and PDFium hand over), composited on white: a transparent
    /// PNG's empty areas are white, as they're shown, not black.
    pub fn from_premultiplied_bgra(
        width: usize,
        height: usize,
        stride: usize,
        bgra: &[u8],
    ) -> RgbImage {
        let mut pixels = Vec::with_capacity(width * height * 3);
        for y in 0..height {
            let row = &bgra[y * stride..y * stride + width * 4];
            for p in row.chunks_exact(4) {
                let white = 255 - p[3] as u16;
                pixels.push((p[2] as u16 + white).min(255) as u8);
                pixels.push((p[1] as u16 + white).min(255) as u8);
                pixels.push((p[0] as u16 + white).min(255) as u8);
            }
        }
        RgbImage {
            width,
            height,
            pixels,
        }
    }

    /// From straight (not premultiplied) RGBA, composited on white.
    pub fn from_rgba_on_white(width: usize, height: usize, rgba: &[u8]) -> RgbImage {
        let mut pixels = Vec::with_capacity(width * height * 3);
        for p in rgba.chunks_exact(4) {
            let alpha = p[3] as u32;
            for &c in &p[..3] {
                pixels.push(((c as u32 * alpha + 255 * (255 - alpha) + 127) / 255) as u8);
            }
        }
        RgbImage {
            width,
            height,
            pixels,
        }
    }
}

/// The size that fits `width` × `height` into `max_pixel` on its long side (unchanged when it already fits).
pub fn fit(width: usize, height: usize, max_pixel: usize) -> (usize, usize) {
    let longest = width.max(height);
    if longest <= max_pixel || longest == 0 {
        return (width, height);
    }
    let scale = max_pixel as f64 / longest as f64;
    (
        ((width as f64 * scale).round() as usize).max(1),
        ((height as f64 * scale).round() as usize).max(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3 × 2 image whose pixels are numbered 0…5 (in the red channel).
    fn numbered() -> RgbImage {
        RgbImage::new(3, 2, (0..6u8).flat_map(|n| [n, 0, 0]).collect())
    }

    fn reds(image: &RgbImage) -> Vec<u8> {
        image.pixels.chunks(3).map(|p| p[0]).collect()
    }

    #[test]
    fn orientations() {
        // 0 1 2
        // 3 4 5
        assert_eq!(reds(&numbered().oriented(1)), [0, 1, 2, 3, 4, 5]);
        assert_eq!(reds(&numbered().oriented(3)), [5, 4, 3, 2, 1, 0]);
        let clockwise = numbered().oriented(6); // 90° clockwise: 3 0 / 4 1 / 5 2
        assert_eq!((clockwise.width, clockwise.height), (2, 3));
        assert_eq!(reds(&clockwise), [3, 0, 4, 1, 5, 2]);
        let counter = numbered().oriented(8); // 2 5 / 1 4 / 0 3
        assert_eq!(reds(&counter), [2, 5, 1, 4, 0, 3]);
    }

    #[test]
    fn transparency_becomes_white() {
        let rgb = RgbImage::from_premultiplied_bgra(2, 1, 8, &[0, 0, 0, 0, 0, 0, 255, 255]);
        assert_eq!(rgb.pixels, [255, 255, 255, 255, 0, 0]);
        assert_eq!(fit(4000, 3000, 1024), (1024, 768));
        assert_eq!(fit(800, 600, 1024), (800, 600));
    }
}
