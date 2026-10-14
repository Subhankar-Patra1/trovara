//! `trovara-helper search-service`: the search panel's searcher as a process (TRD §11). It keeps the index open and
//! the query encoder warm, so a search while typing costs milliseconds, not a process start.
//!
//! One JSON request per line on stdin, one JSON reply per line on stdout with the request's `id`; lines without an id
//! are events ({"event": "ready" | "encoder", …}).
//!   {"op":"search","q":…,"code":bool,"kinds":[…],"limit":n,"meaning":bool,"prefix_last":bool}: "code" searches the
//!       code index (code.sqlite, opened once it exists) with the code prompt; hits then carry "focus", the line to
//!       open at
//!       → {"hits":[…], "strong":n, "meaning":bool, "ms":{"embed":…,"search":…}}
//!   {"op":"embed_ahead","q":…}   → {"ok":true}: embeds and caches (the panel sends what's typed on every keystroke)
//!   {"op":"text","segment":id}   → {"text":…}: the part of a file a hit points at, for the preview
//!   {"op":"render","path":…,"kind":"pdf|video|image","loc":n,"pixels":n} → {"image":path}: a JPEG for a preview or a
//!       thumbnail, made beside the request loop (typing never waits on one), kept in <index>\previews until restart
//!   {"op":"reload","code":bool?}  → {"ok":true,"vectors":n,"code_vectors":n}: after indexing changed an index (both
//!       without "code")
//!   {"op":"folders","roots":[…],"skipped":[…],"types":[…],"code_roots":[…],"code_skipped":[…]} → {"ok":true}: what
//!       the panel may show
//!   {"op":"code_off"}            → {"ok":true}: deletes the code index ("Turn Off Code Search…")
//!   {"op":"prewarm"}             → {"ok":true}: starts the encoder and loads the vectors (the panel is opening)
//!   {"op":"status"}              → {"encoder":"stopped|starting|ready|unavailable","vectors":n}
//!   {"op":"quit"}                → {"ok":true}, then exit
//! After `--encoder-idle` seconds without a query the encoder and the vectors are freed; keyword search keeps working.

use std::collections::{BTreeSet, HashMap};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use trovara_core::searcher::{strong_count, SearchOptions};
use trovara_core::store::path_key;
use trovara_core::{paths, FileKind, IndexOptions, IndexStore, Prompts, Searcher};
use trovara_llama::{Embedder, Models};

use crate::service::emit;

struct State {
    searcher: Searcher,
    /// The code index's searcher, once there is a code index ("code:" searches).
    code: Option<Searcher>,
    index_folder: PathBuf,
    encoder: Option<Embedder>,
    encoder_state: &'static str,
    cache: HashMap<String, Vec<f32>>,
    last_query: Instant,
    roots: Vec<String>,
    skipped: Vec<String>,
    types: BTreeSet<String>,
    /// The code folders and what's left out in them: what code results may show.
    code_roots: Vec<String>,
    code_skipped: Vec<String>,
    log: PathBuf,
}

impl State {
    /// The query's vector (cached), or None when the encoder can't run: keyword results still work.
    /// `code`: with the code prompt ("task: code retrieval"), for the code index.
    fn vector(&mut self, text: &str, code: bool) -> Option<Vec<f32>> {
        self.last_query = Instant::now();
        let prompt = if code { Prompts::code_query(text) } else { Prompts::query(text) };
        if let Some(cached) = self.cache.get(&prompt) {
            return Some(cached.clone());
        }
        let encoder = self.running_encoder()?;
        match encoder.embed_texts(std::slice::from_ref(&prompt)) {
            Ok(mut vectors) => {
                let vector = vectors.remove(0);
                if self.cache.len() > 256 {
                    self.cache.clear();
                }
                self.cache.insert(prompt, vector.clone());
                Some(vector)
            }
            Err(error) => {
                eprintln!("encoder: {error}");
                self.stop_encoder(); // the next query starts a new one
                None
            }
        }
    }

    fn running_encoder(&mut self) -> Option<&mut Embedder> {
        if self.encoder.is_none() {
            if self.encoder_state == "unavailable" {
                return None;
            }
            self.encoder_state = "starting";
            let started = Instant::now();
            match Models::locate().and_then(|models| Embedder::new(&models, true, Some(&self.log))) {
                Ok(encoder) => {
                    emit(json!({"event": "encoder", "state": "ready", "seconds": started.elapsed().as_secs_f64(), "device": encoder.device}));
                    self.encoder = Some(encoder);
                    self.encoder_state = "ready";
                }
                Err(error) => {
                    self.encoder_state = "unavailable";
                    emit(json!({"event": "encoder", "state": "unavailable", "error": error.to_string()}));
                    return None;
                }
            }
        }
        self.encoder.as_mut()
    }

    fn stop_encoder(&mut self) {
        self.encoder = None;
        self.cache.clear();
        if self.encoder_state != "unavailable" {
            self.encoder_state = "stopped";
        }
    }

    /// Whether the panel may show `path`: under a chosen folder and not a skipped one, not of a skipped type, and
    /// still there.
    fn shown(&self, path: &str, code: bool) -> bool {
        let key = path_key(path);
        let under = |folder: &String| {
            let f = path_key(folder);
            key == f || key.starts_with(&format!("{f}\\"))
        };
        let (roots, skipped) = if code { (&self.code_roots, &self.code_skipped) } else { (&self.roots, &self.skipped) };
        (roots.is_empty() || roots.iter().any(under))
            && !skipped.iter().any(under)
            && !self.types.contains(&Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or_default().to_lowercase())
            && Path::new(path).exists()
    }

    fn handle(&mut self, op: &str, request: &Value) -> Result<Value, String> {
        match op {
            "search" => {
                let text = request["q"].as_str().unwrap_or_default();
                let code = request["code"].as_bool() == Some(true);
                if code && !self.open_code()? {
                    return Ok(json!({"hits": [], "strong": 0, "meaning": false, "warning": "code search is off"}));
                }
                let kinds: Option<BTreeSet<FileKind>> =
                    request["kinds"].as_array().map(|k| k.iter().filter_map(|v| v.as_str().and_then(FileKind::parse)).collect());
                let limit = request["limit"].as_u64().unwrap_or(40) as usize;
                let want_meaning = request["meaning"].as_bool().unwrap_or(true);
                let started = Instant::now();
                let mut vector = if want_meaning && !text.trim().is_empty() { self.vector(text, code) } else { None };
                // Vectors from another model or pipeline must never meet: keyword-only then.
                let mut warning = None;
                if vector.is_some() {
                    let source = if code { self.code.as_ref().and_then(|c| c.vector_source.clone()) } else { self.searcher.vector_source.clone() };
                    if let Some(indexed) = &source {
                        if *indexed != trovara_llama::vector_source() {
                            vector = None;
                            warning = Some(format!("meaning search is off: the index was built with {indexed}"));
                        }
                    }
                }
                let embedded = Instant::now();
                let options = SearchOptions { kinds: kinds.as_ref(), limit, prefix_last: request["prefix_last"].as_bool().unwrap_or(false), ..SearchOptions::default() };
                let searcher = if code { self.code.as_mut().unwrap() } else { &mut self.searcher };
                let found = searcher.search(text, vector.as_deref(), &options).map_err(|e| e.to_string())?;
                let hits: Vec<_> = found.into_iter().filter(|hit| self.shown(&hit.path, code)).collect();
                let mut reply = json!({
                    "hits": hits,
                    "strong": strong_count(&hits, 0.5),
                    "meaning": vector.is_some(),
                    "ms": {"embed": (embedded - started).as_secs_f64() * 1000.0, "search": embedded.elapsed().as_secs_f64() * 1000.0},
                });
                if let Some(warning) = warning {
                    reply["warning"] = json!(warning);
                }
                Ok(reply)
            }
            "embed_ahead" => {
                let text = request["q"].as_str().unwrap_or_default();
                if !text.trim().is_empty() {
                    self.vector(text, request["code"].as_bool() == Some(true));
                }
                Ok(json!({"ok": true}))
            }
            "text" => {
                let segment = request["segment"].as_i64().ok_or("which segment?")?;
                let searcher = match (request["code"].as_bool() == Some(true), self.code.as_ref()) {
                    (true, Some(code)) => code,
                    _ => &self.searcher,
                };
                Ok(json!({"text": searcher.text_of_segment(segment).map_err(|e| e.to_string())?}))
            }
            "reload" => {
                // Only the index that changed, when the request says which ("code": true or false).
                let which = request["code"].as_bool();
                if which != Some(true) {
                    reload(&mut self.searcher, &self.index_folder, paths::MAIN_INDEX_FILE)?;
                }
                if which != Some(false) {
                    match self.code.as_mut() {
                        Some(code) => reload(code, &self.index_folder, paths::CODE_INDEX_FILE)?,
                        None => {
                            self.open_code()?;
                        }
                    }
                }
                Ok(json!({"ok": true, "vectors": self.searcher.vector_count(),
                          "code_vectors": self.code.as_ref().map_or(0, |c| c.vector_count())}))
            }
            "code_off" => {
                // "Turn Off Code Search…": the code index goes (the app stopped its indexing helper first).
                self.code = None;
                for name in [paths::CODE_INDEX_FILE.to_string(), format!("{}-wal", paths::CODE_INDEX_FILE), format!("{}-shm", paths::CODE_INDEX_FILE)] {
                    let path = self.index_folder.join(name);
                    if path.exists() {
                        std::fs::remove_file(&path).map_err(|e| format!("can't delete {}: {e}", path.display()))?;
                    }
                }
                Ok(json!({"ok": true}))
            }
            "folders" => {
                let list = |name: &str| request[name].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect::<Vec<_>>());
                if let Some(roots) = list("roots") {
                    self.roots = roots;
                }
                if let Some(skipped) = list("skipped") {
                    self.skipped = skipped;
                }
                if let Some(types) = list("types") {
                    self.types = IndexOptions::extensions(&types.join(","));
                }
                if let Some(roots) = list("code_roots") {
                    self.code_roots = roots;
                }
                if let Some(skipped) = list("code_skipped") {
                    self.code_skipped = skipped;
                }
                Ok(json!({"ok": true}))
            }
            "prewarm" => {
                self.last_query = Instant::now();
                self.running_encoder();
                if !self.searcher.vectors_loaded() {
                    self.searcher.reload().map_err(|e| e.to_string())?;
                }
                Ok(json!({"ok": true, "encoder": self.encoder_state, "vectors": self.searcher.vector_count()}))
            }
            "status" => Ok(json!({
                "encoder": self.encoder_state,
                "vectors": if self.searcher.vectors_loaded() { self.searcher.vector_count() } else { 0 },
                "device": self.encoder.as_ref().map(|e| e.device.clone()),
                "index": self.searcher.store.file.display().to_string(),
            })),
            other => Err(format!("unknown op {other}")),
        }
    }

    /// The code index's searcher, opened the first time it's needed and there is one. False: no code index.
    fn open_code(&mut self) -> Result<bool, String> {
        if self.code.is_some() {
            return Ok(true);
        }
        if !self.index_folder.join(paths::CODE_INDEX_FILE).is_file() {
            return Ok(false);
        }
        let store = IndexStore::open(&self.index_folder, paths::CODE_INDEX_FILE).map_err(|e| e.to_string())?;
        self.code = Some(Searcher::new(store, false).map_err(|e| e.to_string())?);
        Ok(true)
    }
}

/// Reads an index again after indexing: with its vectors when they were loaded, else files only (kinds, names;
/// vectors come back with the next meaning search).
fn reload(searcher: &mut Searcher, folder: &Path, file: &str) -> Result<(), String> {
    if searcher.vectors_loaded() {
        return searcher.reload().map_err(|e| e.to_string());
    }
    let store = IndexStore::open(folder, file).map_err(|e| e.to_string())?;
    *searcher = Searcher::new(store, false).map_err(|e| e.to_string())?;
    Ok(())
}

/// Renders run beside the request loop, two at a time, each made once per file version.
struct Renders {
    folder: PathBuf,
    done: Mutex<HashMap<String, String>>,
    slots: Mutex<usize>,
    free: Condvar,
}

impl Renders {
    fn render(&self, request: &Value) -> Result<Value, String> {
        let path = request["path"].as_str().ok_or("which file?")?;
        let kind = request["kind"].as_str().unwrap_or("image");
        let loc = request["loc"].as_f64();
        let pixels = (request["pixels"].as_u64().unwrap_or(1024) as usize).clamp(32, 2048);
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0.0, |d| d.as_secs_f64());
        let key = format!("{path}|{modified}|{kind}|{}|{pixels}", loc.unwrap_or(-1.0));
        if let Some(done) = self.done.lock().unwrap().get(&key).cloned() {
            if Path::new(&done).exists() {
                return Ok(json!({"image": done}));
            }
        }
        let image = match kind {
            "pdf" => trovara_extract::pdf::render_page(Path::new(path), loc.unwrap_or(1.0).max(1.0) as usize, pixels).map_err(|e| e.to_string())?,
            "video" => {
                let at = loc.unwrap_or(0.0).max(0.0);
                trovara_win::media::frames(Path::new(path), &[at], pixels, 2.0)?.into_iter().next().ok_or_else(|| format!("no frame at {at} s"))?.image
            }
            _ => {
                if Path::new(path).extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")) {
                    trovara_extract::svg::render(Path::new(path), pixels, 1).map_err(|e| e.to_string())?.0
                } else {
                    let decoded = trovara_win::wic::decode(Path::new(path), pixels, 1).map_err(|e| e.to_string())?;
                    let orientation = trovara_extract::exif::read(Path::new(path)).and_then(|e| e.orientation).unwrap_or(1);
                    let heif = Path::new(path).extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e.to_lowercase().as_str(), "heic" | "heif" | "avif"));
                    decoded.image.oriented(if heif { 1 } else { orientation })
                }
            }
        };
        let name = format!("{:016x}.jpg", fxhash(&key));
        let out = self.folder.join(name);
        trovara_win::wic::save_jpeg(&image, &out, 0.85).map_err(|e| e.to_string())?;
        let out = out.display().to_string();
        self.done.lock().unwrap().insert(key, out.clone());
        Ok(json!({"image": out}))
    }
}

/// A stable 64-bit hash for preview file names.
fn fxhash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
}

pub fn serve(args: &[String]) -> Result<(), String> {
    let index_folder = crate::index::index_folder(args);
    let idle = Duration::from_secs_f64(crate::value(args, "--encoder-idle").and_then(|v| v.parse().ok()).unwrap_or(300.0));
    let store = IndexStore::open(&index_folder, paths::MAIN_INDEX_FILE).map_err(|e| e.to_string())?;
    let searcher = Searcher::new(store, false).map_err(|e| e.to_string())?;
    let previews = index_folder.join("previews");
    let _ = std::fs::remove_dir_all(&previews);
    std::fs::create_dir_all(&previews).map_err(|e| e.to_string())?;
    let _ = std::fs::create_dir_all(paths::logs());
    let state = Arc::new(Mutex::new(State {
        searcher,
        code: None,
        index_folder: index_folder.clone(),
        encoder: None,
        encoder_state: "stopped",
        cache: HashMap::new(),
        last_query: Instant::now(),
        roots: crate::indexer::roots_from(args, &["--index", "--exclude", "--exclude-ext", "--encoder-idle", "--code-root", "--code-exclude"]),
        skipped: crate::index::values(args, "--exclude"),
        types: crate::index::values(args, "--exclude-ext").iter().flat_map(|e| IndexOptions::extensions(e)).collect(),
        code_roots: crate::index::values(args, "--code-root"),
        code_skipped: crate::index::values(args, "--code-exclude"),
        log: paths::logs().join("search.log"),
    }));
    let renders = Arc::new(Renders { folder: previews, done: Mutex::new(HashMap::new()), slots: Mutex::new(2), free: Condvar::new() });

    // After `idle` without a query the encoder and the vectors go (their memory back to the system).
    {
        let state = state.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
            if (state.encoder.is_some() || state.searcher.vectors_loaded()) && state.last_query.elapsed() > idle {
                state.stop_encoder();
                state.searcher.release_vectors();
                emit(json!({"event": "encoder", "state": "stopped", "reason": "idle"}));
            }
        });
    }

    emit(json!({"event": "ready", "index": index_folder.display().to_string(), "pid": std::process::id()}));
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim_start_matches('\u{FEFF}').trim();
        if line.is_empty() {
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(line) else { continue };
        let id = request["id"].clone();
        let op = request["op"].as_str().unwrap_or_default().to_string();
        if op == "quit" {
            emit(json!({"id": id, "ok": true}));
            break;
        }
        if op == "render" {
            {
                let mut slots = renders.slots.lock().unwrap();
                while *slots == 0 {
                    slots = renders.free.wait(slots).unwrap();
                }
                *slots -= 1;
            }
            let renders = renders.clone();
            std::thread::spawn(move || {
                let mut reply = renders.render(&request).unwrap_or_else(|error| json!({"ok": false, "error": error}));
                reply["id"] = id;
                emit(reply);
                *renders.slots.lock().unwrap() += 1;
                renders.free.notify_one();
            });
            continue;
        }
        let mut reply = {
            let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
            state.handle(&op, &request).unwrap_or_else(|error| json!({"ok": false, "error": error}))
        };
        reply["id"] = id;
        emit(reply);
    }
    Ok(())
}
