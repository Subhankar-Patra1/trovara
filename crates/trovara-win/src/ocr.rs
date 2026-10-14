//! Text recognition with Windows.Media.Ocr (TRD §6.2): free, built in, ~25 languages (each one Windows has its OCR
//! pack for; a display language brings its own). For keyword search: the words in screenshots and scanned pages,
//! matched exactly (error codes, receipt numbers, names). Meaning comes from the picture's own vector.
//!
//! Windows' OCR doesn't detect the language: an engine reads one language's script. The user-profile engine (the first
//! of the Windows display languages with OCR) reads screenshots; an Arabic engine, when installed, re-reads PDF pages
//! whose Arabic text layer lost letters.

use std::sync::OnceLock;

use trovara_core::image::RgbImage;
use windows::core::HSTRING;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

/// Which engine reads a picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    /// The user's own languages (Windows' display languages that have OCR).
    UserProfile,
    Arabic,
}

/// The languages this PC can read, as BCP-47 tags ("en-US", "ar-SA").
pub fn languages() -> Vec<String> {
    init();
    OcrEngine::AvailableRecognizerLanguages()
        .map(|languages| {
            languages
                .into_iter()
                .filter_map(|l| l.LanguageTag().ok())
                .map(|tag| tag.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether OCR works here at all (at least one language installed).
pub fn is_available() -> bool {
    engine(Script::UserProfile).is_some()
}

pub fn reads_arabic() -> bool {
    engine(Script::Arabic).is_some()
}

fn init() {
    // Already initialized on this thread (in any mode) is fine.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
}

/// The engine for `script`, made once per process (None when that language isn't installed). OcrEngine is agile:
/// any thread may use it.
fn engine(script: Script) -> Option<&'static OcrEngine> {
    static USER: OnceLock<Option<OcrEngine>> = OnceLock::new();
    static ARABIC: OnceLock<Option<OcrEngine>> = OnceLock::new();
    init();
    match script {
        Script::UserProfile => USER
            .get_or_init(|| OcrEngine::TryCreateFromUserProfileLanguages().ok())
            .as_ref(),
        Script::Arabic => ARABIC
            .get_or_init(|| {
                let tag = languages().into_iter().find(|tag| tag.starts_with("ar"))?;
                let language = Language::CreateLanguage(&HSTRING::from(tag)).ok()?;
                OcrEngine::TryCreateFromLanguage(&language).ok()
            })
            .as_ref(),
    }
}

/// The biggest side Windows' OCR takes; bigger pictures are scaled down first.
fn max_dimension() -> usize {
    OcrEngine::MaxImageDimension()
        .map(|d| d as usize)
        .unwrap_or(2600)
}

/// The text in `image`, a line per line read ("" when nothing is read, or OCR isn't installed).
pub fn text(image: &RgbImage, script: Script) -> String {
    let Some(engine) = engine(script) else {
        return String::new();
    };
    let resized;
    let image = if image.width.max(image.height) > max_dimension() {
        resized = shrink_image(image, max_dimension());
        &resized
    } else {
        image
    };
    recognize(engine, image).unwrap_or_default()
}

/// A word OCR read, and where: pixels of the picture it was given (x, y, width, height).
#[derive(Clone, Debug)]
pub struct Word {
    pub text: String,
    pub rect: (f32, f32, f32, f32),
}

/// The words in `image` with their places, for marking found words on a screenshot or a scanned page (PRD
/// F-PRE-10). Empty when nothing is read or OCR isn't installed.
pub fn words(image: &RgbImage, script: Script) -> Vec<Word> {
    let Some(engine) = engine(script) else {
        return Vec::new();
    };
    let (resized, scale);
    let input = if image.width.max(image.height) > max_dimension() {
        resized = shrink_image(image, max_dimension());
        scale = image.width as f32 / resized.width as f32;
        &resized
    } else {
        scale = 1.0;
        image
    };
    recognize_words(engine, input)
        .unwrap_or_default()
        .into_iter()
        .map(|word| {
            let (x, y, w, h) = word.rect;
            Word { text: word.text, rect: (x * scale, y * scale, w * scale, h * scale) }
        })
        .collect()
}

fn bitmap(image: &RgbImage) -> windows::core::Result<SoftwareBitmap> {
    // BGRA8, opaque.
    let mut bgra = Vec::with_capacity(image.width * image.height * 4);
    for p in image.pixels.chunks_exact(3) {
        bgra.extend_from_slice(&[p[2], p[1], p[0], 255]);
    }
    let buffer = CryptographicBuffer::CreateFromByteArray(&bgra)?;
    let bitmap = SoftwareBitmap::CreateWithAlpha(
        BitmapPixelFormat::Bgra8,
        image.width as i32,
        image.height as i32,
        BitmapAlphaMode::Premultiplied,
    )?;
    bitmap.CopyFromBuffer(&buffer)?;
    Ok(bitmap)
}

/// One recognition at a time per process: two at once on the same engine (the preview renderer's two threads) can
/// fail, and a failure reads as "no words".
static RECOGNIZING: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn recognize_words(engine: &OcrEngine, image: &RgbImage) -> windows::core::Result<Vec<Word>> {
    let bitmap = bitmap(image)?;
    let _turn = RECOGNIZING.lock().unwrap_or_else(|p| p.into_inner());
    let result = engine.RecognizeAsync(&bitmap)?.join()?;
    let mut words = Vec::new();
    for line in result.Lines()? {
        for word in line.Words()? {
            let rect = word.BoundingRect()?;
            words.push(Word { text: word.Text()?.to_string(), rect: (rect.X, rect.Y, rect.Width, rect.Height) });
        }
    }
    Ok(words)
}

fn recognize(engine: &OcrEngine, image: &RgbImage) -> windows::core::Result<String> {
    let bitmap = bitmap(image)?;
    let _turn = RECOGNIZING.lock().unwrap_or_else(|p| p.into_inner());
    let result = engine.RecognizeAsync(&bitmap)?.join()?;
    let mut lines = Vec::new();
    for line in result.Lines()? {
        let text = line.Text()?.to_string();
        if !text.trim().is_empty() {
            lines.push(text);
        }
    }
    Ok(lines.join("\n"))
}

/// A box-filtered downscale to fit `max_side` (OCR's limit, and video frames the reader didn't scale).
pub fn shrink_image(image: &RgbImage, max_side: usize) -> RgbImage {
    let (w, h) = trovara_core::image::fit(image.width, image.height, max_side);
    let mut out = vec![0u8; w * h * 3];
    for y in 0..h {
        let (y0, y1) = (
            y * image.height / h,
            ((y + 1) * image.height / h).max(y * image.height / h + 1),
        );
        for x in 0..w {
            let (x0, x1) = (
                x * image.width / w,
                ((x + 1) * image.width / w).max(x * image.width / w + 1),
            );
            let mut sum = [0u32; 3];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let p = (sy * image.width + sx) * 3;
                    for (total, &value) in sum.iter_mut().zip(&image.pixels[p..p + 3]) {
                        *total += value as u32;
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let o = (y * w + x) * 3;
            for (pixel, total) in out[o..o + 3].iter_mut().zip(sum) {
                *pixel = (total / n) as u8;
            }
        }
    }
    RgbImage::new(w, h, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_screenshot() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/ocr/payment-error.png");
        let image = crate::wic::decode(&path, 4000, 1).unwrap().image;
        println!(
            "OCR languages: {:?} (Arabic: {})",
            languages(),
            reads_arabic()
        );
        if !is_available() {
            eprintln!("SKIPPED: no OCR language is installed");
            return;
        }
        let started = std::time::Instant::now();
        let text = text(&image, Script::UserProfile);
        println!(
            "{:.0} ms:\n{text}",
            started.elapsed().as_secs_f64() * 1000.0
        );
        // Windows' OCR reads "_" as a space. Keyword search splits "ERR_CARD_DECLINED_4471" into the same four words,
        // so a search for the code still finds it.
        for expected in [
            "Payment failed",
            "declined",
            "ERR CARD DECLINED 4471",
            "K7Q2LM",
            "48.90",
            "Try again",
        ] {
            assert!(
                text.contains(expected),
                "OCR missed {expected:?} in:\n{text}"
            );
        }
    }

    #[test]
    fn huge_pictures_shrink_to_fit() {
        let big = RgbImage::new(4, 2, vec![10; 24]);
        let small = shrink_image(&big, 2);
        assert_eq!((small.width, small.height), (2, 1));
        assert!(small.pixels.iter().all(|&p| p == 10));
    }
}

#[cfg(test)]
mod word_tests {
    /// What OCR reads, word by word, on the picture in TROVARA_OCR_PROBE (a look, not a check).
    #[test]
    #[ignore]
    fn words_on_a_picture() {
        let path = std::env::var("TROVARA_OCR_PROBE").expect("TROVARA_OCR_PROBE");
        let decoded = crate::wic::decode(std::path::Path::new(&path), 4000, 1).unwrap();
        for word in super::words(&decoded.image, super::Script::UserProfile) {
            println!("{:?} {:?}", word.text, word.rect);
        }
    }
}
