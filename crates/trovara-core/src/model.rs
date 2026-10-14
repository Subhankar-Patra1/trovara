//! What the index is made of: kinds of files and segments, the indexing settings, and the model's prompts.
//! (TRD §4.3–4.4, §5.3.)

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// What a file is, for indexing and for grouping results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    Screenshot,
    Image,
    Pdf,
    Doc,
    Audio,
    Video,
    /// Source code, a repo's docs and notebooks: only in the code index.
    Code,
}

impl FileKind {
    pub const ALL: [FileKind; 7] = [
        FileKind::Screenshot,
        FileKind::Image,
        FileKind::Pdf,
        FileKind::Doc,
        FileKind::Audio,
        FileKind::Video,
        FileKind::Code,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            FileKind::Screenshot => "screenshot",
            FileKind::Image => "image",
            FileKind::Pdf => "pdf",
            FileKind::Doc => "doc",
            FileKind::Audio => "audio",
            FileKind::Video => "video",
            FileKind::Code => "code",
        }
    }

    pub fn parse(text: &str) -> Option<FileKind> {
        FileKind::ALL.into_iter().find(|kind| kind.as_str() == text)
    }

    /// The order of the first indexing pass: what people look for most comes first.
    pub fn indexing_rank(self) -> u8 {
        match self {
            FileKind::Screenshot => 0,
            FileKind::Image => 1,
            FileKind::Pdf => 2,
            FileKind::Doc => 3,
            FileKind::Audio => 4,
            FileKind::Video => 5,
            FileKind::Code => 6,
        }
    }

    /// The same order as SQL, for `ORDER BY`.
    pub const INDEXING_ORDER_SQL: &'static str =
        "CASE kind WHEN 'screenshot' THEN 0 WHEN 'image' THEN 1 \
        WHEN 'pdf' THEN 2 WHEN 'doc' THEN 3 WHEN 'audio' THEN 4 WHEN 'video' THEN 5 ELSE 6 END";
}

/// One searchable piece of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentKind {
    /// File name + folder names (keyword search only).
    Name,
    /// The image itself.
    Image,
    /// Text read from a screenshot or a scanned page.
    Ocr,
    /// A PDF page's text.
    Page,
    /// A PDF page rendered as an image (scans, slides).
    PageImage,
    /// A piece of a document's text.
    Chunk,
    /// A window of audio (a file, or a video's soundtrack).
    Audio,
    /// A video keyframe.
    Frame,
    /// A stretch of a code file: `loc` to `loc_end` are its first and last lines.
    Lines,
    /// A stretch of a notebook: `loc` to `loc_end` are its first and last cells, from 1.
    Cells,
}

impl SegmentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SegmentKind::Name => "name",
            SegmentKind::Image => "image",
            SegmentKind::Ocr => "ocr",
            SegmentKind::Page => "page",
            SegmentKind::PageImage => "page_image",
            SegmentKind::Chunk => "chunk",
            SegmentKind::Audio => "audio",
            SegmentKind::Frame => "frame",
            SegmentKind::Lines => "lines",
            SegmentKind::Cells => "cells",
        }
    }
}

/// What produced a segment's vector. Scores are normalized per modality, because text vectors score higher than image
/// or audio vectors for the same topic (measured: 0.75–0.86 vs 0.67–0.78).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modality {
    None,
    Text,
    Image,
    Audio,
}

impl Modality {
    pub fn as_str(self) -> &'static str {
        match self {
            Modality::None => "none",
            Modality::Text => "text",
            Modality::Image => "image",
            Modality::Audio => "audio",
        }
    }
}

/// The vision soft-token budgets the model accepts.
pub const VISION_BUDGETS: [u32; 5] = [70, 140, 280, 560, 1120];

/// Indexing settings. Anything that changes vectors is part of `fingerprint`; an index never mixes settings.
#[derive(Clone, Debug, PartialEq)]
pub struct IndexOptions {
    /// This index is the code index: source files, their repos' docs and notebooks, and nothing else.
    pub code: bool,
    /// Characters a stretch of code holds at most (whole lines; a longer line is a stretch of its own).
    pub code_stretch: usize,
    /// A bigger source file was made by a program: left out.
    pub max_code_bytes: u64,
    /// A notebook's file holds its outputs too: bigger than this, it isn't read.
    pub max_notebook_bytes: u64,
    pub include_code: bool,
    pub include_datasets: bool,
    pub min_image_side: u32,
    /// PDFs up to this many pages are read whole in the first pass.
    pub big_pdf_pages: u32,
    /// Pages per step of a long PDF.
    pub pdf_step: u32,
    /// Chunks per step of a long document (~64,000 characters).
    pub doc_step: u32,
    pub max_pages: u32,
    pub max_chunks: u32,
    /// Fewer characters than this on a PDF page: embed the rendered page instead.
    pub min_page_text: usize,
    pub photo_budget: u32,
    pub screenshot_budget: u32,
    pub page_budget: u32,
    pub frame_budget: u32,
    /// Seconds between sampled video frames (near-duplicates are dropped).
    pub frame_interval: f64,
    /// The model ignores audio past 30 s of one input.
    pub audio_window: f64,
    pub audio_hop: f64,
    pub excluded_folders: Vec<String>,
    pub excluded_extensions: BTreeSet<String>,
}

impl Default for IndexOptions {
    fn default() -> Self {
        IndexOptions {
            code: false,
            code_stretch: 1800,
            max_code_bytes: 1 << 20,
            max_notebook_bytes: 20 << 20,
            include_code: false,
            include_datasets: false,
            min_image_side: 256,
            big_pdf_pages: 50,
            pdf_step: 30,
            doc_step: 40,
            max_pages: 5_000,
            max_chunks: 5_000,
            min_page_text: 200,
            photo_budget: 140,
            screenshot_budget: 280,
            page_budget: 280,
            frame_budget: 70,
            frame_interval: 3.0,
            audio_window: 30.0,
            audio_hop: 25.0,
            excluded_folders: Vec::new(),
            excluded_extensions: BTreeSet::new(),
        }
    }
}

impl IndexOptions {
    /// File extensions as people write them ("HEIC, .txt *.md") → {"heic", "md", "txt"}.
    pub fn extensions(text: &str) -> BTreeSet<String> {
        text.split(|c: char| c == ',' || c.is_whitespace())
            .map(|word| word.trim_start_matches(['*', '.']).to_lowercase())
            .filter(|word| !word.is_empty())
            .collect()
    }

    /// What the index's vectors depend on. A change means re-embedding.
    pub fn fingerprint(&self) -> String {
        if self.code {
            return format!("code1 lines{}", self.code_stretch);
        }
        format!(
            "photo{} shot{} page{} frame{}/{:.1}s audio{:.1}/{:.1}s",
            self.photo_budget,
            self.screenshot_budget,
            self.page_budget,
            self.frame_budget,
            self.frame_interval,
            self.audio_window,
            self.audio_hop
        )
    }
}

/// EmbeddingGemma 2's task prompts (model card). Media inputs take no prompt.
pub struct Prompts;

impl Prompts {
    pub fn query(text: &str) -> String {
        format!("task: search result | query: {text}")
    }

    /// A search of the code index: the model card's prompt for finding code by what it does.
    pub fn code_query(text: &str) -> String {
        format!("task: code retrieval | query: {text}")
    }

    pub fn document(title: Option<&str>, text: &str) -> String {
        let title = title.filter(|title| !title.is_empty()).unwrap_or("none");
        format!("title: {title} | text: {text}")
    }
}

/// "code: retry with backoff": a search of the code index, and nothing else.
pub struct CodeQuery;

impl CodeQuery {
    pub const PREFIX: &'static str = "code:";

    /// What's typed after "code:" ("" for "code:" alone), or None for a search that isn't one of code.
    pub fn text(query: &str) -> Option<&str> {
        let trimmed = query.trim_start();
        let head = trimmed.get(..Self::PREFIX.len())?;
        if !head.eq_ignore_ascii_case(Self::PREFIX) {
            return None;
        }
        Some(trimmed[Self::PREFIX.len()..].trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_match_the_reference_format() {
        let options = IndexOptions::default();
        assert_eq!(
            options.fingerprint(),
            "photo140 shot280 page280 frame70/3.0s audio30.0/25.0s"
        );
        let code = IndexOptions {
            code: true,
            ..IndexOptions::default()
        };
        assert_eq!(code.fingerprint(), "code1 lines1800");
    }

    #[test]
    fn extensions_are_read_as_people_write_them() {
        let set = IndexOptions::extensions("HEIC, .txt *.md");
        assert_eq!(set.into_iter().collect::<Vec<_>>(), ["heic", "md", "txt"]);
    }

    #[test]
    fn prompts() {
        assert_eq!(Prompts::query("cat"), "task: search result | query: cat");
        assert_eq!(
            Prompts::code_query("retry"),
            "task: code retrieval | query: retry"
        );
        assert_eq!(Prompts::document(None, "hi"), "title: none | text: hi");
        assert_eq!(Prompts::document(Some(""), "hi"), "title: none | text: hi");
        assert_eq!(
            Prompts::document(Some("Lease"), "pets"),
            "title: Lease | text: pets"
        );
    }

    #[test]
    fn code_queries() {
        assert_eq!(
            CodeQuery::text("code: retry with backoff"),
            Some("retry with backoff")
        );
        assert_eq!(CodeQuery::text("  CODE:foo "), Some("foo"));
        assert_eq!(CodeQuery::text("code:"), Some(""));
        assert_eq!(CodeQuery::text("cod"), None);
        assert_eq!(CodeQuery::text("a dog"), None);
        assert_eq!(CodeQuery::text("ক"), None);
    }

    #[test]
    fn kinds_round_trip() {
        for kind in FileKind::ALL {
            assert_eq!(FileKind::parse(kind.as_str()), Some(kind));
        }
        assert!(FileKind::Screenshot.indexing_rank() < FileKind::Video.indexing_rank());
    }
}
