//! Walks the chosen folders and applies the rules (TRD §9.1): reads file-system metadata only, never file contents
//! (except the 600-byte sniff that tells a real MPEG-TS video from a TypeScript file).
//!
//! - Hidden and system items, reparse points (junctions, symlinks: no loops out of the chosen folders), and folders
//!   the rules skip aren't walked.
//! - Cloud placeholders (OneDrive and others) are never opened: reading one would download it.
//! - Inside a code project only screenshots count; folders that look like datasets are skipped and reported.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use trovara_core::rules::{
    classify, classify_code, code_skip_reason, dataset_folders, is_project_root, skip_reason_for_folder,
    system_folders,
    Classification,
};
use trovara_core::store::path_key;
use trovara_core::{Candidate, FileKind, IndexOptions};

#[derive(Debug, Default)]
pub struct CrawlReport {
    pub candidates: Vec<Candidate>,
    /// Reason → files left out.
    pub skipped: BTreeMap<String, usize>,
    /// Reason → folders not walked into.
    pub skipped_folders: BTreeMap<String, usize>,
    /// Folder → files skipped as a dataset.
    pub datasets: HashMap<String, usize>,
}

const HIDDEN_OR_SYSTEM: u32 = 0x2 | 0x4;
const REPARSE_POINT: u32 = 0x400;
/// Recall on data access / recall on open / offline: a cloud file that isn't on this PC.
const CLOUD_ONLY: u32 = 0x0040_0000 | 0x0004_0000 | 0x1000;

/// The chosen folders, whole.
pub fn crawl(roots: &[String], options: &IndexOptions) -> CrawlReport {
    if options.code {
        return crawl_code(roots, options);
    }
    let starts: Vec<(String, PathBuf, bool)> = roots
        .iter()
        .map(|root| (root.clone(), PathBuf::from(root), true))
        .collect();
    crawl_from(&starts, roots, options, HashSet::new())
}

/// Some folders inside the chosen ones (the folders the change journal says changed): each one's own files, or
/// (`whole`) everything in it. The rules apply as in a full crawl: a folder hidden, skipped or excluded somewhere
/// between its root and it isn't read, and one inside a code project keeps only its screenshots.
pub fn crawl_folders(
    roots: &[String],
    folders: &[(String, bool)],
    options: &IndexOptions,
) -> CrawlReport {
    // Code folders are walked whole: what git ignores is decided from the repo's top, not from a changed folder.
    if options.code {
        return crawl_code(roots, options);
    }
    let system = system_folders();
    let mut starts = Vec::new();
    let mut project_roots = HashSet::new();
    for (folder, whole) in folders {
        let key = path_key(folder);
        let Some(root) = roots
            .iter()
            .filter(|r| key == path_key(r) || key.starts_with(&format!("{}\\", path_key(r))))
            .max_by_key(|r| r.len())
        else {
            continue;
        };
        // Every folder from the root down to this one must be one the walk would enter.
        let mut walked = true;
        let mut ancestor = PathBuf::from(root);
        let rest = &folder[root.len().min(folder.len())..];
        let mut chain = vec![ancestor.clone()];
        for part in rest.split(['\\', '/']).filter(|p| !p.is_empty()) {
            ancestor.push(part);
            let hidden = std::fs::metadata(&ancestor)
                .map(|m| attributes(&m) & (HIDDEN_OR_SYSTEM | REPARSE_POINT) != 0)
                .unwrap_or(false);
            if part.starts_with('.')
                || hidden
                || skip_reason_for_folder(&ancestor, &system, &options.excluded_folders).is_some()
            {
                walked = false;
                break;
            }
            chain.push(ancestor.clone());
        }
        if !walked {
            continue;
        }
        // A code project above it: its files are the project's.
        for up in &chain[..chain.len() - 1] {
            if let Ok(entries) = std::fs::read_dir(up) {
                let names: Vec<String> = entries
                    .flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect();
                if is_project_root(names.iter().map(String::as_str)) {
                    project_roots.insert(path_key(&up.to_string_lossy()));
                }
            }
        }
        starts.push((root.clone(), PathBuf::from(folder), *whole));
    }
    crawl_from(&starts, roots, options, project_roots)
}

/// Walks from each (root, start folder, whole): `whole` walks everything under the start, else only its own files.
/// `chosen` are the folders the person chose (never datasets).
fn crawl_from(
    starts: &[(String, PathBuf, bool)],
    chosen: &[String],
    options: &IndexOptions,
    mut project_roots: HashSet<String>,
) -> CrawlReport {
    let mut report = CrawlReport::default();
    let system = system_folders();
    let mut found: Vec<Candidate> = Vec::new();
    let mut found_keys: HashSet<String> = HashSet::new(); // a folder inside another root would be walked twice

    for (root, start, whole) in starts {
        // A skip applies inside the chosen folders that contain it; a folder chosen inside a skipped one is walked whole.
        let root_key = path_key(root);
        let skips: Vec<String> = options
            .excluded_folders
            .iter()
            .filter(|skip| path_key(skip).starts_with(&format!("{root_key}\\")))
            .cloned()
            .collect();
        let mut stack = vec![start.clone()];
        while let Some(folder) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&folder) else {
                *report
                    .skipped_folders
                    .entry("can't be read".into())
                    .or_default() += 1;
                continue;
            };
            let entries: Vec<std::fs::DirEntry> = entries.flatten().collect();
            let names: Vec<String> = entries
                .iter()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            if is_project_root(names.iter().map(String::as_str)) {
                project_roots.insert(path_key(&folder.to_string_lossy()));
            }
            for entry in entries {
                let name = entry.file_name().to_string_lossy().into_owned();
                let Ok(meta) = entry.metadata() else { continue };
                let attributes = attributes(&meta);
                let path = entry.path();
                if meta.is_dir() {
                    if name.starts_with('.') || attributes & (HIDDEN_OR_SYSTEM | REPARSE_POINT) != 0
                    {
                        *report
                            .skipped_folders
                            .entry("hidden, system or linked folder".into())
                            .or_default() += 1;
                    } else if let Some(reason) = skip_reason_for_folder(&path, &system, &skips) {
                        *report.skipped_folders.entry(reason.into()).or_default() += 1;
                    } else {
                        if *whole {
                            stack.push(path);
                        }
                    }
                    continue;
                }
                if !meta.is_file() || name.starts_with('.') || attributes & HIDDEN_OR_SYSTEM != 0 {
                    continue;
                }
                let kind = match classify(&path, options) {
                    Classification::Index(kind) => kind,
                    Classification::Skip(reason) => {
                        *report.skipped.entry(reason.into()).or_default() += 1;
                        continue;
                    }
                    Classification::Ignore => {
                        *report.skipped.entry("other type".into()).or_default() += 1;
                        continue;
                    }
                };
                if attributes & REPARSE_POINT != 0 {
                    *report
                        .skipped
                        .entry("link or special file".into())
                        .or_default() += 1;
                    continue;
                }
                if attributes & CLOUD_ONLY != 0 {
                    *report
                        .skipped
                        .entry("in the cloud, not downloaded".into())
                        .or_default() += 1;
                    continue;
                }
                let Some(text) = path.to_str() else {
                    *report
                        .skipped
                        .entry("name Windows can't spell".into())
                        .or_default() += 1;
                    continue;
                };
                if !found_keys.insert(path_key(text)) {
                    continue;
                }
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0.0, |d| d.as_secs_f64());
                // File IDs are read in `sync`, for paths the index doesn't know yet (opening every file costs time).
                found.push(Candidate {
                    path: text.to_string(),
                    kind,
                    size: meta.len() as i64,
                    mtime,
                    inode: 0,
                    device: 0,
                });
            }
        }
    }

    // Inside a code project only screenshots count (assets, fixtures and datasets don't).
    let mut in_project: HashMap<String, bool> = HashMap::new();
    let mut kept = Vec::with_capacity(found.len());
    for file in found {
        let folder = parent(&file.path);
        if file.kind != FileKind::Screenshot
            && is_in_project(&folder, &project_roots, &mut in_project)
        {
            *report
                .skipped
                .entry("inside a code project".into())
                .or_default() += 1;
        } else {
            kept.push(file);
        }
    }

    if !options.include_datasets {
        let files: Vec<(String, String, FileKind)> = kept
            .iter()
            .map(|c| {
                (
                    parent(&c.path),
                    Path::new(&c.path)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    c.kind,
                )
            })
            .collect();
        let datasets = dataset_folders(&files, chosen, 300);
        if !datasets.is_empty() {
            let keys: HashSet<String> = datasets.keys().map(|f| path_key(f)).collect();
            kept.retain(|file| {
                let skip = keys.contains(&path_key(&parent(&file.path)))
                    && matches!(
                        file.kind,
                        FileKind::Image | FileKind::Audio | FileKind::Video
                    );
                if skip {
                    *report.skipped.entry("dataset".into()).or_default() += 1;
                }
                !skip
            });
            report.datasets = datasets;
        }
    }
    report.candidates = kept;
    report
}

/// The code folders, for the code index (TRD §6.6): code files only (`classify_code`), in a git repo only those git
/// keeps (its .gitignore files, .git\info\exclude and the global excludes, through the `ignore` crate, so no git is
/// needed), and none in vendored or built folders, submodules or worktrees. Outside a repo every code file counts.
fn crawl_code(roots: &[String], options: &IndexOptions) -> CrawlReport {
    use std::sync::{Arc, Mutex};
    let mut report = CrawlReport::default();
    let system = system_folders();
    let skipped_folders: Arc<Mutex<BTreeMap<String, usize>>> = Arc::default();
    let mut found_keys: HashSet<String> = HashSet::new();
    for root in roots {
        let root_key = path_key(root);
        let skips: Vec<String> = options
            .excluded_folders
            .iter()
            .filter(|skip| path_key(skip).starts_with(&format!("{root_key}\\")))
            .cloned()
            .collect();
        let (system, counts) = (system.clone(), skipped_folders.clone());
        let walker = ignore::WalkBuilder::new(root)
            .hidden(true)
            .parents(true) // a repo's subfolder chosen on its own still follows the repo's .gitignore
            .git_ignore(true)
            .git_exclude(true)
            .git_global(true)
            .ignore(true)
            .require_git(true)
            .follow_links(false)
            .filter_entry(move |entry| {
                if entry.depth() == 0 || !entry.file_type().is_some_and(|t| t.is_dir()) {
                    return true;
                }
                let path = entry.path();
                let attributes = entry.metadata().map(|m| attributes(&m)).unwrap_or(0);
                let reason = if attributes & (HIDDEN_OR_SYSTEM | REPARSE_POINT) != 0 {
                    Some("hidden, system or linked folder")
                } else {
                    skip_reason_for_folder(path, &system, &skips).or_else(|| code_skip_reason(path))
                };
                match reason {
                    Some(reason) => {
                        *counts.lock().unwrap().entry(reason.into()).or_default() += 1;
                        false
                    }
                    None => true,
                }
            })
            .build();
        for entry in walker.flatten() {
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            let attributes = attributes(&meta);
            if attributes & HIDDEN_OR_SYSTEM != 0 {
                continue;
            }
            match classify_code(path, options) {
                Classification::Index(_) => {}
                Classification::Skip(reason) => {
                    *report.skipped.entry(reason.into()).or_default() += 1;
                    continue;
                }
                Classification::Ignore => {
                    *report.skipped.entry("other type".into()).or_default() += 1;
                    continue;
                }
            }
            let skip = |report: &mut CrawlReport, why: &str| *report.skipped.entry(why.into()).or_default() += 1;
            if attributes & REPARSE_POINT != 0 {
                skip(&mut report, "link or special file");
                continue;
            }
            if attributes & CLOUD_ONLY != 0 {
                skip(&mut report, "in the cloud, not downloaded");
                continue;
            }
            // Too big to be written by hand; a notebook's file holds its outputs too.
            let notebook = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ipynb"));
            let limit = if notebook { options.max_notebook_bytes } else { options.max_code_bytes };
            if meta.len() > limit {
                skip(&mut report, "too big to be written by hand");
                continue;
            }
            let Some(text) = path.to_str() else {
                skip(&mut report, "name Windows can't spell");
                continue;
            };
            if !found_keys.insert(path_key(text)) {
                continue;
            }
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0.0, |d| d.as_secs_f64());
            report.candidates.push(Candidate {
                path: text.to_string(),
                kind: FileKind::Code,
                size: meta.len() as i64,
                mtime,
                inode: 0,
                device: 0,
            });
        }
    }
    report.skipped_folders = std::mem::take(&mut *skipped_folders.lock().unwrap());
    report
}

fn parent(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn is_in_project(folder: &str, roots: &HashSet<String>, memo: &mut HashMap<String, bool>) -> bool {
    let key = path_key(folder);
    if let Some(&known) = memo.get(&key) {
        return known;
    }
    let answer = roots.contains(&key)
        || Path::new(folder).parent().is_some_and(|up| {
            let up = up.to_string_lossy();
            !up.is_empty() && up.len() < folder.len() && is_in_project(&up, roots, memo)
        });
    memo.insert(key, answer);
    answer
}

#[cfg(windows)]
fn attributes(meta: &std::fs::Metadata) -> u32 {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes()
}

#[cfg(not(windows))]
fn attributes(_meta: &std::fs::Metadata) -> u32 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_crawl_keeps_what_git_keeps() {
        let root = std::env::temp_dir().join(format!("trovara-code-crawl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
        };
        write(r"app\main.py", "print(1)\n");
        write(r"app\README.md", "# App\n");
        write(r"app\.gitignore", "build/\n*.log\nscratch.py\n");
        write(r"app\scratch.py", "x = 1\n"); // ignored by git
        write(r"app\notes\todo.md", "- ship\n"); // new, not ignored: kept
        write(r"app\build\out.py", "x = 2\n"); // ignored (and a built folder)
        write(r"app\vendor\lib\core.py", "x = 3\n"); // vendored: left out
        write(r"app\node_modules\pad\index.js", "x\n"); // a dependency
        write(r"app\data.json", "{}\n"); // data
        write(r"app\big.py", &"x = 1\n".repeat(200_000)); // 1.2 MB: made by a program
        write(r"app\sub\.git", "gitdir: ../.git/modules/sub\n"); // a submodule
        write(r"app\sub\lib.py", "x = 4\n");
        write(r"loose\backup.sh", "rsync\n"); // not in a repo: everything code counts
        std::fs::create_dir_all(root.join(r"app\.git")).unwrap(); // a repo (the ignore crate needs only its folder)
        let options = IndexOptions { code: true, ..IndexOptions::default() };
        let report = crawl(&[root.to_string_lossy().into_owned()], &options);
        let mut names: Vec<String> = report
            .candidates
            .iter()
            .map(|c| c.path[root.to_string_lossy().len() + 1..].to_string())
            .collect();
        names.sort();
        assert_eq!(names, [r"app\README.md", r"app\main.py", r"app\notes\todo.md", r"loose\backup.sh"]);
        assert_eq!(report.skipped_folders["vendored or built code"], 1); // vendor (build is git-ignored first)
        assert_eq!(report.skipped_folders["submodule or worktree"], 1);
        assert_eq!(report.skipped["data file"], 1);
        assert_eq!(report.skipped["too big to be written by hand"], 1);
        // A repo's subfolder chosen on its own still follows the repo's .gitignore.
        let sub = crawl(&[root.join("app").to_string_lossy().into_owned()], &options);
        assert!(!sub.candidates.iter().any(|c| c.path.ends_with("scratch.py")));
        // The main index never takes code from a code project.
        let main = crawl(&[root.to_string_lossy().into_owned()], &IndexOptions::default());
        assert!(main.candidates.is_empty(), "{:?}", main.candidates.iter().map(|c| &c.path).collect::<Vec<_>>());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rules_apply_while_walking() {
        let root = std::env::temp_dir().join(format!("trovara-crawl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let make = |rel: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"x").unwrap();
        };
        make(r"Photos\beach.jpg");
        make(r"Photos\notes.txt");
        make(r"Photos\Old\skipme.jpg");
        make(r"code\app\Cargo.toml");
        make(r"code\app\assets\logo.png");
        make(r"code\app\Screenshot 2026-01-01.png");
        make(r"code\app\node_modules\x\readme.md");
        make(r"keys\server.pem");
        make(r"music\song.mp3");
        let options = IndexOptions {
            excluded_folders: vec![root.join(r"Photos\Old").to_string_lossy().into_owned()],
            ..IndexOptions::default()
        };
        let report = crawl(&[root.to_string_lossy().into_owned()], &options);
        let mut names: Vec<String> = report
            .candidates
            .iter()
            .map(|c| {
                Path::new(&c.path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "Screenshot 2026-01-01.png",
                "beach.jpg",
                "notes.txt",
                "song.mp3"
            ]
        );
        assert_eq!(report.skipped["inside a code project"], 1); // the logo, not the screenshot
        assert_eq!(report.skipped["key or password file"], 1);
        assert_eq!(report.skipped_folders["excluded folder"], 1);
        assert_eq!(
            report.skipped_folders["developer, system or cache folder"],
            1
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
