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

    /// The page as a picture fitted into a square of `pixels`, on white, annotations drawn.
    pub fn render(&self, pixels: usize) -> Option<RgbImage> {
        let (width, height) = unsafe {
            (
                FPDF_GetPageWidthF(self.handle) as f64,
                FPDF_GetPageHeightF(self.handle) as f64,
            )
        };
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        let scale = pixels as f64 / width.max(height);
        let (w, h) = (
            ((width * scale).round() as usize).max(1),
            ((height * scale).round() as usize).max(1),
        );
        unsafe {
            let bitmap = FPDFBitmap_Create(w as c_int, h as c_int, 0);
            if bitmap.is_null() {
                return None;
            }
            FPDFBitmap_FillRect(bitmap, 0, 0, w as c_int, h as c_int, 0xFFFF_FFFF);
            FPDF_RenderPageBitmap(
                bitmap,
                self.handle,
                0,
                0,
                w as c_int,
                h as c_int,
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
