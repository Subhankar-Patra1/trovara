//! How long indexing a folder will take (TRD §10.5), from metadata alone: counts, PDF page counts (PDFium reads only
//! the header) and media durations (Media Foundation), so nothing is decoded. The setup window shows it per folder and
//! subfolder.
//!
//! Costs per unit start from fixed numbers (the reference's, measured on an M4 Pro; an RTX 3050 laptop measured within
//! 10% of them in M1), scaled by a speed check (`trovara-helper speed`), and go by what indexing measures on this PC
//! once there's enough of a kind (`costs.json` beside the index).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};
use trovara_core::{Candidate, FileKind, IndexOptions};

/// Seconds of indexing per unit.
#[derive(Clone, Debug, Serialize)]
pub struct Costs {
    pub image: f64,
    pub screenshot: f64,
    pub pdf_page: f64,
    pub doc_chunk: f64,
    pub audio_second: f64,
    pub video_second: f64,
    pub code_stretch: f64,
}

impl Default for Costs {
    fn default() -> Self {
        Costs {
            image: 0.12,
            screenshot: 0.32,
            pdf_page: 0.09,
            doc_chunk: 0.045,
            audio_second: 0.0089,
            video_second: 0.019,
            code_stretch: 0.037,
        }
    }
}

/// Units a kind needs before its measured cost is trusted: a few dozen files, or minutes of media.
fn enough(name: &str) -> f64 {
    match name {
        "pdf_page" | "doc_chunk" => 30.0,
        "audio_second" | "video_second" => 600.0,
        "code_stretch" => 100.0,
        _ => 20.0,
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Tally {
    pub seconds: f64,
    pub units: f64,
}

/// What indexing costs on this PC, measured by indexing (`costs.json` beside the index).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Measured {
    pub tallies: BTreeMap<String, Tally>,
    /// How this PC compares to the fixed costs' machine: 1 is as fast, 2 takes twice as long (`speed`).
    pub speed: Option<f64>,
}

impl Measured {
    pub fn file(index_folder: &Path) -> std::path::PathBuf {
        index_folder.join("costs.json")
    }

    pub fn load(index_folder: &Path) -> Measured {
        std::fs::read_to_string(Self::file(index_folder))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, index_folder: &Path) {
        let _ = std::fs::create_dir_all(index_folder);
        let temporary = Self::file(index_folder).with_extension("tmp");
        if std::fs::write(
            &temporary,
            serde_json::to_string_pretty(self).unwrap_or_default(),
        )
        .is_ok()
        {
            let _ = std::fs::rename(&temporary, Self::file(index_folder));
        }
    }

    /// A job that took `seconds` for `units` of `name`. Recent work counts most: past ten times what's enough, what
    /// came before is halved.
    pub fn add(&mut self, name: &str, units: f64, seconds: f64) {
        if units <= 0.0 || seconds <= 0.0 {
            return;
        }
        let tally = self.tallies.entry(name.to_string()).or_default();
        if tally.units > 10.0 * enough(name) {
            tally.seconds /= 2.0;
            tally.units /= 2.0;
        }
        tally.seconds += seconds;
        tally.units += units;
    }

    /// The costs to estimate with: measured ones where there are enough, else the fixed ones as fast as `speed` says.
    pub fn costs(&self) -> Costs {
        let mut costs = Costs::default();
        if let Some(speed) = self.speed {
            let s = speed.clamp(0.1, 20.0);
            for value in [
                &mut costs.image,
                &mut costs.screenshot,
                &mut costs.pdf_page,
                &mut costs.doc_chunk,
                &mut costs.audio_second,
                &mut costs.video_second,
                &mut costs.code_stretch,
            ] {
                *value *= s;
            }
        }
        let measured = |name: &str| {
            self.tallies
                .get(name)
                .filter(|t| t.units >= enough(name))
                .map(|t| t.seconds / t.units)
        };
        costs.image = measured("image").unwrap_or(costs.image);
        costs.screenshot = measured("screenshot").unwrap_or(costs.screenshot);
        costs.pdf_page = measured("pdf_page").unwrap_or(costs.pdf_page);
        costs.doc_chunk = measured("doc_chunk").unwrap_or(costs.doc_chunk);
        costs.audio_second = measured("audio_second").unwrap_or(costs.audio_second);
        costs.video_second = measured("video_second").unwrap_or(costs.video_second);
        costs.code_stretch = measured("code_stretch").unwrap_or(costs.code_stretch);
        costs
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Estimate {
    pub files: BTreeMap<String, usize>,
    /// Pages the first pass will read, and all of them.
    pub pdf_pages: usize,
    pub pdf_pages_total: usize,
    pub audio_seconds: f64,
    pub video_seconds: f64,
    pub doc_chunks: usize,
    /// The first pass.
    pub seconds: f64,
    /// Long PDFs and documents, and how long the rest of them takes once everything else is in.
    pub long_files: usize,
    pub later_seconds: f64,
    /// The code index: its stretches of code, and about how big it gets on disk.
    pub code_stretches: usize,
    pub bytes: f64,
}

/// What the code index takes on disk (the reference's measures): per character of code (its text, twice, and its
/// keyword index) and per stretch (a vector of 768 halves, its row). A stretch holds 1,400 characters on average.
const CODE_BYTES_PER_CHARACTER: f64 = 2.4;
const CODE_BYTES_PER_STRETCH: f64 = 1600.0;
const CODE_CHARACTERS_PER_STRETCH: i64 = 1400;

impl Estimate {
    fn add(&mut self, other: &Estimate) {
        for (kind, n) in &other.files {
            *self.files.entry(kind.clone()).or_default() += n;
        }
        self.pdf_pages += other.pdf_pages;
        self.pdf_pages_total += other.pdf_pages_total;
        self.audio_seconds += other.audio_seconds;
        self.video_seconds += other.video_seconds;
        self.doc_chunks += other.doc_chunks;
        self.seconds += other.seconds;
        self.long_files += other.long_files;
        self.later_seconds += other.later_seconds;
        self.code_stretches += other.code_stretches;
        self.bytes += other.bytes;
    }
}

pub fn estimate(candidates: &[Candidate], options: &IndexOptions, costs: &Costs) -> Estimate {
    let mut estimate = Estimate::default();
    for file in candidates {
        *estimate
            .files
            .entry(file.kind.as_str().to_string())
            .or_default() += 1;
        match file.kind {
            FileKind::Image => estimate.seconds += costs.image,
            FileKind::Screenshot => estimate.seconds += costs.screenshot,
            FileKind::Pdf => {
                let total = trovara_extract::pdf::page_count(Path::new(&file.path));
                let all = total.min(options.max_pages as usize);
                let first = if total > options.big_pdf_pages as usize {
                    (options.pdf_step as usize).min(all)
                } else {
                    all
                };
                estimate.pdf_pages_total += total;
                estimate.pdf_pages += first;
                estimate.seconds += first as f64 * costs.pdf_page;
                if all > first {
                    estimate.long_files += 1;
                    estimate.later_seconds += (all - first) as f64 * costs.pdf_page;
                }
            }
            FileKind::Doc => {
                let all = ((file.size / 2000) as usize).clamp(1, options.max_chunks as usize);
                let first = all.min(options.doc_step as usize);
                estimate.doc_chunks += first;
                estimate.seconds += first as f64 * costs.doc_chunk;
                if all > first {
                    estimate.long_files += 1;
                    estimate.later_seconds += (all - first) as f64 * costs.doc_chunk;
                }
            }
            FileKind::Audio => {
                let seconds = trovara_win::media::info(Path::new(&file.path))
                    .map(|i| i.duration)
                    .unwrap_or(0.0);
                estimate.audio_seconds += seconds;
                estimate.seconds += seconds * costs.audio_second;
            }
            FileKind::Video => {
                let seconds = trovara_win::media::info(Path::new(&file.path))
                    .map(|i| i.duration)
                    .unwrap_or(0.0);
                estimate.video_seconds += seconds;
                estimate.seconds += seconds * costs.video_second;
            }
            FileKind::Code => {
                // A notebook's file is mostly its outputs, which aren't read: a guess at its cells' share.
                let characters =
                    if file.path.to_ascii_lowercase().ends_with(".ipynb") { file.size / 8 } else { file.size };
                let stretches =
                    ((characters + CODE_CHARACTERS_PER_STRETCH - 1) / CODE_CHARACTERS_PER_STRETCH).max(1) as usize;
                estimate.code_stretches += stretches;
                estimate.seconds += stretches as f64 * costs.code_stretch;
                estimate.bytes +=
                    characters as f64 * CODE_BYTES_PER_CHARACTER + stretches as f64 * CODE_BYTES_PER_STRETCH;
            }
        }
    }
    estimate
}

/// Estimates for each subfolder one level down, and for the files right in `folder` (under its own path).
pub fn by_subfolder(
    candidates: &[Candidate],
    folder: &str,
    options: &IndexOptions,
    costs: &Costs,
) -> BTreeMap<String, Estimate> {
    let prefix = format!("{}\\", folder.trim_end_matches(['\\', '/']));
    // path_key drops a trailing backslash (but a drive's root keeps it): this is where "rest" starts.
    let key_prefix = trovara_core::store::inside_prefix(&trovara_core::store::path_key(folder));
    let mut groups: HashMap<String, Vec<Candidate>> = HashMap::new();
    for file in candidates {
        let key = trovara_core::store::path_key(&file.path);
        let group = key
            .strip_prefix(&key_prefix)
            .and_then(|rest| {
                rest.find('\\')
                    .map(|at| format!("{prefix}{}", &file.path[prefix.len()..prefix.len() + at]))
            })
            .unwrap_or_else(|| folder.to_string());
        groups.entry(group).or_default().push(file.clone());
    }
    groups
        .into_iter()
        .map(|(group, files)| (group, estimate(&files, options, costs)))
        .collect()
}

/// `trovara-helper estimate <folders> [--json]`: one line per folder as soon as it's done.
pub fn run(args: &[String]) -> Result<(), String> {
    let roots = crate::indexer::roots_from(args, &["--index", "--exclude", "--exclude-ext"]);
    if roots.is_empty() {
        return Err("which folders? trovara-helper estimate <folder>…".into());
    }
    let json = crate::flag(args, "--json");
    let options = crate::indexer::options_from(args);
    let costs = Measured::load(&crate::index::index_folder(args)).costs();
    let mut total = 0.0;
    for folder in &roots {
        if !Path::new(folder).is_dir() {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"folder": folder, "readable": false, "seconds": 0, "files": {}})
                );
            } else {
                println!("{folder}    can't read it (not there, or not allowed)");
            }
            continue;
        }
        let started = std::time::Instant::now();
        let crawl = crate::crawler::crawl(std::slice::from_ref(folder), &options);
        let parts = by_subfolder(&crawl.candidates, folder, &options, &costs);
        let mut all = Estimate::default();
        for part in parts.values() {
            all.add(part);
        }
        total += all.seconds;
        // Biggest first: the folder lists show where a long first pass comes from, so it can be cut down.
        let mut subfolders: Vec<(&String, &Estimate)> =
            parts.iter().filter(|(name, _)| *name != folder).collect();
        subfolders.sort_by(|a, b| b.1.seconds.total_cmp(&a.1.seconds).then(a.0.cmp(b.0)));
        if json {
            let mut fields = serde_json::to_value(&all).unwrap_or_default();
            fields["folder"] = serde_json::json!(folder);
            fields["readable"] = serde_json::json!(true);
            fields["skipped"] = serde_json::json!(crawl.skipped.values().sum::<usize>());
            fields["datasets"] = serde_json::json!(crawl.datasets.len());
            fields["ms"] = serde_json::json!(started.elapsed().as_millis() as u64);
            fields["subfolders"] = serde_json::json!(subfolders
                .iter()
                .take(500)
                .map(|(name, e)| serde_json::json!({"folder": name, "seconds": e.seconds, "files": e.files, "long_files": e.long_files, "later_seconds": e.later_seconds}))
                .collect::<Vec<_>>());
            println!("{fields}");
        } else {
            let files: Vec<String> = all
                .files
                .iter()
                .map(|(kind, n)| format!("{n} {kind}"))
                .collect();
            println!(
                "{folder}    {}    first pass ~{}{}",
                files.join(", "),
                clock(all.seconds),
                if all.long_files > 0 {
                    format!(
                        ", then ~{} for the rest of {} long files",
                        clock(all.later_seconds),
                        all.long_files
                    )
                } else {
                    String::new()
                }
            );
            for (name, e) in subfolders.iter().take(8) {
                println!(
                    "    {:<60} ~{}",
                    Path::new(name)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    clock(e.seconds)
                );
            }
        }
    }
    if !json && roots.len() > 1 {
        println!("all folders: ~{}", clock(total));
    }
    Ok(())
}

/// `trovara-helper speed`: how this PC compares to the fixed costs' machine (a photo at 140 tokens takes 0.12 s there),
/// from embedding the reference photo, kept beside the index; and what indexing has measured here.
pub fn speed(args: &[String]) -> Result<(), String> {
    let index_folder = crate::index::index_folder(args);
    let mut measured = Measured::load(&index_folder);
    if let (Ok(models), Some(reference)) = (
        trovara_llama::Models::locate(),
        trovara_llama::reference::folder(),
    ) {
        let mut embedder =
            trovara_llama::Embedder::new(&models, false, None).map_err(|e| e.to_string())?;
        let photo = reference.join("photo.jpg");
        let mut best = f64::MAX;
        for _ in 0..4 {
            let started = std::time::Instant::now();
            embedder
                .embed_image_file(&photo, 140)
                .map_err(|e| e.to_string())?;
            best = best.min(started.elapsed().as_secs_f64());
        }
        let speed = best / Costs::default().image;
        measured.speed = Some(speed);
        measured.save(&index_folder);
        println!(
            "a photo at 140 tokens: {:.3} s on {} (×{speed:.2} the fixed costs' machine)",
            best, embedder.device
        );
    } else {
        println!("no model yet: the fixed costs stand in until it's downloaded");
    }
    println!(
        "measured by indexing ({}):{}",
        Measured::file(&index_folder).display(),
        if measured.tallies.is_empty() {
            " nothing yet"
        } else {
            ""
        }
    );
    for (name, tally) in &measured.tallies {
        println!(
            "  {name:13} {:.4} s each, over {:.0}",
            tally.seconds / tally.units,
            tally.units
        );
    }
    Ok(())
}

fn clock(seconds: f64) -> String {
    let s = seconds.round() as u64;
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min", (s + 30) / 60)
    } else {
        format!("{}h {:02}min", s / 3600, (s % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_estimates_time_and_room() {
        let file = |name: &str, size: i64| Candidate {
            path: format!(r"C:\x\{name}"),
            kind: FileKind::Code,
            size,
            mtime: 0.0,
            inode: 0,
            device: 0,
        };
        let costs = Costs::default();
        let options = IndexOptions { code: true, ..IndexOptions::default() };
        let estimate = estimate(&[file("a.py", 14_000), file("b.swift", 100), file("c.ipynb", 112_000)], &options, &costs);
        // 14,000 characters are 10 stretches, a tiny file 1, a notebook's cells about an eighth of its file.
        assert_eq!(estimate.code_stretches, 10 + 1 + 10);
        assert_eq!(estimate.files["code"], 3);
        assert!((estimate.seconds - 21.0 * costs.code_stretch).abs() < 1e-9);
        assert!(estimate.bytes > 21.0 * 1600.0 && estimate.bytes < 200_000.0);
    }

    #[test]
    fn measured_costs_take_over_once_there_are_enough() {
        let mut measured = Measured::default();
        assert_eq!(measured.costs().image, 0.12);
        measured.speed = Some(2.0);
        assert!((measured.costs().image - 0.24).abs() < 1e-9);
        for _ in 0..19 {
            measured.add("image", 1.0, 0.5);
        }
        assert!(
            (measured.costs().image - 0.24).abs() < 1e-9,
            "19 aren't enough yet"
        );
        measured.add("image", 1.0, 0.5);
        assert!((measured.costs().image - 0.5).abs() < 1e-9);
        assert_eq!(clock(42.0), "42 s");
        assert_eq!(clock(3_700.0), "1h 01min");
    }
}
