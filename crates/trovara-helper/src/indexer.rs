//! The indexer (TRD §10.1): keeps the index in line with the chosen folders (`sync`: new, changed, moved, gone), then
//! reads, embeds and stores what's pending (`run`), with the reference's rules for long files: a PDF over 50 pages gets
//! its first 30 pages now and the rest after everything else, 30 at a time (`finish_long_files`); a document its first
//! 40 passages, the same way.
//!
//! Readers: pictures through WIC (+ resvg for SVG) with EXIF and, for screenshots, OCR; PDFs through PDFium (pages as
//! text, or rendered and OCR'd when they're scans or slides); documents (txt, md, html, rtf, docx, odt, doc); audio and
//! video through Media Foundation (30 s sound windows; a frame per shot).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::path::Path;
use std::time::Instant;

use serde::Serialize;
use trovara_core::code::CodeError;
use trovara_core::rules::title;
use trovara_core::search_text::{chunks, excerpt, has_words};
use trovara_core::store::{path_key, Info};
use trovara_core::{
    paths, Candidate, FileKind, FileRecord, IndexOptions, IndexStore, Modality, Prompts,
    SegmentKind,
};
use trovara_extract::{docs, exif, pdf, svg, ExtractError};
use trovara_llama::{EmbedError, Embedder, Models};
use trovara_win::system::file_id;

use crate::crawler;

/// What a sync found and did.
#[derive(Debug, Default, Serialize)]
pub struct SyncSummary {
    pub found: usize,
    pub added: usize,
    pub changed: usize,
    pub moved: usize,
    pub removed: usize,
    pub unchanged: usize,
    /// Files this sync queued (new or changed), so a watcher's new files can go first.
    #[serde(skip)]
    pub queued: Vec<i64>,
    pub skipped: BTreeMap<String, usize>,
    pub skipped_folders: BTreeMap<String, usize>,
    pub datasets: HashMap<String, usize>,
    pub ms: u64,
}

/// One file (or one step of a long file) read.
#[derive(Debug, Serialize)]
pub struct Event {
    pub done: usize,
    pub total: usize,
    pub path: String,
    pub kind: FileKind,
    pub state: &'static str,
    pub seconds: f64,
    pub note: Option<String>,
}

/// Pictures are prepared at this long side before the encoder fills its budget: photos 1024, screenshots 1536 (their
/// text is small).
const PHOTO_PIXELS: usize = 1024;
const SCREENSHOT_PIXELS: usize = 1536;
/// PDF pages are rendered at this long side.
const PAGE_PIXELS: usize = 1400;
/// Video frames are taken at this long side (the encoder's 70-token budget needs no more).
const FRAME_PIXELS: usize = 1024;
/// At most this many frames are sampled from one video (a long film gets one every duration/1200 s).
const MAX_FRAMES: usize = 1200;
/// Screenshots are read by OCR at up to this long side (full resolution for any common screen).
const OCR_PIXELS: usize = 4096;

/// Windows.Media.Ocr, handed to the PDF reader.
struct WindowsOcr;

impl pdf::Ocr for WindowsOcr {
    fn text(&self, image: &trovara_core::image::RgbImage, arabic: bool) -> String {
        let script = if arabic {
            trovara_win::ocr::Script::Arabic
        } else {
            trovara_win::ocr::Script::UserProfile
        };
        trovara_win::ocr::text(image, script)
    }

    fn reads_arabic(&self) -> bool {
        trovara_win::ocr::reads_arabic()
    }
}
/// The text of a page that goes into its vector (the model reads ~2,000 tokens).
const PAGE_TEXT_LIMIT: usize = 4000;

type Failure = String;

/// The outcome of reading (part of) a file: its state and a note for the log.
struct Outcome {
    state: &'static str,
    note: Option<String>,
}

pub struct Indexer {
    pub store: IndexStore,
    options: IndexOptions,
    models: Models,
    embedder: Option<Embedder>,
    /// What the last read cost: (kind of unit, units), for the estimates (`costs.json`).
    cost: Option<(&'static str, f64)>,
    /// Seconds the last read spent starting the model (left out of its cost).
    startup: f64,
    measured: crate::estimator::Measured,
    log: std::path::PathBuf,
}

impl Indexer {
    pub fn new(store: IndexStore, options: IndexOptions) -> Result<Indexer, String> {
        let models = Models::locate().map_err(|e| e.to_string())?;
        let log = paths::logs();
        let _ = std::fs::create_dir_all(&log);
        let store_folder = store
            .file
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        Ok(Indexer {
            store,
            options,
            models,
            embedder: None,
            cost: None,
            startup: 0.0,
            measured: crate::estimator::Measured::load(&store_folder),
            log: log.join("indexer.log"),
        })
    }

    /// The model, started on first use: text-only while only text needs embedding, the full one (with the image and
    /// audio encoders) once anything else does.
    fn embedder(&mut self, full: bool) -> Result<&mut Embedder, Failure> {
        if self.embedder.as_ref().is_some_and(|e| e.text_only && full) {
            self.embedder = None;
        }
        if self.embedder.is_none() {
            // Background mode puts disk reads at "very low" priority: reading the 865 MB of model files took 45 s instead
            // of 3. The load runs at normal priority, then the process goes back to the background.
            let background = crate::service::in_background();
            if background {
                trovara_win::system::leave_background();
            }
            let embedder =
                Embedder::new(&self.models, !full, Some(&self.log)).map_err(|e| e.to_string());
            if background {
                trovara_win::system::enter_background(true);
            }
            let embedder = embedder?;
            self.startup += embedder.load_seconds;
            crate::service::say(&format!(
                "model ready on {} in {} s{}",
                embedder.device,
                embedder.load_seconds,
                if full { "" } else { " (text only)" }
            ));
            self.embedder = Some(embedder);
        }
        Ok(self.embedder.as_mut().unwrap())
    }

    /// Brings the index in line with the chosen folders: new files are queued, changed ones queued again, moved ones
    /// re-pathed (same file ID, size and date: nothing is embedded again), and files that are gone or newly left out
    /// are removed. A root that isn't there now (an unplugged drive) keeps its rows.
    pub fn sync(&self, roots: &[String]) -> Result<SyncSummary, String> {
        let started = Instant::now();
        let crawl = crawler::crawl(roots, &self.options);
        let existing = self.store.files_under(roots).map_err(|e| e.to_string())?;
        // A root that went away while it was walked (a drive ejected mid-crawl) looked emptier than it is: keep its
        // rows rather than take that for "deleted".
        let vanished: Vec<String> = roots
            .iter()
            .filter(|root| !Path::new(root).is_dir())
            .map(|root| format!("{}\\", path_key(root)))
            .collect();
        self.reconcile(crawl, &existing, &vanished, started)
    }

    /// Like `sync`, for some folders only (the ones the change journal says changed while Trovara wasn't running):
    /// each folder's own files, and everything in a folder the index has no files under (new, or renamed). Files in a
    /// subfolder that's gone leave with it.
    pub fn sync_folders(
        &self,
        roots: &[String],
        folders: &[String],
    ) -> Result<SyncSummary, String> {
        let started = Instant::now();
        let mut plan: Vec<(String, bool)> = Vec::new();
        let mut existing: Vec<FileRecord> = Vec::new();
        let mut taken: HashSet<i64> = HashSet::new();
        let mut planned: HashSet<String> = HashSet::new();
        for folder in folders {
            let key = path_key(folder);
            if !planned.insert(key.clone()) {
                continue;
            }
            let under = self
                .store
                .files_under(std::slice::from_ref(folder))
                .map_err(|e| e.to_string())?;
            let whole = under.is_empty();
            plan.push((folder.clone(), whole));
            for row in under {
                let row_key = path_key(&row.path);
                let parent = row_key
                    .rsplit_once('\\')
                    .map(|(p, _)| p.to_string())
                    .unwrap_or_default();
                let direct = parent == key;
                // In a subfolder that's gone: its files leave with it.
                let gone = !direct && {
                    let rest = &row.path[folder.len().min(row.path.len())..];
                    let first = rest
                        .trim_start_matches(['\\', '/'])
                        .split(['\\', '/'])
                        .next()
                        .unwrap_or_default();
                    !Path::new(folder).join(first).is_dir()
                };
                if (whole || direct || gone) && taken.insert(row.id) {
                    existing.push(row);
                }
            }
        }
        let crawl = crawler::crawl_folders(roots, &plan, &self.options);
        self.reconcile(crawl, &existing, &[], started)
    }

    /// Brings `existing` (the index's rows for what was crawled) in line with `crawl`: new files are queued, changed
    /// ones queued again, moved ones re-pathed (same file ID, size and date, anywhere in the index: nothing is embedded
    /// again), and rows not seen are removed, except under `keep` (prefixes of roots that aren't there now).
    fn reconcile(
        &self,
        crawl: crawler::CrawlReport,
        existing: &[FileRecord],
        keep: &[String],
        started: Instant,
    ) -> Result<SyncSummary, String> {
        let by_path: HashMap<String, &FileRecord> =
            existing.iter().map(|r| (path_key(&r.path), r)).collect();
        let by_id: HashMap<(u64, u64), &FileRecord> = existing
            .iter()
            .filter(|r| r.inode != 0)
            .map(|r| ((r.device, r.inode), r))
            .collect();
        let seen: HashSet<String> = crawl.candidates.iter().map(|c| path_key(&c.path)).collect();
        let mut moved_from: HashSet<i64> = HashSet::new();
        let mut summary = SyncSummary::default();
        let with_id = |candidate: &Candidate| {
            let mut candidate = candidate.clone();
            if let Some((device, inode)) = file_id(Path::new(&candidate.path)) {
                (candidate.device, candidate.inode) = (device, inode);
            }
            candidate
        };
        self.store
            .transaction(|| {
                for file in &crawl.candidates {
                    if let Some(row) = by_path.get(&path_key(&file.path)) {
                        if row.size != file.size
                            || (row.mtime - file.mtime).abs() > 0.001
                            || row.kind != file.kind
                        {
                            self.store.requeue_as(row.id, &with_id(file))?;
                            summary.changed += 1;
                            summary.queued.push(row.id);
                        } else {
                            summary.unchanged += 1;
                        }
                        continue;
                    }
                    let file = with_id(file);
                    // The same file somewhere else: among the rows crawled (its old path not seen), or anywhere in the
                    // index when its old path is gone from the disk (a folder renamed elsewhere).
                    let moved = if file.inode == 0 {
                        None
                    } else if let Some(row) = by_id.get(&(file.device, file.inode)).copied() {
                        (!seen.contains(&path_key(&row.path))).then(|| row.clone())
                    } else {
                        self.store
                            .file_by_id(file.device, file.inode)?
                            .filter(|row| !Path::new(&row.path).exists())
                    }
                    .filter(|row| {
                        !moved_from.contains(&row.id)
                            && row.size == file.size
                            && (row.mtime - file.mtime).abs() < 0.001
                    });
                    if let Some(row) = moved {
                        self.store.move_to(row.id, &file.path)?;
                        moved_from.insert(row.id);
                        summary.moved += 1;
                    } else {
                        summary.queued.push(self.store.insert(&file)?);
                        summary.added += 1;
                    }
                }
                for row in existing {
                    let key = path_key(&row.path);
                    if !seen.contains(&key)
                        && !moved_from.contains(&row.id)
                        && !keep.iter().any(|k| key.starts_with(k.as_str()))
                    {
                        self.store.delete(row.id)?;
                        summary.removed += 1;
                    }
                }
                Ok(())
            })
            .map_err(|e| e.to_string())?;
        summary.found = crawl.candidates.len();
        summary.skipped = crawl.skipped;
        summary.skipped_folders = crawl.skipped_folders;
        summary.datasets = crawl.datasets;
        summary.ms = started.elapsed().as_millis() as u64;
        Ok(summary)
    }

    /// The first pass: pending files, screenshots first, newest first (`only`: just these). `should_stop` is asked
    /// between files (pausing, or a sync that should go first); `on_event` hears about each file.
    pub fn run(
        &mut self,
        only: Option<&HashSet<i64>>,
        should_stop: &mut dyn FnMut() -> bool,
        on_event: &mut dyn FnMut(&Event),
    ) -> Result<BTreeMap<&'static str, usize>, String> {
        let mut pending = self.store.pending(None).map_err(|e| e.to_string())?;
        if let Some(ids) = only {
            pending.retain(|file| ids.contains(&file.id));
        }
        let mut states = BTreeMap::new();
        let total = pending.len();
        for (n, file) in pending.iter().enumerate() {
            if should_stop() {
                break;
            }
            let started = Instant::now();
            crate::watchdog::reading(Some(&file.path));
            let outcome = self.read(file, None);
            crate::watchdog::reading(None);
            let outcome = outcome.map_err(|e| format!("{}: {e}", file.path))?;
            self.tally(started.elapsed().as_secs_f64());
            *states.entry(outcome.state).or_default() += 1;
            on_event(&Event {
                done: n + 1,
                total,
                path: file.path.clone(),
                kind: file.kind,
                state: outcome.state,
                seconds: started.elapsed().as_secs_f64(),
                note: outcome.note,
            });
        }
        self.save_costs();
        Ok(states)
    }

    /// The later pass: the rest of long PDFs and documents, a step at a time, each step one transaction (stopping
    /// between steps loses nothing, and the next pass goes on from there). Runs once nothing is pending.
    pub fn finish_long_files(
        &mut self,
        should_stop: &mut dyn FnMut() -> bool,
        on_event: &mut dyn FnMut(&Event),
    ) -> Result<BTreeMap<&'static str, usize>, String> {
        let unfinished = self.store.unfinished().map_err(|e| e.to_string())?;
        let mut jobs: Vec<(FileRecord, Range<usize>)> = Vec::new();
        for file in unfinished {
            let info = self.store.info(file.id).map_err(|e| e.to_string())?;
            let size = if file.kind == FileKind::Pdf {
                self.options.pdf_step
            } else {
                self.options.doc_step
            } as usize;
            let mut start = match info.get("read_to").and_then(|v| v.parse().ok()) {
                Some(start) => start,
                None => self
                    .store
                    .last_place(file.id)
                    .map_err(|e| e.to_string())?
                    .unwrap_or(0),
            };
            let end = info
                .get("read_end")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0)
                .max(start + 1);
            while start < end {
                jobs.push((file.clone(), start..(start + size).min(end)));
                start += size;
            }
        }
        let total = jobs.len();
        let mut states = BTreeMap::new();
        let mut finished: HashSet<i64> = HashSet::new();
        for (n, (file, step)) in jobs.into_iter().enumerate() {
            if should_stop() {
                break;
            }
            // Finished sooner than its steps said (it went unreadable): its other steps are skipped.
            if finished.contains(&file.id) {
                continue;
            }
            let started = Instant::now();
            crate::watchdog::reading(Some(&file.path));
            let outcome = self.read(&file, Some(step));
            crate::watchdog::reading(None);
            let outcome = outcome.map_err(|e| format!("{}: {e}", file.path))?;
            self.tally(started.elapsed().as_secs_f64());
            if outcome.state != "partial" {
                finished.insert(file.id);
                *states.entry(outcome.state).or_default() += 1;
            }
            on_event(&Event {
                done: n + 1,
                total,
                path: file.path.clone(),
                kind: file.kind,
                state: outcome.state,
                seconds: started.elapsed().as_secs_f64(),
                note: outcome.note,
            });
        }
        self.save_costs();
        Ok(states)
    }

    /// Frees the model (about 1 GB) until the next file needs it.
    pub fn close_model(&mut self) {
        self.embedder = None;
    }

    /// Adds what the last read cost to the measured costs (a model start isn't part of any file's cost).
    fn tally(&mut self, seconds: f64) {
        let startup = std::mem::take(&mut self.startup);
        if let Some((name, units)) = self.cost.take() {
            self.measured.add(name, units, (seconds - startup).max(0.0));
        }
    }

    /// Saves the measured costs beside the index (after a run, so estimates learn this PC's speed).
    pub fn save_costs(&self) {
        if let Some(folder) = self.store.file.parent() {
            let mut saved = crate::estimator::Measured::load(folder);
            for (name, tally) in &self.measured.tallies {
                let before = saved.tallies.get(name).copied().unwrap_or_default();
                if tally.units > before.units || tally.seconds != before.seconds {
                    saved.tallies.insert(name.clone(), *tally);
                }
            }
            saved.save(folder);
        }
    }

    /// This indexer fills the code index.
    pub fn is_code(&self) -> bool {
        self.options.code
    }

    pub fn model_running(&self) -> bool {
        self.embedder.is_some()
    }

    /// Reads one file (or one step of a long one) and stores what it found. A file that can't be read is marked so
    /// and the run goes on; only the model failing stops it (Err).
    fn read(&mut self, file: &FileRecord, step: Option<Range<usize>>) -> Result<Outcome, Failure> {
        let path = Path::new(&file.path);
        let result = match file.kind {
            FileKind::Image | FileKind::Screenshot => self.image(file, path),
            FileKind::Pdf => self.pdf(file, path, step.clone()),
            FileKind::Doc => self.doc(file, path, step.clone()),
            FileKind::Audio => self.audio(file, path),
            FileKind::Video => self.video(file, path),
            FileKind::Code => self.code(file, path),
        };
        match result {
            Ok(outcome) => Ok(outcome),
            Err(ReadError::Model(message)) => Err(message),
            Err(ReadError::File(error)) => {
                let detail = error.to_string();
                if let Some(step) = step {
                    // A long file that can't be read any further keeps what was read so far.
                    let mut info = self.store.info(file.id).map_err(|e| e.to_string())?;
                    info.remove("read_to");
                    info.remove("read_end");
                    let unit = if file.kind == FileKind::Pdf {
                        "page"
                    } else {
                        "chunk"
                    };
                    let note = format!("read to {unit} {}, not after: {detail}", step.start);
                    self.store
                        .finish(file.id, "done", Some(&note), &info)
                        .map_err(|e| e.to_string())?;
                    return Ok(Outcome {
                        state: "done",
                        note: Some(note),
                    });
                }
                let state = error.state();
                self.store
                    .finish(file.id, state, Some(&detail), &Info::new())
                    .map_err(|e| e.to_string())?;
                Ok(Outcome {
                    state,
                    note: Some(detail),
                })
            }
        }
    }

    fn image(&mut self, file: &FileRecord, path: &Path) -> Result<Outcome, ReadError> {
        let screenshot = file.kind == FileKind::Screenshot;
        let max_pixel = if screenshot {
            SCREENSHOT_PIXELS
        } else {
            PHOTO_PIXELS
        };
        let min_side = self.options.min_image_side as usize;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_lowercase();
        let metadata = exif::read(path);
        let (image, width, height) = if ext == "svg" {
            svg::render(path, max_pixel, min_side)?
        } else {
            let decoded =
                trovara_win::wic::decode(path, max_pixel, min_side).map_err(
                    |error| match error {
                        trovara_win::wic::WicError::TooSmall(w, h) => ExtractError::TooSmall(w, h),
                        other => ExtractError::Unreadable(other.to_string()),
                    },
                )?;
            // HEIF and AVIF decoders apply the file's own rotation (irot/imir); for the rest, EXIF says how the camera
            // was held.
            let orientation = if matches!(ext.as_str(), "heic" | "heif" | "avif") {
                1
            } else {
                metadata.as_ref().and_then(|m| m.orientation).unwrap_or(1)
            };
            let (w, h) = if orientation >= 5 {
                (decoded.height, decoded.width)
            } else {
                (decoded.width, decoded.height)
            };
            (decoded.image.oriented(orientation), w, h)
        };
        let budget = if screenshot {
            self.options.screenshot_budget
        } else {
            self.options.photo_budget
        } as usize;
        let vector = match self.embedder(true)?.embed_image_rgb(
            &image.pixels,
            image.width,
            image.height,
            budget,
        ) {
            Ok(vector) => vector,
            Err(EmbedError::Input(why)) => {
                return Err(ReadError::File(ExtractError::Unreadable(why)))
            }
            Err(other) => return Err(ReadError::Model(other.to_string())),
        };
        // The words on screen are for keyword search: codes, names and errors typed exactly. Meaning comes from the
        // picture alone (a vector of the words made screenshots outrank better matches in the reference's evals).
        // Read at full resolution: small interface text is what screenshots are full of.
        let ocr_text = if screenshot && ext != "svg" && trovara_win::ocr::is_available() {
            match trovara_win::wic::decode(path, OCR_PIXELS, 1) {
                Ok(full) => {
                    let orientation = if matches!(ext.as_str(), "heic" | "heif" | "avif") {
                        1
                    } else {
                        metadata.as_ref().and_then(|m| m.orientation).unwrap_or(1)
                    };
                    trovara_win::ocr::text(
                        &full.image.oriented(orientation),
                        trovara_win::ocr::Script::UserProfile,
                    )
                }
                Err(_) => String::new(),
            }
        } else {
            String::new()
        };
        let mut info = Info::new();
        info.insert("width".into(), width.to_string());
        info.insert("height".into(), height.to_string());
        if let Some(taken) = metadata.and_then(|m| m.taken) {
            info.insert("taken".into(), taken);
        }
        if screenshot {
            info.insert("ocr_chars".into(), ocr_text.chars().count().to_string());
        }
        self.store
            .transaction(|| {
                self.store.add_segment(
                    file.id,
                    SegmentKind::Image,
                    Modality::Image,
                    None,
                    None,
                    None,
                    None,
                    Some(&vector),
                )?;
                if !ocr_text.is_empty() {
                    self.store.add_segment(
                        file.id,
                        SegmentKind::Ocr,
                        Modality::Text,
                        None,
                        None,
                        Some(&excerpt(&ocr_text, 240)),
                        Some(&ocr_text),
                        None,
                    )?;
                }
                self.store.finish(file.id, "done", None, &info)
            })
            .map_err(|e| ReadError::Model(e.to_string()))?;
        self.cost = Some((if screenshot { "screenshot" } else { "image" }, 1.0));
        Ok(Outcome {
            state: "done",
            note: screenshot.then(|| format!("ocr {} chars", ocr_text.chars().count())),
        })
    }

    /// Pages `step` of a PDF (all of a short one, the first 30 of a long one in the first pass). A long one stays
    /// `partial` with `read_to` and `read_end` (pages, from 0) in its info until its last step.
    fn pdf(
        &mut self,
        file: &FileRecord,
        path: &Path,
        step: Option<Range<usize>>,
    ) -> Result<Outcome, ReadError> {
        let later = step.is_some();
        let options = self.options.clone();
        let mut wanted = 0..0;
        let read = pdf::pages(
            path,
            |total| {
                let range = step
                    .clone()
                    .unwrap_or(if total > options.big_pdf_pages as usize {
                        0..options.pdf_step as usize
                    } else {
                        0..options.max_pages as usize
                    });
                let limit = total.min(options.max_pages as usize);
                wanted = range.start.min(limit)..range.end.min(limit);
                wanted.clone()
            },
            options.min_page_text,
            PAGE_PIXELS,
            trovara_win::ocr::is_available().then_some(&WindowsOcr as &dyn pdf::Ocr),
        )?;
        let end = read.total.min(options.max_pages as usize);
        let unread = wanted.end < end;
        if read.pages.is_empty() && !later && !unread {
            self.store
                .finish(file.id, "skipped", Some("no pages"), &Info::new())
                .map_err(|e| ReadError::Model(e.to_string()))?;
            return Ok(Outcome {
                state: "skipped",
                note: Some("no pages".into()),
            });
        }
        let title = read.title.clone().unwrap_or_else(|| title(path));
        let clip = |text: &str| text.chars().take(PAGE_TEXT_LIMIT).collect::<String>();
        let text_pages: Vec<&pdf::PageContent> =
            read.pages.iter().filter(|p| p.image.is_none()).collect();
        // A page that's mostly digits (a table, a data listing) gets keyword search only.
        let word_pages: Vec<&pdf::PageContent> = text_pages
            .iter()
            .copied()
            .filter(|p| has_words(&p.text))
            .collect();
        let image_pages: Vec<&pdf::PageContent> =
            read.pages.iter().filter(|p| p.image.is_some()).collect();
        let garbled_pages: Vec<&pdf::PageContent> =
            read.pages.iter().filter(|p| p.garbled.is_some()).collect();
        let mut text_vectors = std::collections::HashMap::new();
        let mut image_vectors = Vec::new();
        let mut garbled_vectors = Vec::new();
        if !word_pages.is_empty() || !image_pages.is_empty() {
            let budget = options.page_budget as usize;
            let embedder = self.embedder(!image_pages.is_empty())?;
            for batch in word_pages.chunks(16) {
                let prompts: Vec<String> = batch
                    .iter()
                    .map(|p| Prompts::document(Some(&title), &clip(&p.text)))
                    .collect();
                let vectors = embedder
                    .embed_texts(&prompts)
                    .map_err(|e| ReadError::Model(e.to_string()))?;
                for (page, vector) in batch.iter().zip(vectors) {
                    text_vectors.insert(page.number, vector);
                }
            }
            for page in &image_pages {
                let image = page.image.as_ref().unwrap();
                image_vectors.push(
                    embedder
                        .embed_image_rgb(&image.pixels, image.width, image.height, budget)
                        .ok(),
                );
            }
            let prompts: Vec<String> = garbled_pages
                .iter()
                .map(|p| Prompts::document(Some(&title), &clip(p.garbled.as_ref().unwrap())))
                .collect();
            if !prompts.is_empty() {
                garbled_vectors = embedder
                    .embed_texts(&prompts)
                    .map_err(|e| ReadError::Model(e.to_string()))?;
            }
        }
        let store = &self.store;
        let state = if unread { "partial" } else { "done" };
        store
            .transaction(|| {
                // Meaning only: garbled words are never shown or matched against what's typed.
                for (page, vector) in garbled_pages.iter().zip(&garbled_vectors) {
                    store.add_segment(
                        file.id,
                        SegmentKind::Page,
                        Modality::Text,
                        Some(page.number as f64),
                        None,
                        None,
                        None,
                        Some(vector),
                    )?;
                }
                for page in &text_pages {
                    store.add_segment(
                        file.id,
                        SegmentKind::Page,
                        Modality::Text,
                        Some(page.number as f64),
                        None,
                        Some(&excerpt(&page.text, 240)),
                        Some(&page.text),
                        text_vectors.get(&page.number).map(Vec::as_slice),
                    )?;
                }
                for (page, vector) in image_pages.iter().zip(&image_vectors) {
                    let excerpt = (!page.text.is_empty()).then(|| excerpt(&page.text, 240));
                    store.add_segment(
                        file.id,
                        SegmentKind::PageImage,
                        Modality::Image,
                        Some(page.number as f64),
                        None,
                        excerpt.as_deref(),
                        Some(&page.text),
                        vector.as_deref(),
                    )?;
                }
                let mut info = if later {
                    store.info(file.id)?
                } else {
                    Info::new()
                };
                info.insert("pages".into(), read.total.to_string());
                let scanned = info
                    .get("scanned_pages")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(0)
                    + image_pages.len();
                info.insert("scanned_pages".into(), scanned.to_string());
                if unread {
                    info.insert("read_to".into(), wanted.end.to_string());
                    info.insert("read_end".into(), end.to_string());
                } else {
                    info.remove("read_to");
                    info.remove("read_end");
                }
                let detail = if unread {
                    Some(format!("read to page {} of {}", wanted.end, read.total))
                } else if end < read.total {
                    Some(format!("the first {end} of {} pages", read.total))
                } else {
                    None
                };
                store.finish(file.id, state, detail.as_deref(), &info)
            })
            .map_err(|e| ReadError::Model(e.to_string()))?;
        let span = if later {
            format!(
                "pages {}–{} of {}",
                wanted.start + 1,
                wanted.end,
                read.total
            )
        } else {
            format!("{}/{} pages", read.pages.len(), read.total)
        };
        self.cost = Some(("pdf_page", read.pages.len() as f64));
        Ok(Outcome {
            state,
            note: Some(format!(
                "{span}, {} as images{}",
                image_pages.len(),
                match read.pages.iter().filter(|p| p.ocr).count() {
                    0 => String::new(),
                    n => format!(", {n} read by OCR"),
                }
            )),
        })
    }

    /// Chunks `step` of a document (all of a short one, the first 40 of a long one), the way `pdf` reads pages.
    fn doc(
        &mut self,
        file: &FileRecord,
        path: &Path,
        step: Option<Range<usize>>,
    ) -> Result<Outcome, ReadError> {
        let later = step.is_some();
        let text = docs::text_of(path)?;
        let max_chunks = self.options.max_chunks as usize;
        // The chunks are cut the same way every time, so a later step goes on where the last one stopped.
        let all = chunks(&text, 1800, 200, max_chunks + 1);
        let total = all.len();
        if total == 0 {
            return Err(ReadError::File(ExtractError::Empty("no text".into())));
        }
        let end = total.min(max_chunks);
        let range = step.unwrap_or(0..self.options.doc_step as usize);
        let read = range.start.min(end)..range.end.min(end);
        let pieces = &all[read.clone()];
        let title = title(path);
        // Text that's mostly digits (a data dump saved as .txt) gets keyword search only.
        let worded: Vec<usize> = (0..pieces.len())
            .filter(|&i| has_words(&pieces[i]))
            .collect();
        let mut vectors: Vec<Option<Vec<f32>>> = vec![None; pieces.len()];
        if !worded.is_empty() {
            let embedder = self.embedder(false)?;
            for batch in worded.chunks(16) {
                let prompts: Vec<String> = batch
                    .iter()
                    .map(|&i| Prompts::document(Some(&title), &pieces[i]))
                    .collect();
                let made = embedder
                    .embed_texts(&prompts)
                    .map_err(|e| ReadError::Model(e.to_string()))?;
                for (&i, vector) in batch.iter().zip(made) {
                    vectors[i] = Some(vector);
                }
            }
        }
        let unread = read.end < end;
        let state = if unread { "partial" } else { "done" };
        let store = &self.store;
        store
            .transaction(|| {
                for (offset, piece) in pieces.iter().enumerate() {
                    store.add_segment(
                        file.id,
                        SegmentKind::Chunk,
                        Modality::Text,
                        Some((read.start + offset + 1) as f64),
                        None,
                        Some(&excerpt(piece, 240)),
                        Some(piece),
                        vectors[offset].as_deref(),
                    )?;
                }
                let mut info = Info::new();
                info.insert("chunks".into(), end.to_string());
                info.insert("chars".into(), text.chars().count().to_string());
                if unread {
                    info.insert("read_to".into(), read.end.to_string());
                    info.insert("read_end".into(), end.to_string());
                }
                let detail = if unread {
                    Some(format!("read to chunk {} of {end}", read.end))
                } else if end < total {
                    Some(format!("the first {end} chunks"))
                } else {
                    None
                };
                store.finish(file.id, state, detail.as_deref(), &info)
            })
            .map_err(|e| ReadError::Model(e.to_string()))?;
        let span = if later {
            format!("chunks {}–{} of {end}", read.start + 1, read.end)
        } else {
            format!("{} chunks", pieces.len())
        };
        let keyword_only = pieces.len() - worded.len();
        self.cost = Some(("doc_chunk", pieces.len() as f64));
        Ok(Outcome {
            state,
            note: Some(if keyword_only > 0 {
                format!("{span}, {keyword_only} for keywords only")
            } else {
                span
            }),
        })
    }
}

impl Indexer {
    /// A code file's stretches of lines (a notebook's of cells), each embedded with the file's path in its repo as its
    /// title. A stretch of digits only (data written as code) is kept for keyword search.
    fn code(&mut self, file: &FileRecord, path: &Path) -> Result<Outcome, ReadError> {
        let code = trovara_core::code::read(path, self.options.code_stretch, self.options.max_code_bytes).map_err(
            |error| match error {
                CodeError::LeftOut(why) => ReadError::File(ExtractError::Empty(why)),
                CodeError::Unreadable(why) => ReadError::File(ExtractError::Unreadable(why)),
            },
        )?;
        if code.pieces.is_empty() {
            return Err(ReadError::File(ExtractError::Empty("no code".into())));
        }
        let worded: Vec<usize> = (0..code.pieces.len()).filter(|&i| has_words(&code.pieces[i].text)).collect();
        let mut vectors: Vec<Option<Vec<f32>>> = vec![None; code.pieces.len()];
        if !worded.is_empty() {
            let embedder = self.embedder(false)?;
            for batch in worded.chunks(16) {
                let prompts: Vec<String> =
                    batch.iter().map(|&i| Prompts::document(Some(&code.title), &code.pieces[i].text)).collect();
                let made = embedder.embed_texts(&prompts).map_err(|e| ReadError::Model(e.to_string()))?;
                for (&i, vector) in batch.iter().zip(made) {
                    vectors[i] = Some(vector);
                }
            }
        }
        let kind = if code.cells.is_some() { SegmentKind::Cells } else { SegmentKind::Lines };
        let store = &self.store;
        store
            .transaction(|| {
                for (offset, piece) in code.pieces.iter().enumerate() {
                    store.add_segment(
                        file.id,
                        kind,
                        Modality::Text,
                        Some(piece.first as f64),
                        Some(piece.last as f64),
                        Some(&trovara_core::code::summary(&piece.text)),
                        Some(&piece.text),
                        vectors[offset].as_deref(),
                    )?;
                }
                let mut info = Info::new();
                info.insert("lines".into(), code.lines.to_string());
                if let Some(cells) = code.cells {
                    info.insert("cells".into(), cells.to_string());
                }
                if let Some(repo) = &code.repo {
                    info.insert("repo".into(), repo.clone());
                }
                info.insert("language".into(), trovara_core::code::language(path));
                store.finish(file.id, "done", None, &info)
            })
            .map_err(|e| ReadError::Model(e.to_string()))?;
        self.cost = Some(("code_stretch", code.pieces.len() as f64));
        let size = match code.cells {
            Some(cells) => format!("{cells} cells"),
            None => format!("{} lines", code.lines),
        };
        let keyword_only = code.pieces.len() - worded.len();
        Ok(Outcome {
            state: "done",
            note: Some(format!(
                "{} stretches, {size}{}",
                code.pieces.len(),
                if keyword_only > 0 { format!(", {keyword_only} for keywords only") } else { String::new() }
            )),
        })
    }

    /// The sound of `path` (a recording, or a video's soundtrack) as 30 s windows 25 s apart, embedded as they're
    /// decoded. Silent windows are skipped; a tail of at least 2 s past the last window's overlap gets its own.
    /// Returns (start, end, vector) per window.
    fn sound(&mut self, path: &Path) -> Result<Vec<(f64, f64, Vec<f32>)>, ReadError> {
        let rate = trovara_win::media::SAMPLE_RATE as f64;
        let window = (self.options.audio_window * rate) as usize;
        let hop = (self.options.audio_hop * rate) as usize;
        let embedder = self.embedder(true)?;
        let mut buffer: Vec<f32> = Vec::new();
        let mut offset = 0usize; // sample index of buffer[0] in the file
        let mut pieces = Vec::new();
        let mut model_error: Option<String> = None;
        let mut emit = |buffer: &[f32],
                        offset: usize,
                        pieces: &mut Vec<(f64, f64, Vec<f32>)>|
         -> Result<(), String> {
            let rms = (buffer.iter().map(|&x| x as f64 * x as f64).sum::<f64>()
                / buffer.len().max(1) as f64)
                .sqrt();
            if rms <= 0.003 {
                return Ok(()); // about -50 dBFS: silence isn't worth a vector
            }
            match embedder.embed_audio_samples(buffer) {
                Ok(vector) => {
                    pieces.push((
                        offset as f64 / rate,
                        (offset + buffer.len()) as f64 / rate,
                        vector,
                    ));
                    Ok(())
                }
                Err(EmbedError::Input(_)) => Ok(()),
                Err(other) => Err(other.to_string()),
            }
        };
        let decoded = trovara_win::media::audio(path, |samples| {
            buffer.extend_from_slice(samples);
            while buffer.len() >= window {
                if let Err(error) = emit(&buffer[..window], offset, &mut pieces) {
                    model_error = Some(error.clone());
                    return Err(error);
                }
                buffer.drain(..hop);
                offset += hop;
            }
            Ok(())
        });
        if let Some(error) = model_error {
            return Err(ReadError::Model(error));
        }
        decoded.map_err(|why| ReadError::File(ExtractError::Unreadable(why)))?;
        // The tail: new sound beyond the previous window's overlap, or the whole file if it was shorter than a window.
        let covered = if offset == 0 { 0 } else { window - hop };
        if buffer.len().saturating_sub(covered) >= (2.0 * rate) as usize {
            emit(&buffer, offset, &mut pieces).map_err(ReadError::Model)?;
        }
        Ok(pieces)
    }

    fn audio(&mut self, file: &FileRecord, path: &Path) -> Result<Outcome, ReadError> {
        let info = trovara_win::media::info(path)
            .map_err(|why| ReadError::File(ExtractError::Unreadable(why)))?;
        let pieces = self.sound(path)?;
        if pieces.is_empty() {
            return Err(ReadError::File(ExtractError::Empty(
                "silent or shorter than 2 s".into(),
            )));
        }
        let duration = if info.duration > 0.0 {
            info.duration
        } else {
            pieces.last().map_or(0.0, |p| p.1)
        };
        let store = &self.store;
        store
            .transaction(|| {
                for (start, end, vector) in &pieces {
                    store.add_segment(
                        file.id,
                        SegmentKind::Audio,
                        Modality::Audio,
                        Some(*start),
                        Some(*end),
                        None,
                        None,
                        Some(vector),
                    )?;
                }
                let mut info = Info::new();
                info.insert("duration".into(), format!("{duration:.1}"));
                store.finish(file.id, "done", None, &info)
            })
            .map_err(|e| ReadError::Model(e.to_string()))?;
        self.cost = Some(("audio_second", duration));
        Ok(Outcome {
            state: "done",
            note: Some(format!("{} windows, {} s", pieces.len(), duration as u64)),
        })
    }

    /// A frame every 3 s (or every duration/1200 for very long videos), from 0.5 s, keeping one per shot: a frame that
    /// looks like the last kept one (8x8 average hash within 4 bits) only extends it. Plus the soundtrack.
    fn video(&mut self, file: &FileRecord, path: &Path) -> Result<Outcome, ReadError> {
        let info = trovara_win::media::info(path)
            .map_err(|why| ReadError::File(ExtractError::Unreadable(why)))?;
        let mut kept: Vec<(f64, f64, trovara_core::image::RgbImage)> = Vec::new();
        if info.video && info.duration > 0.0 {
            let step = self
                .options
                .frame_interval
                .max(info.duration / MAX_FRAMES as f64);
            let first = 0.5f64.min(info.duration / 2.0);
            let times: Vec<f64> = (0..)
                .map(|i| first + i as f64 * step)
                .take_while(|&t| t < info.duration)
                .collect();
            let frames = trovara_win::media::frames(path, &times, FRAME_PIXELS, step)
                .map_err(|why| ReadError::File(ExtractError::Unreadable(why)))?;
            let mut last_hash: Option<u64> = None;
            for frame in frames {
                let hash = trovara_win::media::average_hash(&frame.image);
                if last_hash.is_some_and(|last| (last ^ hash).count_ones() <= 4) {
                    continue;
                }
                last_hash = Some(hash);
                kept.push((frame.time, info.duration, frame.image));
            }
            // Each kept frame stands for the stretch until the next one.
            for i in 1..kept.len() {
                kept[i - 1].1 = kept[i].0;
            }
        }
        let sound = if info.audio {
            self.sound(path)?
        } else {
            Vec::new()
        };
        if kept.is_empty() && sound.is_empty() {
            return Err(ReadError::File(ExtractError::Empty(
                "no frames or sound".into(),
            )));
        }
        let budget = self.options.frame_budget as usize;
        let embedder = self.embedder(true)?;
        let mut frame_vectors = Vec::with_capacity(kept.len());
        for (_, _, image) in &kept {
            frame_vectors.push(
                embedder
                    .embed_image_rgb(&image.pixels, image.width, image.height, budget)
                    .ok(),
            );
        }
        let store = &self.store;
        store
            .transaction(|| {
                for ((start, end, _), vector) in kept.iter().zip(&frame_vectors) {
                    store.add_segment(
                        file.id,
                        SegmentKind::Frame,
                        Modality::Image,
                        Some(*start),
                        Some(*end),
                        None,
                        None,
                        vector.as_deref(),
                    )?;
                }
                for (start, end, vector) in &sound {
                    store.add_segment(
                        file.id,
                        SegmentKind::Audio,
                        Modality::Audio,
                        Some(*start),
                        Some(*end),
                        None,
                        None,
                        Some(vector),
                    )?;
                }
                let mut details = Info::new();
                details.insert("duration".into(), format!("{:.1}", info.duration));
                details.insert("frames".into(), kept.len().to_string());
                details.insert("sound_windows".into(), sound.len().to_string());
                if info.width > 0 {
                    details.insert("width".into(), info.width.to_string());
                    details.insert("height".into(), info.height.to_string());
                }
                store.finish(file.id, "done", None, &details)
            })
            .map_err(|e| ReadError::Model(e.to_string()))?;
        self.cost = Some(("video_second", info.duration));
        Ok(Outcome {
            state: "done",
            note: Some(format!(
                "{} frames, {} sound windows, {} s",
                kept.len(),
                sound.len(),
                info.duration as u64
            )),
        })
    }
}

/// A file that couldn't be read (the run goes on), or the model failing (the run stops).
enum ReadError {
    File(ExtractError),
    Model(String),
}

impl From<ExtractError> for ReadError {
    fn from(error: ExtractError) -> Self {
        ReadError::File(error)
    }
}

impl From<Failure> for ReadError {
    fn from(message: Failure) -> Self {
        ReadError::Model(message)
    }
}

/// `\\?\C:\x` (what canonicalize returns on Windows) → `C:\x`.
pub fn strip_verbatim(path: &Path) -> String {
    let text = path.display().to_string();
    text.strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| text.strip_prefix(r"\\?\").map(str::to_string))
        .unwrap_or(text)
}

/// The chosen folders from the command line, as the walk spells them (`C:\x`, any case and slash accepted). Folders
/// that aren't there now are kept as given: an unplugged drive's files stay indexed.
pub fn roots_from(args: &[String], options_with_values: &[&str]) -> Vec<String> {
    crate::index::positional(args, options_with_values)
        .into_iter()
        .map(|root| {
            std::fs::canonicalize(&root)
                .map(|p| strip_verbatim(&p))
                .unwrap_or(root)
        })
        .collect()
}

pub fn options_from(args: &[String]) -> IndexOptions {
    IndexOptions {
        excluded_folders: crate::index::values(args, "--exclude"),
        excluded_extensions: crate::index::values(args, "--exclude-ext")
            .iter()
            .flat_map(|e| IndexOptions::extensions(e))
            .collect(),
        include_datasets: args.iter().any(|a| a == "--include-datasets"),
        code: args.iter().any(|a| a == "--code"),
        code_stretch: crate::value(args, "--code-stretch").and_then(|v| v.parse().ok()).unwrap_or(1800),
        ..IndexOptions::default()
    }
}

/// Opens the index for indexing: an index made with other vectors or settings is emptied and read again (file names
/// stay searchable meanwhile); a file an earlier helper had to stop on is marked so; what Trovara reads better now is
/// queued again. Returns the indexer and notes for the log.
pub fn open(index_folder: &Path, options: IndexOptions) -> Result<(Indexer, Vec<String>), String> {
    // The code index is a file of its own beside the main one: code search never touches the rest.
    let file = if options.code { paths::CODE_INDEX_FILE } else { paths::MAIN_INDEX_FILE };
    let store = IndexStore::open(index_folder, file).map_err(|e| e.to_string())?;
    let mut notes = Vec::new();
    let fingerprint = format!(
        "{} · {}",
        options.fingerprint(),
        trovara_llama::vector_source()
    );
    if store
        .meta("fingerprint")
        .map_err(|e| e.to_string())?
        .is_some_and(|stored| stored != fingerprint)
    {
        store.reset_all().map_err(|e| e.to_string())?;
        notes.push("the vectors or settings changed: every file is read again".into());
    }
    store
        .set_meta("fingerprint", Some(&fingerprint))
        .map_err(|e| e.to_string())?;
    store
        .set_meta("vector_source", Some(&trovara_llama::vector_source()))
        .map_err(|e| e.to_string())?;
    if let Some((Some(path), why)) = crate::watchdog::take_note(index_folder) {
        if store.mark_stopped(&path, &why).map_err(|e| e.to_string())? {
            notes.push(format!("stopped reading {path}: it {why}"));
        }
    }
    // Read again what Trovara reads better now: scans and screenshots from before OCR worked here (once), and
    // pictures that waited for a codec (each run: a failed decode costs milliseconds).
    let mut again = if options.code { 0 } else { store.retry_missing_codecs().map_err(|e| e.to_string())? };
    if !options.code && trovara_win::ocr::is_available() {
        again += store
            .read_again_once(
                "scans_ocr",
                "SELECT DISTINCT file FROM segments WHERE kind = 'page_image'
                 UNION SELECT id FROM files WHERE kind = 'screenshot' AND state = 'done'
                   AND id NOT IN (SELECT file FROM segments WHERE kind = 'ocr')",
            )
            .map_err(|e| e.to_string())?;
    }
    if again > 0 {
        notes.push(format!(
            "{again} files are read again (Trovara reads them better now)"
        ));
    }
    Ok((Indexer::new(store, options)?, notes))
}

/// `trovara-helper index <folders>`: one sync and everything pending, then exits (the app uses `index-service`).
pub fn index(args: &[String]) -> Result<(), String> {
    let roots = roots_from(args, &["--index", "--exclude", "--exclude-ext", "--code-stretch"]);
    if roots.is_empty() {
        return Err("index which folders? trovara-helper index <folder>…".into());
    }
    let started = Instant::now();
    let (mut indexer, notes) = open(&crate::index::index_folder(args), options_from(args))?;
    for note in notes {
        println!("{note}");
    }
    let available: Vec<String> = roots
        .iter()
        .filter(|root| Path::new(root).is_dir())
        .cloned()
        .collect();
    for missing in roots.iter().filter(|root| !Path::new(root).is_dir()) {
        println!("{missing} isn't there now: its files stay indexed");
    }
    indexer.store.set_scope(Some(available.clone()));
    let sync = indexer.sync(&available)?;
    let skipped: usize = sync.skipped.values().sum();
    println!(
        "{} files in {} ({} new, {} changed, {} moved, {} gone; {skipped} left out) in {} ms",
        sync.found,
        available.join(", "),
        sync.added,
        sync.changed,
        sync.moved,
        sync.removed,
        sync.ms
    );
    if !sync.skipped.is_empty() {
        let reasons: Vec<String> = sync
            .skipped
            .iter()
            .map(|(why, n)| format!("{n} {why}"))
            .collect();
        println!("  left out: {}", reasons.join(" · "));
    }
    for (folder, files) in &sync.datasets {
        println!("  dataset, skipped: {folder} ({files} files)");
    }
    let pending = indexer.store.pending_count().map_err(|e| e.to_string())?;
    let unfinished = indexer.store.unfinished().map_err(|e| e.to_string())?.len();
    if pending == 0 && unfinished == 0 {
        println!("up to date");
        return Ok(());
    }
    let mut progress = |event: &Event| {
        if event.state == "failed" || event.state == "skipped" {
            println!(
                "  {} {}: {}",
                event.state,
                event.path,
                event.note.clone().unwrap_or_default()
            );
        }
        if event.done % 25 == 0 || event.done == event.total {
            println!(
                "  {}/{} · {:.0} s",
                event.done,
                event.total,
                started.elapsed().as_secs_f64()
            );
        }
    };
    let states = indexer.run(None, &mut || false, &mut progress)?;
    let summary: Vec<String> = states
        .iter()
        .map(|(state, n)| format!("{n} {state}"))
        .collect();
    println!(
        "first pass: {} in {:.1} s",
        summary.join(", "),
        started.elapsed().as_secs_f64()
    );
    let rest = indexer.finish_long_files(&mut || false, &mut |event| {
        println!(
            "  {} {}",
            event.path,
            event.note.clone().unwrap_or_default()
        )
    })?;
    if !rest.is_empty() {
        println!(
            "long files finished, {:.1} s in all",
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}
