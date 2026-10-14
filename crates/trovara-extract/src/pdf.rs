//! PDFs through PDFium, Chromium's PDF library (prebuilt: vendor/pdfium, TRD §6.3): each page's text, or a rendering
//! of the page when it has too little text (scans, slides) or its text came out in glyph order.
//!
//! PDFium is one process-wide library and not thread-safe: every use goes through `with_document`, under `LOCK`. It
//! reads through a callback (any file name opens, in any language; a big file is never read whole).

#![allow(non_camel_case_types, non_snake_case)]

use std::ffi::{c_char, c_int, c_uchar, c_ulong, c_void, CString};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Mutex, Once};

use trovara_core::image::RgbImage;
use trovara_core::search_text::PageTextProblem;

use crate::{ExtractError, Result};

type FPDF_DOCUMENT = *mut c_void;
type FPDF_PAGE = *mut c_void;
type FPDF_TEXTPAGE = *mut c_void;
type FPDF_BITMAP = *mut c_void;

#[repr(C)]
struct FPDF_FILEACCESS {
    m_FileLen: c_ulong,
    m_GetBlock: Option<
        unsafe extern "C" fn(
            param: *mut c_void,
            position: c_ulong,
            buf: *mut c_uchar,
            size: c_ulong,
        ) -> c_int,
    >,
    m_Param: *mut c_void,
}

const FPDF_ERR_PASSWORD: c_ulong = 4;
const FPDF_ANNOT: c_int = 0x01;

#[link(name = "pdfium.dll", kind = "dylib")]
extern "system" {
    fn FPDF_InitLibrary();
    fn FPDF_LoadCustomDocument(
        access: *mut FPDF_FILEACCESS,
        password: *const c_char,
    ) -> FPDF_DOCUMENT;
    fn FPDF_GetLastError() -> c_ulong;
    fn FPDF_CloseDocument(document: FPDF_DOCUMENT);
    fn FPDF_GetPageCount(document: FPDF_DOCUMENT) -> c_int;
    fn FPDF_LoadPage(document: FPDF_DOCUMENT, index: c_int) -> FPDF_PAGE;
    fn FPDF_ClosePage(page: FPDF_PAGE);
    fn FPDF_GetPageWidthF(page: FPDF_PAGE) -> f32;
    fn FPDF_GetPageHeightF(page: FPDF_PAGE) -> f32;
    fn FPDFText_LoadPage(page: FPDF_PAGE) -> FPDF_TEXTPAGE;
    fn FPDFText_ClosePage(text_page: FPDF_TEXTPAGE);
    fn FPDFText_CountChars(text_page: FPDF_TEXTPAGE) -> c_int;
    fn FPDFText_CountRects(text_page: FPDF_TEXTPAGE, start_index: c_int, count: c_int) -> c_int;
    fn FPDFText_GetRect(
        text_page: FPDF_TEXTPAGE,
        rect_index: c_int,
        left: *mut f64,
        top: *mut f64,
        right: *mut f64,
        bottom: *mut f64,
    ) -> c_int;
    fn FPDFText_GetText(
        text_page: FPDF_TEXTPAGE,
        start: c_int,
        count: c_int,
        result: *mut u16,
    ) -> c_int;
    fn FPDFBitmap_Create(width: c_int, height: c_int, alpha: c_int) -> FPDF_BITMAP;
    fn FPDFBitmap_FillRect(
        bitmap: FPDF_BITMAP,
        left: c_int,
        top: c_int,
        width: c_int,
        height: c_int,
        color: c_ulong,
    ) -> c_int;
    fn FPDFBitmap_GetBuffer(bitmap: FPDF_BITMAP) -> *mut c_void;
    fn FPDFBitmap_GetStride(bitmap: FPDF_BITMAP) -> c_int;
    fn FPDFBitmap_Destroy(bitmap: FPDF_BITMAP);
    fn FPDF_RenderPageBitmap(
        bitmap: FPDF_BITMAP,
        page: FPDF_PAGE,
        x: c_int,
        y: c_int,
        w: c_int,
        h: c_int,
        rotate: c_int,
        flags: c_int,
    );
    fn FPDF_GetMetaText(
        document: FPDF_DOCUMENT,
        tag: *const c_char,
        buffer: *mut c_void,
        length: c_ulong,
    ) -> c_ulong;
}

static LOCK: Mutex<()> = Mutex::new(());
static INIT: Once = Once::new();

unsafe extern "C" fn get_block(
    param: *mut c_void,
    position: c_ulong,
    buf: *mut c_uchar,
    size: c_ulong,
) -> c_int {
    let file = &mut *(param as *mut File);
    let out = std::slice::from_raw_parts_mut(buf, size as usize);
    match file
        .seek(SeekFrom::Start(position as u64))
        .and_then(|_| file.read_exact(out))
    {
        Ok(()) => 1,
        Err(_) => 0,
    }
}

/// An open document; closed when dropped (always under `LOCK`, held by `with_document`).
pub struct Document {
    handle: FPDF_DOCUMENT,
}

/// Runs `body` with the document open. `Locked` when it needs a password, `Unreadable` when it won't open.
pub fn with_document<T>(path: &Path, body: impl FnOnce(&Document) -> Result<T>) -> Result<T> {
    let mut file = File::open(path).map_err(|error| ExtractError::Unreadable(error.to_string()))?;
    let length = file
        .metadata()
        .map_err(|error| ExtractError::Unreadable(error.to_string()))?
        .len();
    // PDFium counts in C's unsigned long: 4 GB at most on Windows.
    if length > c_ulong::MAX as u64 {
        return Err(ExtractError::Unreadable("too big".into()));
    }
    let _guard = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    INIT.call_once(|| unsafe { FPDF_InitLibrary() });
    let mut access = FPDF_FILEACCESS {
        m_FileLen: length as c_ulong,
        m_GetBlock: Some(get_block),
        m_Param: &mut file as *mut File as *mut c_void,
    };
    let handle = unsafe { FPDF_LoadCustomDocument(&mut access, std::ptr::null()) };
    if handle.is_null() {
        return Err(if unsafe { FPDF_GetLastError() } == FPDF_ERR_PASSWORD {
            ExtractError::Locked
        } else {
            ExtractError::Unreadable("can't open PDF".into())
        });
    }
    let document = Document { handle };
    let result = body(&document);
    drop(document); // before `file` and the lock go
    result
}

impl Drop for Document {
    fn drop(&mut self) {
        unsafe { FPDF_CloseDocument(self.handle) }
    }
}

impl Document {
    pub fn page_count(&self) -> usize {
        unsafe { FPDF_GetPageCount(self.handle) }.max(0) as usize
    }

    /// A metadata string ("Title", "Author"…), or None when there's none.
    pub fn meta(&self, tag: &str) -> Option<String> {
        let tag = CString::new(tag).ok()?;
        let size = unsafe { FPDF_GetMetaText(self.handle, tag.as_ptr(), std::ptr::null_mut(), 0) };
        if size <= 2 {
            return None;
        }
        let mut buffer = vec![0u16; size as usize / 2];
        unsafe { FPDF_GetMetaText(self.handle, tag.as_ptr(), buffer.as_mut_ptr().cast(), size) };
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        let text = String::from_utf16_lossy(&buffer[..end]).trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    pub fn page(&self, index: usize) -> Option<Page<'_>> {
        let handle = unsafe { FPDF_LoadPage(self.handle, index as c_int) };
        (!handle.is_null()).then_some(Page {
            handle,
            _document: self,
        })
    }
}

pub struct Page<'a> {
    handle: FPDF_PAGE,
    _document: &'a Document,
}

impl Drop for Page<'_> {
    fn drop(&mut self) {
        unsafe { FPDF_ClosePage(self.handle) }
    }
}

impl Page<'_> {
    /// The page's text in reading order, lines ending in "\n".
    pub fn text(&self) -> String {
        unsafe {
            let text_page = FPDFText_LoadPage(self.handle);
            if text_page.is_null() {
                return String::new();
            }
            let count = FPDFText_CountChars(text_page);
            let mut out = String::new();
            if count > 0 {
                let mut buffer = vec![0u16; count as usize + 1];
                let written = FPDFText_GetText(text_page, 0, count, buffer.as_mut_ptr());
                if written > 1 {
                    out = String::from_utf16_lossy(&buffer[..written as usize - 1])
                        .replace("\r\n", "\n")
                        .replace('\r', "\n");
                }
            }
            FPDFText_ClosePage(text_page);
            out
        }
    }

    /// Where `words` are on the page (found as keyword search finds them: `search_text::mark_ranges`), as rectangles
    /// in page points from the top-left (x0, y0, x1, y1), in reading order.
    ///
    /// `forms` adds, for the page's text, the other forms keyword search matches ("months" for "month").
    pub fn word_rects(&self, words: &[String], forms: &dyn Fn(&str) -> Vec<String>) -> Vec<(f64, f64, f64, f64)> {
        let height = unsafe { FPDF_GetPageHeightF(self.handle) } as f64;
        let mut rects = Vec::new();
        unsafe {
            let text_page = FPDFText_LoadPage(self.handle);
            if text_page.is_null() {
                return rects;
            }
            let count = FPDFText_CountChars(text_page);
            if count > 0 {
                let mut units = vec![0u16; count as usize + 1];
                let written = FPDFText_GetText(text_page, 0, count, units.as_mut_ptr()).max(1) as usize - 1;
                // One char per PDFium character index (a lone surrogate becomes U+FFFD), so a match's char offsets
                // are PDFium's indexes.
                let chars: Vec<char> = units[..written]
                    .iter()
                    .map(|&u| char::from_u32(u as u32).unwrap_or('\u{FFFD}'))
                    .collect();
                let text: String = chars.iter().collect();
                let char_at: Vec<usize> = text.char_indices().map(|(at, _)| at).collect();
                let index_of = |byte: usize| char_at.partition_point(|&at| at < byte);
                let mut all: Vec<String> = words.to_vec();
                all.extend(forms(&text));
                for range in trovara_core::search_text::mark_ranges(&all, &text).into_iter().take(400) {
                    let (start, end) = (index_of(range.start), index_of(range.end));
                    let n = FPDFText_CountRects(text_page, start as c_int, (end - start) as c_int);
                    for i in 0..n.max(0) {
                        let (mut left, mut top, mut right, mut bottom) = (0f64, 0f64, 0f64, 0f64);
                        if FPDFText_GetRect(text_page, i, &mut left, &mut top, &mut right, &mut bottom) != 0 {
                            rects.push((left, height - top, right, height - bottom));
                        }
                    }
                }
            }
            FPDFText_ClosePage(text_page);
        }
        rects
    }

    /// Where the page's text runs across it, (left, right) in points: its column, from the rectangles of all its lines.
    pub fn text_columns(&self) -> Option<(f64, f64)> {
        unsafe {
            let text_page = FPDFText_LoadPage(self.handle);
            if text_page.is_null() {
                return None;
            }
            let count = FPDFText_CountChars(text_page);
            let mut extent: Option<(f64, f64)> = None;
            let n = if count > 0 { FPDFText_CountRects(text_page, 0, count) } else { 0 };
            for i in 0..n.clamp(0, 4000) {
                let (mut left, mut top, mut right, mut bottom) = (0f64, 0f64, 0f64, 0f64);
                if FPDFText_GetRect(text_page, i, &mut left, &mut top, &mut right, &mut bottom) != 0 && right > left {
                    extent = Some(extent.map_or((left, right), |(l, r)| (l.min(left), r.max(right))));
                }
            }
            FPDFText_ClosePage(text_page);
            extent
        }
    }

    /// The page with `rects` (page points, top-left origin) marked yellow, like a PDF viewer's search (PRD F-PRE-04).
    /// Without rects, the whole page. `aspect` (width / height): the shape of the preview it's shown in. With `zoom`,
    /// the part of the page around the marks in that shape: as wide as the page's text (a viewer's "fit width", so
    /// the marked lines show whole) and high enough for the marks near the first; without, the whole page with its
    /// marks (the full-size viewer).
    pub fn render_marked(&self, pixels: usize, rects: &[(f64, f64, f64, f64)], aspect: f64, zoom: bool) -> Option<RgbImage> {
        let (width, height) = unsafe { (FPDF_GetPageWidthF(self.handle) as f64, FPDF_GetPageHeightF(self.handle) as f64) };
        let Some(&(_, first_top, _, first_bottom)) = rects.first() else { return self.render(pixels) };
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        let (left, top, area_width, area_height) = if zoom {
            // The marks near the first (within a third of the page's height): what the preview shows together.
            let middle = (first_top + first_bottom) / 2.0;
            let near: Vec<&(f64, f64, f64, f64)> =
                rects.iter().filter(|r| ((r.1 + r.3) / 2.0 - middle).abs() < height / 3.0).collect();
            let (x0, y0, x1, y1) = near.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |a, r| {
                (a.0.min(r.0), a.1.min(r.1), a.2.max(r.2), a.3.max(r.3))
            });
            let aspect = aspect.clamp(0.5, 3.0);
            // The text's column with a little margin; the whole page when the text can't be measured.
            let (column_left, column_right) = self.text_columns().unwrap_or((0.0, width));
            let margin = width * 0.025;
            let column_left = (column_left.min(x0) - margin).max(0.0);
            let column_right = (column_right.max(x1) + margin).min(width);
            let mut w = (column_right - column_left).max(width * 0.3).min(width);
            let mut h = w / aspect;
            if h < (y1 - y0) * 1.4 {
                h = ((y1 - y0) * 1.4).min(height);
                w = (h * aspect).min(width);
            }
            h = h.min(height);
            let cy = (y0 + y1) / 2.0;
            // The column from its left edge; when the marks' height made the area narrower, around the marks.
            let mut left = column_left;
            if x1 > left + w {
                left = (x0 + x1) / 2.0 - w / 2.0;
            }
            let left = left.clamp(0.0, (width - w).max(0.0));
            let top = (cy - h * 0.45).clamp(0.0, (height - h).max(0.0));
            (left, top, w, h)
        } else {
            (0.0, 0.0, width, height)
        };
        let mut image = self.render_area(pixels, left, top, area_width, area_height)?;
        let scale = image.width as f64 / area_width;
        let marks: Vec<(f64, f64, f64, f64)> = rects
            .iter()
            .map(|&(x0, y0, x1, y1)| ((x0 - left) * scale, (y0 - top) * scale, (x1 - left) * scale, (y1 - top) * scale))
            .collect();
        image.highlight(&marks);
        Some(image)
    }

    /// The page as a picture fitted into a square of `pixels`, on white, annotations drawn.
    pub fn render(&self, pixels: usize) -> Option<RgbImage> {
        let (width, height) = unsafe { (FPDF_GetPageWidthF(self.handle) as f64, FPDF_GetPageHeightF(self.handle) as f64) };
        self.render_area(pixels, 0.0, 0.0, width, height)
    }

    /// The part of the page from (`left`, `top`) (points from the top-left), `area_width` × `area_height` points,
    /// fitted into a square of `pixels`.
    fn render_area(&self, pixels: usize, left: f64, top: f64, area_width: f64, area_height: f64) -> Option<RgbImage> {
        let (width, height) = unsafe {
            (
                FPDF_GetPageWidthF(self.handle) as f64,
                FPDF_GetPageHeightF(self.handle) as f64,
            )
        };
        if width <= 0.0 || height <= 0.0 || area_width <= 0.0 || area_height <= 0.0 {
            return None;
        }
        let scale = pixels as f64 / area_width.max(area_height);
        let (w, h) = (
            ((area_width * scale).round() as usize).max(1),
            ((area_height * scale).round() as usize).max(1),
        );
        unsafe {
            let bitmap = FPDFBitmap_Create(w as c_int, h as c_int, 0);
            if bitmap.is_null() {
                return None;
            }
            FPDFBitmap_FillRect(bitmap, 0, 0, w as c_int, h as c_int, 0xFFFF_FFFF);
            // The whole page at this scale, shifted so the area starts at the bitmap's top-left.
            FPDF_RenderPageBitmap(
                bitmap,
                self.handle,
                -((left * scale).round() as c_int),
                -((top * scale).round() as c_int),
                ((width * scale).round() as c_int).max(1),
                ((height * scale).round() as c_int).max(1),
                0,
                FPDF_ANNOT,
            );
            let buffer = FPDFBitmap_GetBuffer(bitmap) as *const u8;
            let stride = FPDFBitmap_GetStride(bitmap) as usize;
            let image = (!buffer.is_null()).then(|| {
                let bytes = std::slice::from_raw_parts(buffer, stride * h);
                // BGRx, opaque: the same as premultiplied BGRA with full alpha.
                let mut rgb = Vec::with_capacity(w * h * 3);
                for y in 0..h {
                    for p in bytes[y * stride..y * stride + w * 4].chunks_exact(4) {
                        rgb.extend_from_slice(&[p[2], p[1], p[0]]);
                    }
                }
                RgbImage::new(w, h, rgb)
            });
            FPDFBitmap_Destroy(bitmap);
            image
        }
    }
}

/// One page as the indexer takes it.
#[derive(Clone, Debug)]
pub struct PageContent {
    /// From 1.
    pub number: usize,
    /// May be empty (scans).
    pub text: String,
    /// Set when the page had too little text (or garbled text) and was rendered for the vision encoder.
    pub image: Option<RgbImage>,
    /// Text read in glyph order: too garbled to show or match words against, but enough of its words survive for a
    /// meaning vector beside the page's picture.
    pub garbled: Option<String>,
    /// The text came from OCR (a scan, or a text layer that lost letters).
    pub ocr: bool,
}

/// What was read of a PDF.
#[derive(Debug)]
pub struct Pages {
    pub pages: Vec<PageContent>,
    pub total: usize,
    pub title: Option<String>,
}

/// Text recognition, handed in by the caller (Windows.Media.Ocr in the indexer): this crate doesn't depend on it.
pub trait Ocr {
    /// The words in `image`; `arabic` asks for the Arabic engine.
    fn text(&self, image: &RgbImage, arabic: bool) -> String;
    fn reads_arabic(&self) -> bool;
}

/// Lost Arabic letters are read again by OCR at this size (small diacritics need it).
const ARABIC_OCR_PIXELS: usize = 2000;

/// Reads the pages in `range(total)` (0-based). Pages with real text become text; scans, slides and pages whose text
/// layer came out in glyph order are rendered at `render_pixels`. With `ocr`: a rendered page's words come from OCR
/// (for keyword search: an invoice number on a scan), and an Arabic text layer that lost letters is replaced by OCR's
/// reading when that's real Arabic. Bengali in glyph order isn't OCR'd: Windows' OCR doesn't read Bengali either.
pub fn pages(
    path: &Path,
    range: impl FnOnce(usize) -> std::ops::Range<usize>,
    min_text: usize,
    render_pixels: usize,
    ocr: Option<&dyn Ocr>,
) -> Result<Pages> {
    with_document(path, |document| {
        let total = document.page_count();
        let wanted = range(total);
        let mut pages = Vec::new();
        for index in wanted.start.min(total)..wanted.end.min(total) {
            let Some(page) = document.page(index) else {
                continue;
            };
            let number = index + 1;
            let mut text = page.text().trim().to_string();
            let mut garbled = None;
            match PageTextProblem::check(&text) {
                Some(PageTextProblem::GlyphOrder) => garbled = Some(std::mem::take(&mut text)),
                problem => {
                    if problem == Some(PageTextProblem::LostLetters) {
                        if let Some(ocr) = ocr.filter(|ocr| ocr.reads_arabic()) {
                            if let Some(image) = page.render(ARABIC_OCR_PIXELS) {
                                let read = ocr.text(&image, true).trim().to_string();
                                if read.chars().count() >= min_text
                                    && PageTextProblem::check(&read).is_none()
                                    && read.chars().any(|c| (0x0600..0x0700).contains(&(c as u32)))
                                {
                                    pages.push(PageContent {
                                        number,
                                        text: read,
                                        image: None,
                                        garbled: None,
                                        ocr: true,
                                    });
                                    continue;
                                }
                            }
                        }
                    }
                    if text.chars().count() >= min_text {
                        pages.push(PageContent {
                            number,
                            text,
                            image: None,
                            garbled: None,
                            ocr: false,
                        });
                        continue;
                    }
                }
            }
            let Some(image) = page.render(render_pixels) else {
                continue;
            };
            // A scan, or a page that's mostly a picture: its words come from OCR, for keyword search only, and its
            // meaning stays the picture's vector.
            let mut read_by_ocr = false;
            if let (Some(ocr), None) = (ocr, &garbled) {
                let read = ocr.text(&image, false).trim().to_string();
                if read.chars().count() > text.chars().count() {
                    text = read;
                    read_by_ocr = true;
                }
            }
            pages.push(PageContent {
                number,
                text,
                image: Some(image),
                garbled,
                ocr: read_by_ocr,
            });
        }
        Ok(Pages {
            pages,
            total,
            title: document.meta("Title"),
        })
    })
}

/// How many pages a PDF has (1 when it won't open: estimates only).
pub fn page_count(path: &Path) -> usize {
    with_document(path, |document| Ok(document.page_count())).unwrap_or(1)
}

/// Page `number` (from 1) with `words` marked yellow where they're on it, shown around the first (PRD F-PRE-04); the
/// whole page when none is there. True with it when something was marked (a scan has no text layer to mark from).
#[allow(clippy::too_many_arguments)]
pub fn render_page_marked(
    path: &Path,
    number: usize,
    pixels: usize,
    words: &[String],
    forms: &dyn Fn(&str) -> Vec<String>,
    aspect: f64,
    zoom: bool,
) -> Result<(RgbImage, bool)> {
    with_document(path, |document| {
        let page = document
            .page(number.saturating_sub(1))
            .ok_or_else(|| ExtractError::Unreadable(format!("no page {number}")))?;
        let rects = page.word_rects(words, forms);
        let image = page
            .render_marked(pixels, &rects, aspect, zoom)
            .ok_or_else(|| ExtractError::Unreadable(format!("can't render page {number}")))?;
        Ok((image, !rects.is_empty()))
    })
}

/// Page `number` (from 1), fitted into a square of `pixels`: for previews.
pub fn render_page(path: &Path, number: usize, pixels: usize) -> Result<RgbImage> {
    with_document(path, |document| {
        let page = document
            .page(number.saturating_sub(1))
            .ok_or_else(|| ExtractError::Unreadable(format!("no page {number}")))?;
        page.render(pixels)
            .ok_or_else(|| ExtractError::Unreadable(format!("can't render page {number}")))
    })
}
