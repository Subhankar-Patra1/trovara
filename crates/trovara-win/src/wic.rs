//! Pictures through WIC, the Windows Imaging Component (TRD §6.1): JPEG, PNG, GIF, BMP, TIFF, ICO and JPEG XR always;
//! WebP, HEIC/HEIF, AVIF and RAW when Windows has their codecs (the WebP, HEIF + HEVC, AV1 and Raw Image extensions,
//! most preinstalled on Windows 11). Nothing to ship, and the codecs are the ones Photos uses.
//!
//! Big pictures are scaled down while decoding (WIC's high-quality cubic), so a 50-megapixel RAW never sits in memory
//! whole as RGB.

use std::path::Path;

use trovara_core::image::RgbImage;
use windows::core::{Interface, HSTRING};
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

/// A decoded picture: scaled to fit `max_pixel`, and its size as stored.
pub struct Decoded {
    pub image: RgbImage,
    pub width: usize,
    pub height: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WicError {
    /// No codec on this PC reads the format; the message says which extension does.
    NoCodec(String),
    TooSmall(usize, usize),
    Failed(String),
}

impl std::fmt::Display for WicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WicError::NoCodec(message) | WicError::Failed(message) => f.write_str(message),
            WicError::TooSmall(width, height) => write!(f, "small image {width}×{height}"),
        }
    }
}

const WINCODEC_ERR_COMPONENTNOTFOUND: i32 = 0x88982F50u32 as i32;
const WINCODEC_ERR_UNKNOWNIMAGEFORMAT: i32 = 0x88982F07u32 as i32;
/// MF_E_TRANSFORM_NOT_FOUND: the HEIF container opened, but no HEVC (or AV1) decoder is installed to read its picture.
const MF_E_TRANSFORM_NOT_FOUND: i32 = 0xC00D5212u32 as i32;

/// What to install when no codec reads `ext`.
fn missing_codec(ext: &str) -> String {
    match ext {
        "heic" | "heif" => "HEIC pictures need Microsoft's free \"HEIF Image Extensions\" and \"HEVC Video Extensions\" (Microsoft Store)".into(),
        "avif" => "AVIF pictures need Microsoft's free \"AV1 Video Extension\" (Microsoft Store)".into(),
        "webp" => "WebP pictures need Microsoft's free \"Webp Image Extensions\" (Microsoft Store)".into(),
        _ if trovara_core::rules::RAW_EXTENSIONS.contains(&ext) => {
            format!("RAW pictures ({}) need Microsoft's free \"Raw Image Extension\" (Microsoft Store)", ext.to_uppercase())
        }
        _ => format!("Windows has no decoder for .{ext} pictures"),
    }
}

fn factory() -> windows::core::Result<IWICImagingFactory> {
    unsafe {
        // Already initialized on this thread (in any mode) is fine.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
    }
}

/// The picture's size as stored, without decoding it (None when WIC can't read it).
pub fn size(path: &Path) -> Option<(usize, usize)> {
    let factory = factory().ok()?;
    unsafe {
        let decoder = factory
            .CreateDecoderFromFilename(
                &HSTRING::from(path.as_os_str()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let (mut width, mut height) = (0u32, 0u32);
        frame.GetSize(&mut width, &mut height).ok()?;
        Some((width as usize, height as usize))
    }
}

/// Decodes the picture's first frame (an icon's largest), scaled so its long side is at most `max_pixel`, composited
/// on white. `min_side`: a picture whose long side is smaller is too small to show anything (`TooSmall`).
pub fn decode(path: &Path, max_pixel: usize, min_side: usize) -> Result<Decoded, WicError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let failed = |error: windows::core::Error| {
        let code = error.code().0;
        if code == WINCODEC_ERR_COMPONENTNOTFOUND
            || code == WINCODEC_ERR_UNKNOWNIMAGEFORMAT
            || code == MF_E_TRANSFORM_NOT_FOUND
        {
            WicError::NoCodec(missing_codec(&ext))
        } else {
            WicError::Failed(format!("can't decode image ({})", error.message()))
        }
    };
    let factory = factory().map_err(failed)?;
    unsafe {
        let decoder = factory
            .CreateDecoderFromFilename(
                &HSTRING::from(path.as_os_str()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .map_err(failed)?;
        // An icon holds several sizes: take the largest. Everything else: the first frame (a GIF's first picture).
        let frames = decoder.GetFrameCount().map_err(failed)?;
        let mut best = (0u32, 0u32, 0u32);
        let count = if ext == "ico" { frames } else { 1 };
        for index in 0..count {
            let frame = decoder.GetFrame(index).map_err(failed)?;
            let (mut w, mut h) = (0u32, 0u32);
            frame.GetSize(&mut w, &mut h).map_err(failed)?;
            if w as u64 * h as u64 > best.1 as u64 * best.2 as u64 {
                best = (index, w, h);
            }
        }
        let (index, width, height) = best;
        if (width.max(height) as usize) < min_side {
            return Err(WicError::TooSmall(width as usize, height as usize));
        }
        let frame = decoder.GetFrame(index).map_err(failed)?;
        let mut source: IWICBitmapSource = frame.cast().map_err(failed)?;
        let (w, h) = trovara_core::image::fit(width as usize, height as usize, max_pixel);
        if (w, h) != (width as usize, height as usize) {
            let scaler = factory.CreateBitmapScaler().map_err(failed)?;
            scaler
                .Initialize(
                    &source,
                    w as u32,
                    h as u32,
                    WICBitmapInterpolationModeHighQualityCubic,
                )
                .map_err(failed)?;
            source = scaler.cast().map_err(failed)?;
        }
        let converter = factory.CreateFormatConverter().map_err(failed)?;
        converter
            .Initialize(
                &source,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(failed)?;
        let stride = w * 4;
        let mut bgra = vec![0u8; stride * h];
        converter
            .CopyPixels(std::ptr::null(), stride as u32, &mut bgra)
            .map_err(failed)?;
        Ok(Decoded {
            image: RgbImage::from_premultiplied_bgra(w, h, stride, &bgra),
            width: width as usize,
            height: height as usize,
        })
    }
}

/// Writes `image` as a JPEG (quality 0–1) through WIC's encoder: previews and thumbnails for the app.
pub fn save_jpeg(image: &RgbImage, path: &Path, quality: f32) -> Result<(), WicError> {
    use windows::Win32::System::Com::StructuredStorage::{IPropertyBag2, PROPBAG2};
    use windows::Win32::System::Variant::VARIANT;
    let failed = |error: windows::core::Error| WicError::Failed(format!("can't write JPEG ({})", error.message()));
    let factory = factory().map_err(failed)?;
    unsafe {
        let stream = factory.CreateStream().map_err(failed)?;
        stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), windows::Win32::Foundation::GENERIC_WRITE.0).map_err(failed)?;
        let encoder = factory.CreateEncoder(&GUID_ContainerFormatJpeg, std::ptr::null()).map_err(failed)?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache).map_err(failed)?;
        let mut frame = None;
        let mut options: Option<IPropertyBag2> = None;
        encoder.CreateNewFrame(&mut frame, &mut options).map_err(failed)?;
        let frame = frame.ok_or_else(|| WicError::Failed("no JPEG frame".into()))?;
        if let Some(options) = &options {
            let name: Vec<u16> = "ImageQuality".encode_utf16().chain(std::iter::once(0)).collect();
            let bag = PROPBAG2 { pstrName: windows::core::PWSTR(name.as_ptr() as *mut u16), ..Default::default() };
            let value = VARIANT::from(quality.clamp(0.0, 1.0));
            let _ = options.Write(1, &bag, &value);
        }
        frame.Initialize(options.as_ref()).map_err(failed)?;
        frame.SetSize(image.width as u32, image.height as u32).map_err(failed)?;
        let mut format = GUID_WICPixelFormat24bppBGR;
        frame.SetPixelFormat(&mut format).map_err(failed)?;
        // WIC's 24-bit format is BGR.
        let mut bgr = Vec::with_capacity(image.pixels.len());
        for p in image.pixels.chunks_exact(3) {
            bgr.extend_from_slice(&[p[2], p[1], p[0]]);
        }
        frame.WritePixels(image.height as u32, (image.width * 3) as u32, &bgr).map_err(failed)?;
        frame.Commit().map_err(failed)?;
        encoder.Commit().map_err(failed)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/reference")
            .join(name)
    }

    #[test]
    fn decodes_and_scales() {
        let photo = decode(&fixture("photo.jpg"), 512, 256).unwrap();
        assert_eq!((photo.width, photo.height), (1024, 768));
        assert_eq!((photo.image.width, photo.image.height), (512, 384));
        // The sky in the top left is blue.
        let p = &photo.image.pixels[..3];
        assert!(p[2] > p[0] && p[2] > 150, "{p:?}");
        let screen = decode(&fixture("screen.png"), 1536, 256).unwrap();
        assert_eq!((screen.image.width, screen.image.height), (1440, 900));
        assert_eq!(size(&fixture("screen.png")), Some((1440, 900)));
        assert!(matches!(
            decode(&fixture("photo.jpg"), 512, 2000),
            Err(WicError::TooSmall(1024, 768))
        ));
        assert!(matches!(
            decode(&fixture("vectors.json"), 512, 1),
            Err(WicError::NoCodec(_))
        ));
    }
}
