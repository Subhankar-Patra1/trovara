//! `trovara-helper index-service`: the app's indexing helper (TRD §10.2). Commands in on stdin, one JSON event per
//! line out on stdout.
//!
//!   stdin:  sync | stop | backfill on | backfill off | pause | resume | full speed on | full speed off   (EOF = stop)
//!   stdout: {"event": "start" | "note" | "missing" | "pruned" | "sync" | "file" | "rest" | "summary" | "waiting" |
//!            "idle" | "error" | "exit", …}
//!
//! The loop (the reference's): sync; index what the sync just queued when it's a few files (new files go first, on any
//! power source); then the backfill, when allowed; then the rest of long files. Between files it yields to stop, pause
//! and a blocked backfill at once, and to a new sync once indexing has had about as long as the last sync took. When
//! it's done it keeps the model warm for `--linger` seconds (screenshots come in bursts), then frees it.
//!
//! Where Trovara goes further than the reference: with `--watch` the helper watches the folders itself (a new file is
//! searchable a second or two after it lands) and stays running, small, after freeing the model; and the power rules
//! live here: `--on-battery no` holds the backfill while unplugged, Battery Saver always holds it, and `--hours
//! 22:00-06:00` keeps it to set hours. New files are still read on any power source, as in the reference.

use std::collections::HashSet;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::indexer::{self, Event, Indexer};
use crate::{flag, value};

/// One JSON object per line on stdout. The app may be gone by now (it quit while this helper lingered): a closed pipe
/// is ignored.
pub fn emit(event: Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{event}");
    let _ = out.flush();
}

static JSON: AtomicBool = AtomicBool::new(false);
static BACKGROUND: AtomicBool = AtomicBool::new(false);

/// A line for whoever's listening: a `note` event in service mode (stdout is the protocol there), a printed line in
/// the CLI.
pub fn say(message: &str) {
    if JSON.load(Ordering::Relaxed) {
        emit(json!({"event": "note", "message": message}));
    } else {
        println!("  {message}");
    }
}

/// Whether the helper runs in Windows' background mode right now (the model load steps out of it).
pub fn in_background() -> bool {
    BACKGROUND.load(Ordering::Relaxed)
}

/// `--priority auto`: eco on battery, normal on mains power, switched as the power source changes.
static AUTO: AtomicBool = AtomicBool::new(false);
/// "full speed on": normal priority whatever the power source, until "full speed off".
static FULL_SPEED: AtomicBool = AtomicBool::new(false);

/// For `auto`: the priority the power source calls for, applied when it changed (cheap to call often).
fn apply_auto_priority() {
    if !AUTO.load(Ordering::Relaxed) || FULL_SPEED.load(Ordering::Relaxed) {
        return;
    }
    let want = trovara_win::system::power().on_battery;
    if want != in_background() {
        set_background(want);
        say(if want {
            "on battery: indexing in eco mode"
        } else {
            "on mains power: indexing at normal priority"
        });
    }
}

/// The level `--priority` asked for (below `normal`): "low", "thread-background", "background" or "eco".
static LEVEL: Mutex<String> = Mutex::new(String::new());

/// Applies (`on`) or lifts the priority level. The indexing work runs on the calling thread (the service loop).
fn set_background(on: bool) {
    BACKGROUND.store(on, Ordering::Relaxed);
    let level = LEVEL.lock().unwrap_or_else(|p| p.into_inner()).clone();
    match level.as_str() {
        "background" | "eco" => {
            if on {
                trovara_win::system::enter_background(level == "eco");
            } else {
                trovara_win::system::leave_background();
            }
        }
        "process-low" => trovara_win::system::set_below_normal(on),
        "thread-background" => trovara_win::system::set_thread_low(on, true),
        _ => trovara_win::system::set_thread_low(on, false),
    }
}

#[derive(Default)]
struct Requests {
    sync: bool,
    stop: bool,
    backfill: bool,
    paused: bool,
    full_speed: bool,
    /// A command came in: the loop looks again (and says what it does now).
    changed: bool,
}

struct Shared {
    requests: Mutex<Requests>,
    wake: Condvar,
}

impl Shared {
    fn with<T>(&self, body: impl FnOnce(&mut Requests) -> T) -> T {
        body(&mut self.requests.lock().unwrap_or_else(|p| p.into_inner()))
    }

    fn signal(&self) {
        self.wake.notify_all();
    }
}

/// When the backfill may run, besides being allowed: the power source, the screen and the hours.
struct Policy {
    on_battery: bool,
    /// Wait while a fullscreen app, a game or a presentation has the screen.
    pause_fullscreen: bool,
    /// Minutes after midnight, start and end; across midnight when the end comes first. None: all day.
    hours: Option<(u32, u32)>,
}

impl Policy {
    /// Why the backfill has to wait now, or None when it may run.
    fn blocker(&self) -> Option<&'static str> {
        let power = trovara_win::system::power();
        if power.saver {
            return Some("saver");
        }
        if power.on_battery && !self.on_battery {
            return Some("battery");
        }
        if self.pause_fullscreen && trovara_win::system::fullscreen_busy() {
            return Some("fullscreen");
        }
        if let Some((start, end)) = self.hours {
            let now = trovara_win::system::local_minutes();
            let inside = if start == end {
                true
            } else if start < end {
                now >= start && now < end
            } else {
                now >= start || now < end
            };
            if !inside {
                return Some("hours");
            }
        }
        None
    }
}

/// "22:00-06:00" → (1320, 360).
fn parse_hours(text: &str) -> Option<(u32, u32)> {
    let (start, end) = text.split_once('-')?;
    let minutes = |t: &str| {
        let (h, m) = t.trim().split_once(':')?;
        let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
        (h < 24 && m < 60).then_some(h * 60 + m)
    };
    Some((minutes(start)?, minutes(end)?))
}

pub fn serve(args: &[String]) -> Result<(), String> {
    let index_folder = crate::index::index_folder(args);
    let roots = indexer::roots_from(
        args,
        &[
            "--index",
            "--exclude",
            "--exclude-ext",
            "--linger",
            "--new-limit",
            "--on-battery",
            "--hours",
            "--memory-limit",
            "--time-limit",
            "--priority",
            "--pause-fullscreen",
            "--code-stretch",
        ],
    );
    if roots.is_empty() {
        return Err("index-service: which folders?".into());
    }
    let linger = Duration::from_secs_f64(
        value(args, "--linger")
            .and_then(|v| v.parse().ok())
            .unwrap_or(60.0),
    );
    let new_limit: usize = value(args, "--new-limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let watch = flag(args, "--watch");
    let policy = Policy {
        on_battery: value(args, "--on-battery").is_none_or(|v| v != "no"),
        pause_fullscreen: value(args, "--pause-fullscreen").is_none_or(|v| v != "no"),
        hours: value(args, "--hours").and_then(|v| parse_hours(&v)),
    };
    // auto (the default): normal priority on mains power, eco on battery. Any lowered priority slows the GPU work 3–8×
    // on Windows (the GPU's scheduling priority follows the process's, and a low thread that waits on the GPU wakes
    // late): an audio window 0.11 → 0.7–3.7 s, a video 0.8 → 8–9 s on an idle i5-13450HX + RTX 3050. Embedding is GPU
    // work that barely touches the CPU, so normal priority doesn't slow the person's programs; fullscreen apps and
    // games pause the backlog instead, and the battery gets eco. low (thread below normal), thread-background,
    // process-low, background (Windows' background mode) and eco (background + EcoQoS) can be chosen.
    let priority = value(args, "--priority").unwrap_or_else(|| "auto".into());
    JSON.store(true, Ordering::Relaxed);
    *LEVEL.lock().unwrap_or_else(|p| p.into_inner()) = if priority == "auto" {
        "eco".into()
    } else {
        priority.clone()
    };
    let background = priority != "normal";
    AUTO.store(priority == "auto", Ordering::Relaxed);
    if priority != "auto" && background {
        set_background(true);
    }
    apply_auto_priority();
    let memory_limit = value(args, "--memory-limit")
        .and_then(|v| v.parse::<u64>().ok())
        .map(|mb| mb << 20)
        .unwrap_or_else(crate::watchdog::default_memory_limit);
    let time_limit = value(args, "--time-limit")
        .and_then(|v| v.parse().ok())
        .map(Duration::from_secs_f64)
        .unwrap_or(crate::watchdog::DEFAULT_TIME_LIMIT);
    crate::watchdog::start(&index_folder, memory_limit, time_limit);

    let (indexer, notes) = match indexer::open(&index_folder, indexer::options_from(args)) {
        Ok(opened) => opened,
        Err(message) => {
            emit(json!({"event": "error", "message": message, "fatal": true}));
            return Err(message);
        }
    };
    emit(json!({"event": "start", "roots": roots, "watch": watch, "pid": std::process::id()}));
    for note in notes {
        emit(json!({"event": "note", "message": note}));
    }
    if flag(args, "--prune") {
        // Files under none of the given folders leave the index (the app passes all its folders).
        match indexer.store.prune(&roots) {
            Ok(0) => {}
            Ok(n) => emit(json!({"event": "pruned", "files": n})),
            Err(error) => emit(json!({"event": "error", "message": error.to_string()})),
        }
    }

    let shared = Arc::new(Shared {
        requests: Mutex::new(Requests {
            sync: true,
            backfill: flag(args, "--backfill"),
            ..Requests::default()
        }),
        wake: Condvar::new(),
    });
    {
        let shared = shared.clone();
        std::thread::spawn(move || read_commands(&shared, background));
    }
    let _watcher = if watch {
        Some(crate::service::watch(
            &roots,
            &index_folder,
            shared.clone(),
            indexer.is_code(),
        )?)
    } else {
        None
    };

    let mut service = Service {
        indexer,
        roots,
        shared,
        policy,
        new_limit,
        yield_at: Instant::now(),
        missing: Vec::new(),
        index_folder: index_folder.clone(),
        checkpointed: Instant::now(),
    };
    service.run(linger, watch);
    // While the watcher saw every change, the journal's past isn't needed: the next start reads from here.
    if crate::usn::installed() {
        let available = service.available_roots();
        crate::usn::checkpoint(&index_folder, &service.indexer.store, &available);
    }
    emit(json!({"event": "exit"}));
    Ok(())
}

struct Service {
    indexer: Indexer,
    roots: Vec<String>,
    shared: Arc<Shared>,
    policy: Policy,
    new_limit: usize,
    /// A sync asked for while files are being indexed waits until indexing has had about as long as the last sync
    /// took (2 s at the least): a folder that keeps changing must not keep indexing from ever reading a file.
    yield_at: Instant,
    /// The folders reported missing last time (said again only when it changes).
    missing: Vec<String>,
    index_folder: PathBuf,
    /// When the change journal's checkpoint last moved (it moves while idle, every 30 minutes).
    checkpointed: Instant,
}

impl Service {
    fn run(&mut self, linger: Duration, watch: bool) {
        let mut new_files: HashSet<i64> = HashSet::new();
        let mut first_sync = true;
        let models_there = trovara_llama::Models::complete();
        loop {
            if self.shared.with(|r| std::mem::take(&mut r.sync)) {
                if std::mem::take(&mut first_sync) {
                    self.catch_up(&mut new_files);
                } else {
                    self.sync(&mut new_files);
                }
            }
            if self.shared.with(|r| r.stop) {
                break;
            }
            if !models_there {
                // Nothing to embed with yet: file names are searchable; the app starts another helper after the download.
                self.emit_counts("waiting", json!({"for": "model"}));
                if watch && self.wait_for_work(None) {
                    continue;
                }
                break;
            }
            if self.shared.with(|r| r.paused) {
                self.emit_counts("waiting", json!({"for": "paused"}));
                if self.wait_for_work(None) {
                    continue;
                }
                break;
            }
            if !new_files.is_empty() {
                if !self.index(Some(&new_files)) {
                    break;
                }
                new_files = self.still_pending(&new_files);
                continue;
            }
            apply_auto_priority();
            let blocker = self.backfill_blocker();
            let pending = self.indexer.store.pending_count().unwrap_or(0);
            if pending > 0 && blocker.is_none() {
                if !self.index(None) {
                    break;
                }
                if self.shared.with(|r| r.sync) {
                    continue;
                }
            }
            let pending = self.indexer.store.pending_count().unwrap_or(0);
            let unfinished = self
                .indexer
                .store
                .unfinished()
                .map(|u| u.len())
                .unwrap_or(0);
            if pending == 0 && unfinished > 0 && self.backfill_blocker().is_none() {
                if !self.finish_long_files() {
                    break;
                }
                if self.shared.with(|r| r.sync) {
                    continue;
                }
            }
            match self.backfill_blocker() {
                Some(why) if pending > 0 || unfinished > 0 => {
                    self.emit_counts("waiting", json!({"for": why}))
                }
                _ => {
                    self.emit_counts("idle", json!({}));
                    if self.checkpointed.elapsed() > Duration::from_secs(30 * 60)
                        && crate::usn::installed()
                    {
                        let available = self.available_roots();
                        crate::usn::checkpoint(&self.index_folder, &self.indexer.store, &available);
                        self.checkpointed = Instant::now();
                    }
                }
            }
            // Keep the model warm a while; then free it, and leave unless watching.
            if self.indexer.model_running() {
                if self.wait_for_work(Some(linger)) {
                    continue;
                }
                self.indexer.close_model();
                emit(json!({"event": "note", "message": "model freed after idle"}));
            }
            if !watch {
                break;
            }
            if !self.wait_for_work(None) {
                break;
            }
        }
        self.indexer.close_model();
    }

    /// Why the backfill (and the rest of long files) has to wait, or None.
    fn backfill_blocker(&self) -> Option<&'static str> {
        let (allowed, full_speed) = self.shared.with(|r| (r.backfill, r.full_speed));
        if !allowed {
            return Some("backfill off");
        }
        if full_speed {
            return None; // the person asked for it now: power and hours don't hold it
        }
        self.policy.blocker()
    }

    /// The chosen folders that are there now; also what `pending` and the counts look at.
    pub fn available_roots(&mut self) -> Vec<String> {
        let missing: Vec<String> = self
            .roots
            .iter()
            .filter(|root| !Path::new(root).is_dir())
            .cloned()
            .collect();
        if missing != self.missing {
            emit(json!({"event": "missing", "folders": missing}));
            self.missing = missing.clone();
        }
        let available: Vec<String> = self
            .roots
            .iter()
            .filter(|root| !missing.contains(root))
            .cloned()
            .collect();
        self.indexer.store.set_scope(Some(available.clone()));
        available
    }

    /// The first sync of a start: with fast catch-up on, only the folders the change journal says changed since the
    /// last checkpoint (and whole roots where it can't say: no checkpoint yet, the journal reset, not an NTFS drive);
    /// without it, a full sync.
    fn catch_up(&mut self, new_files: &mut HashSet<i64>) {
        let started = Instant::now();
        let available = self.available_roots();
        // Code folders sync whole (what git ignores is decided from a repo's top; a code walk is quick).
        if self.indexer.is_code() {
            return self.sync(new_files);
        }
        let Some(volumes) =
            crate::usn::catch_up(&self.index_folder, &self.indexer.store, &available)
        else {
            return self.sync(new_files);
        };
        self.checkpointed = Instant::now();
        let mut whole: Vec<String> = Vec::new();
        let mut folders: Vec<String> = Vec::new();
        for root in &available {
            let key = trovara_core::store::path_key(root);
            let answer =
                crate::usn::volume_of(root).and_then(|v| volumes.iter().find(|a| a.volume == v));
            match answer.and_then(|a| a.folders.as_ref()) {
                Some(changed) => folders.extend(
                    changed
                        .iter()
                        .filter(|f| {
                            let k = trovara_core::store::path_key(f);
                            k == key || k.starts_with(&format!("{key}\\"))
                        })
                        .cloned(),
                ),
                None => whole.push(root.clone()),
            }
        }
        let mut combined = indexer::SyncSummary::default();
        let mut add = |summary: indexer::SyncSummary| {
            combined.found += summary.found;
            combined.added += summary.added;
            combined.changed += summary.changed;
            combined.moved += summary.moved;
            combined.removed += summary.removed;
            combined.unchanged += summary.unchanged;
            combined.queued.extend(summary.queued);
            for (why, n) in summary.skipped {
                *combined.skipped.entry(why).or_default() += n;
            }
        };
        let mut failed = None;
        if !whole.is_empty() {
            match self.indexer.sync(&whole) {
                Ok(summary) => add(summary),
                Err(message) => failed = Some(message),
            }
        }
        if !folders.is_empty() {
            match self.indexer.sync_folders(&available, &folders) {
                Ok(summary) => add(summary),
                Err(message) => failed = Some(message),
            }
        }
        if let Some(message) = failed {
            emit(
                json!({"event": "error", "message": format!("catch-up failed, syncing everything: {message}")}),
            );
            return self.sync(new_files);
        }
        combined.ms = started.elapsed().as_millis() as u64;
        if combined.queued.len() <= self.new_limit {
            new_files.extend(&combined.queued);
        }
        let mut fields = serde_json::to_value(&combined).unwrap_or(json!({}));
        fields["catch_up"] = json!({"changed_folders": folders.len(), "whole_roots": whole.len()});
        fields["new"] = json!(new_files.len());
        self.emit_counts("sync", fields);
        self.yield_at = Instant::now() + started.elapsed().max(Duration::from_secs(2));
    }

    fn sync(&mut self, new_files: &mut HashSet<i64>) {
        let started = Instant::now();
        let available = self.available_roots();
        match self.indexer.sync(&available) {
            Ok(summary) => {
                if summary.queued.len() <= self.new_limit {
                    new_files.extend(&summary.queued);
                }
                let mut fields = serde_json::to_value(&summary).unwrap_or(json!({}));
                fields["new"] = json!(new_files.len());
                self.emit_counts("sync", fields);
            }
            Err(message) => {
                emit(json!({"event": "error", "message": format!("sync failed: {message}")}))
            }
        }
        let took = started.elapsed().max(Duration::from_secs(2));
        self.yield_at = Instant::now() + took;
    }

    /// Indexes pending files (`only` some: the new ones). False when indexing has to stop (the model failed).
    fn index(&mut self, only: Option<&HashSet<i64>>) -> bool {
        let started = Instant::now();
        let is_new = only.is_some();
        let shared = self.shared.clone();
        let yield_at = self.yield_at;
        let policy = &self.policy;
        let mut should_stop = || {
            let (stop, paused, sync, backfill, full_speed) =
                shared.with(|r| (r.stop, r.paused, r.sync, r.backfill, r.full_speed));
            stop || paused
                || (!is_new && (!backfill || (!full_speed && policy.blocker().is_some())))
                || (Instant::now() >= yield_at && sync)
        };
        let mut on_event = |event: &Event| {
            let mut fields = serde_json::to_value(event).unwrap_or(json!({}));
            fields["event"] = json!("file");
            fields["new"] = json!(is_new);
            emit(fields);
        };
        match self.indexer.run(only, &mut should_stop, &mut on_event) {
            Ok(states) => {
                if !states.is_empty() {
                    emit(
                        json!({"event": "summary", "new": is_new, "states": states, "ms": started.elapsed().as_millis() as u64}),
                    );
                }
                true
            }
            Err(message) => {
                emit(json!({"event": "error", "message": message, "fatal": true}));
                false
            }
        }
    }

    fn finish_long_files(&mut self) -> bool {
        let started = Instant::now();
        let shared = self.shared.clone();
        let yield_at = self.yield_at;
        let policy = &self.policy;
        let mut should_stop = || {
            let (stop, paused, sync, backfill, full_speed) =
                shared.with(|r| (r.stop, r.paused, r.sync, r.backfill, r.full_speed));
            stop || paused
                || !backfill
                || (!full_speed && policy.blocker().is_some())
                || (Instant::now() >= yield_at && sync)
        };
        let mut on_event = |event: &Event| {
            let mut fields = serde_json::to_value(event).unwrap_or(json!({}));
            fields["event"] = json!("rest");
            emit(fields);
        };
        match self
            .indexer
            .finish_long_files(&mut should_stop, &mut on_event)
        {
            Ok(states) => {
                if !states.is_empty() {
                    emit(
                        json!({"event": "summary", "rest": true, "states": states, "ms": started.elapsed().as_millis() as u64}),
                    );
                }
                true
            }
            Err(message) => {
                emit(json!({"event": "error", "message": message, "fatal": true}));
                false
            }
        }
    }

    /// New files a run stopped before (a sync or stop came first): kept for the next round.
    fn still_pending(&self, files: &HashSet<i64>) -> HashSet<i64> {
        let pending: HashSet<i64> = self
            .indexer
            .store
            .pending(None)
            .map(|p| p.into_iter().map(|f| f.id).collect())
            .unwrap_or_default();
        files.intersection(&pending).copied().collect()
    }

    /// Waits for something to do: a command, a change, or (every 30 s) the power or hours letting the backfill run.
    /// `limit`: give up after that long. False: time's up, or told to stop.
    fn wait_for_work(&self, limit: Option<Duration>) -> bool {
        let deadline = limit.map(|l| Instant::now() + l);
        let mut requests = self
            .shared
            .requests
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        loop {
            if requests.stop {
                return false;
            }
            if requests.sync || std::mem::take(&mut requests.changed) {
                return true;
            }
            let has_work = self.indexer.store.pending_count().unwrap_or(0) > 0
                || self
                    .indexer
                    .store
                    .unfinished()
                    .map(|u| !u.is_empty())
                    .unwrap_or(false);
            if has_work
                && !requests.paused
                && requests.backfill
                && (requests.full_speed || self.policy.blocker().is_none())
            {
                return true;
            }
            let now = Instant::now();
            if deadline.is_some_and(|d| now >= d) {
                return false;
            }
            let step = deadline.map_or(Duration::from_secs(30), |d| {
                (d - now).min(Duration::from_secs(30))
            });
            let (next, _) = self
                .shared
                .wake
                .wait_timeout(requests, step)
                .unwrap_or_else(|p| p.into_inner());
            requests = next;
        }
    }

    /// An event with the index's counts: files, searchable, pending, vectors, unfinished long files.
    fn emit_counts(&self, event: &str, mut fields: Value) {
        fields["event"] = json!(event);
        if let Ok(counts) = self.indexer.store.counts() {
            let (mut files, mut searchable, mut pending) = (0i64, 0i64, 0i64);
            for states in counts.by_kind_and_state.values() {
                for (state, n) in states {
                    files += n;
                    if state == "done" || state == "partial" {
                        searchable += n;
                    }
                    if state == "pending" {
                        pending += n;
                    }
                }
            }
            fields["files"] = json!(files);
            fields["searchable"] = json!(searchable);
            fields["pending"] = json!(pending);
            fields["vectors"] = json!(counts.vectors);
            fields["unfinished"] = json!(counts.unfinished);
        }
        emit(fields);
    }
}

const BYTE_ORDER_MARK: char = '\u{FEFF}';

fn read_commands(shared: &Shared, background: bool) {
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        shared.with(|r| {
            r.changed = true;
            // .NET writers put a byte-order mark before the first line: "<BOM>pause" must still be "pause".
            match line.trim_start_matches(BYTE_ORDER_MARK).trim() {
                "sync" => r.sync = true,
                "stop" => r.stop = true,
                "backfill on" => r.backfill = true,
                "backfill off" => r.backfill = false,
                "pause" => r.paused = true,
                "resume" => r.paused = false,
                "full speed on" => {
                    r.full_speed = true;
                    FULL_SPEED.store(true, Ordering::Relaxed);
                    set_background(false);
                }
                "full speed off" => {
                    r.full_speed = false;
                    FULL_SPEED.store(false, Ordering::Relaxed);
                    if AUTO.load(Ordering::Relaxed) {
                        apply_auto_priority();
                    } else if background {
                        set_background(true);
                    }
                }
                _ => {}
            }
        });
        shared.signal();
    }
    shared.with(|r| r.stop = true); // the app went away
    shared.signal();
}

/// Watches the folders (ReadDirectoryChangesW, through `notify`) and asks for a sync once changes have been quiet for
/// half a second. Changes in Trovara's own data folder, hidden folders and folders the rules skip don't count: with the
/// home folder chosen, the index's own writes would otherwise ask for syncs forever.
fn watch(
    roots: &[String],
    index_folder: &Path,
    shared: Arc<Shared>,
    code: bool,
) -> Result<notify::RecommendedWatcher, String> {
    // Code is read again a minute after edits go quiet (an editor saves often; a half-written function isn't worth
    // embedding); anything else half a second after the last change.
    let quiet = if code { Duration::from_secs(60) } else { Duration::from_millis(500) };
    use notify::{RecursiveMode, Watcher};
    let ignore: Vec<String> = [index_folder.to_path_buf(), trovara_core::paths::data_root()]
        .iter()
        .map(|p| trovara_core::store::path_key(&p.to_string_lossy()))
        .collect();
    let (tx, rx) = std::sync::mpsc::channel::<PathBuf>();
    let mut watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
        if let Ok(event) = result {
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
            for path in event.paths {
                let _ = tx.send(path);
            }
        }
    })
    .map_err(|e| format!("can't watch: {e}"))?;
    for root in roots.iter().filter(|root| Path::new(root).is_dir()) {
        watcher
            .watch(Path::new(root), RecursiveMode::Recursive)
            .map_err(|e| format!("can't watch {root}: {e}"))?;
    }
    let root_keys: Vec<String> = roots
        .iter()
        .map(|r| trovara_core::store::path_key(r))
        .collect();
    std::thread::spawn(move || {
        // Whether a change is where the crawl goes: under a root, and no folder between the root and it is hidden or
        // skipped (only the part inside the root counts: a root may itself sit under a "Temp" or a ".cache").
        let counts = |path: &Path| {
            let key = trovara_core::store::path_key(&path.to_string_lossy());
            if ignore
                .iter()
                .any(|i| key == *i || key.starts_with(&format!("{i}\\")))
            {
                return false;
            }
            let Some(root) = root_keys
                .iter()
                .filter(|r| key == **r || key.starts_with(&format!("{r}\\")))
                .max_by_key(|r| r.len())
            else {
                return false;
            };
            !key[root.len()..]
                .split('\\')
                .filter(|part| !part.is_empty())
                .any(|part| {
                    part.starts_with('.')
                        || trovara_core::rules::SKIPPED_FOLDER_NAMES.contains(&part)
                        || code
                            && trovara_core::rules::CODE_SKIPPED_FOLDER_NAMES
                                .iter()
                                .any(|skipped| skipped.eq_ignore_ascii_case(part))
                })
        };
        while let Ok(path) = rx.recv() {
            if !counts(&path) {
                continue;
            }
            // Debounce: wait until the changes that count have been quiet that long (a build writing to target\ all
            // the while mustn't hold the sync off).
            let mut until = Instant::now() + quiet;
            loop {
                let now = Instant::now();
                if now >= until {
                    break;
                }
                match rx.recv_timeout(until - now) {
                    Ok(more) => {
                        if counts(&more) {
                            until = Instant::now() + quiet;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
            shared.with(|r| r.sync = true);
            shared.signal();
        }
    });
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hours() {
        assert_eq!(parse_hours("22:00-06:00"), Some((1320, 360)));
        assert_eq!(parse_hours("01:30-07:15"), Some((90, 435)));
        assert_eq!(parse_hours("25:00-06:00"), None);
        assert_eq!(parse_hours("nonsense"), None);
    }
}
