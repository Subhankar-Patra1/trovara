//! SVG pictures, drawn with resvg (WIC doesn't draw vector pictures): fitted into `max_pixel`, on white. System fonts
//! load once, the first time an SVG has text.

use std::path::Path;
use std::sync::{Arc, LazyLock};

use resvg::{tiny_skia, usvg};
use trovara_core::image::RgbImage;

use crate::{ExtractError, Result};

static FONTS: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
    let mut fonts = usvg::fontdb::Database::new();
    fonts.load_system_fonts();
    Arc::new(fonts)
});

/// The picture's size as drawn (its viewBox or width/height).
pub fn size(path: &Path) -> Result<(usize, usize)> {
    let tree = parse(path)?;
    Ok((
        tree.size().width().round() as usize,
        tree.size().height().round() as usize,
    ))
}

fn parse(path: &Path) -> Result<usvg::Tree> {
    let data = std::fs::read(path).map_err(|error| ExtractError::Unreadable(error.to_string()))?;
    let options = usvg::Options {
        fontdb: FONTS.clone(),
        resources_dir: path.parent().map(Path::to_path_buf),
        ..usvg::Options::default()
    };
    usvg::Tree::from_data(&data, &options)
        .map_err(|error| ExtractError::Unreadable(format!("can't read SVG: {error}")))
}

/// Drawn so that its long side is `max_pixel` (small icons are drawn bigger: the vision encoder wants detail), on
/// white. `min_side`: a picture whose long side is smaller as written is too small to show anything.
pub fn render(path: &Path, max_pixel: usize, min_side: usize) -> Result<(RgbImage, usize, usize)> {
    let tree = parse(path)?;
    let (width, height) = (tree.size().width(), tree.size().height());
    let (w0, h0) = (width.round() as usize, height.round() as usize);
    if w0.max(h0) < min_side.min(64) || width <= 0.0 || height <= 0.0 {
        return Err(ExtractError::TooSmall(w0, h0));
    }
    let scale = max_pixel as f32 / width.max(height);
    let (w, h) = (
        ((width * scale).round() as u32).max(1),
        ((height * scale).round() as u32).max(1),
    );
    let mut pixmap = tiny_skia::Pixmap::new(w, h)
        .ok_or_else(|| ExtractError::Unreadable("can't draw SVG".into()))?;
    pixmap.fill(tiny_skia::Color::WHITE);
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // tiny-skia keeps premultiplied RGBA; on a white, opaque background every pixel ends up opaque.
    let rgb: Vec<u8> = pixmap
        .data()
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    Ok((RgbImage::new(w as usize, h as usize, rgb), w0, h0))
}
