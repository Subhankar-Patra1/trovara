//! Ends the indexing helper when reading one file runs away (TRD §10.4). A decoder can grow without end on a bad file,
//! and nothing stops code like that from outside but ending the process. Every second the watchdog looks at this
//! process's memory and at how long the file being read has taken. Past the memory limit, or past the time limit on
//! one file, it writes down the file and why (`stopped-file.json` beside the index) and exits with `EXIT_CODE`: the app
//! starts a new helper, which marks that file as one that couldn't be read and goes on.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The exit status that says "a file was stopped: start a new helper" (EX_TEMPFAIL, as the reference).
pub const EXIT_CODE: i32 = 75;

static NOW_READING: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// The file being read now (None when between files).
pub fn reading(path: Option<&str>) {
    *NOW_READING.lock().unwrap_or_else(|p| p.into_inner()) =
        path.map(|p| (p.to_string(), Instant::now()));
}

/// Half the PC's memory, but at least 4 GB and at most 8 GB: indexing peaks at 1–3 GB on the biggest files.
pub fn default_memory_limit() -> u64 {
    (trovara_win::system::physical_memory() / 2).clamp(4 << 30, 8 << 30)
}

/// Reading one file never takes this long: an hour of 4K video is minutes, a long book is read 30 pages a step.
pub const DEFAULT_TIME_LIMIT: Duration = Duration::from_secs(15 * 60);

pub fn note_path(index_folder: &Path) -> PathBuf {
    index_folder.join("stopped-file.json")
}

pub fn start(index_folder: &Path, memory_limit: u64, time_limit: Duration) {
    let note = note_path(index_folder);
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        let reading = NOW_READING
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if let Some(used) = trovara_win::system::memory_used() {
            if used > memory_limit {
                stop(
                    &note,
                    reading.as_ref().map(|r| r.0.as_str()),
                    &format!("needed more than {} of memory to read", size(memory_limit)),
                );
            }
        }
        if let Some((path, since)) = &reading {
            if since.elapsed() > time_limit {
                let limit = if time_limit.as_secs() >= 120 {
                    format!("{} minutes", time_limit.as_secs() / 60)
                } else {
                    format!("{} seconds", time_limit.as_secs())
                };
                stop(
                    &note,
                    Some(path),
                    &format!("took more than {limit} to read"),
                );
            }
        }
    });
}

fn stop(note: &Path, path: Option<&str>, why: &str) -> ! {
    let json = serde_json::json!({"path": path, "why": why}).to_string();
    let temporary = note.with_extension("tmp");
    if std::fs::write(&temporary, &json).is_ok() {
        let _ = std::fs::rename(&temporary, note);
    }
    crate::service::emit(
        serde_json::json!({"event": "error", "message": format!("watchdog: {why}: {}", path.unwrap_or("no file being read"))}),
    );
    eprintln!("watchdog: {why}: {}", path.unwrap_or("no file being read"));
    std::process::exit(EXIT_CODE);
}

/// The note an earlier helper left: (path, why). Removed as it's read.
pub fn take_note(index_folder: &Path) -> Option<(Option<String>, String)> {
    let path = note_path(index_folder);
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some((
        value["path"].as_str().map(str::to_string),
        value["why"]
            .as_str()
            .unwrap_or("couldn't be read")
            .to_string(),
    ))
}

fn size(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.0} GB", bytes as f64 / (1u64 << 30) as f64)
    } else {
        format!("{} MB", bytes >> 20)
    }
}
