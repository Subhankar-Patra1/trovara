//! `trovara-helper download-model`: fetches EmbeddingGemma 2's two files at the pinned revision into the Models folder
//! (TRD §12.4). Resumes a partial download (`name.part`, HTTP ranges), checks the SHA-256 while writing, and gives a
//! file its final name only once it matches, so "the file is there" means "the file is right". A copy already in the
//! Hugging Face cache, or in a folder the user picked (`--from`, the app's "Use Files…"), is checked and copied
//! instead.
//!
//! Progress goes to stdout: human lines, or with `--json` one event per line for the app:
//!   {"event":"file","name":…,"bytes":…,"total":…}  {"event":"progress","name":…,"bytes":…,"total":…}
//!   {"event":"verified","name":…,"source":"download|cache|folder"}  {"event":"done"}  {"event":"error","message":…}

use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use trovara_core::model_files::{self, ModelFile};

pub struct Options {
    pub json: bool,
    pub folder: PathBuf,
    /// Files the user downloaded elsewhere (on another network).
    pub from: Option<PathBuf>,
}

pub fn run(options: &Options) -> Result<(), String> {
    std::fs::create_dir_all(&options.folder)
        .map_err(|error| format!("can't create {}: {error}", options.folder.display()))?;
    for file in model_files::ALL {
        let target = options.folder.join(file.name);
        if target.is_file() {
            continue;
        }
        // A copy the user picked, or one in the Hugging Face cache, at the right size and hash.
        let candidates = options
            .from
            .iter()
            .map(|folder| (folder.join(file.name), "folder"))
            .chain(
                model_files::hugging_face_snapshot()
                    .map(|folder| (folder.join(file.name), "cache")),
            );
        let mut done = false;
        for (path, source) in candidates {
            if std::fs::metadata(&path).map(|meta| meta.len()).ok() != Some(file.bytes) {
                continue;
            }
            say(
                options,
                &format!("checking {}", path.display()),
                serde_json::json!({"event": "checking", "name": file.name}),
            );
            if sha256(&path)? == file.sha256 {
                copy_verified(&path, &target)?;
                verified(options, file, source);
                done = true;
                break;
            }
        }
        if done {
            continue;
        }
        if options.from.is_some() {
            return Err(format!(
                "{} isn't in the chosen folder at the right size and checksum",
                file.name
            ));
        }
        download(options, file, &target)?;
        verified(options, file, "download");
    }
    say(
        options,
        "the model is ready",
        serde_json::json!({"event": "done"}),
    );
    Ok(())
}

fn download(options: &Options, file: ModelFile, target: &Path) -> Result<(), String> {
    let part = target.with_file_name(format!("{}.part", file.name));
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("Trovara/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(30))
        .timeout(None)
        .build()
        .map_err(|error| error.to_string())?;
    let mut attempts = 0;
    loop {
        attempts += 1;
        match download_once(options, &client, file, &part) {
            Ok(()) => break,
            Err(Failure::Retry(message)) if attempts < 5 => {
                say(
                    options,
                    &format!("{message}; retrying"),
                    serde_json::json!({"event": "retry", "name": file.name, "message": message}),
                );
                std::thread::sleep(Duration::from_secs(2 * attempts));
            }
            Err(Failure::Retry(message) | Failure::Fatal(message)) => return Err(message),
        }
    }
    // The whole file is there: check it before it gets its name.
    let digest = sha256(&part)?;
    if digest != file.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(format!(
            "{} downloaded with the wrong checksum ({digest}); it was deleted, try again",
            file.name
        ));
    }
    std::fs::rename(&part, target)
        .map_err(|error| format!("can't rename {}: {error}", part.display()))
}

enum Failure {
    Retry(String),
    Fatal(String),
}

fn download_once(
    options: &Options,
    client: &reqwest::blocking::Client,
    file: ModelFile,
    part: &Path,
) -> Result<(), Failure> {
    let mut have = std::fs::metadata(part).map(|meta| meta.len()).unwrap_or(0);
    if have > file.bytes {
        let _ = std::fs::remove_file(part);
        have = 0;
    }
    if have == file.bytes {
        return Ok(());
    }
    let mut request = client.get(file.url());
    if have > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let mut response = request
        .send()
        .map_err(|error| Failure::Retry(format!("can't reach Hugging Face: {error}")))?;
    let status = response.status();
    if status == reqwest::StatusCode::OK {
        have = 0; // the server sent the whole file: start over
    } else if status != reqwest::StatusCode::PARTIAL_CONTENT {
        let message = format!("Hugging Face answered {status} for {}", file.name);
        return Err(if status.is_server_error() {
            Failure::Retry(message)
        } else {
            Failure::Fatal(message)
        });
    }
    let mut out = OpenOptions::new()
        .create(true)
        .write(true)
        .append(have > 0)
        .truncate(have == 0)
        .open(part)
        .map_err(|error| Failure::Fatal(format!("can't write {}: {error}", part.display())))?;
    say(
        options,
        &format!("downloading {} ({} MB)", file.name, file.bytes / 1_000_000),
        serde_json::json!({"event": "file", "name": file.name, "bytes": have, "total": file.bytes}),
    );
    let mut buffer = vec![0u8; 1 << 20];
    let mut last = Instant::now();
    loop {
        let read = response
            .read(&mut buffer)
            .map_err(|error| Failure::Retry(format!("the download stopped: {error}")))?;
        if read == 0 {
            break;
        }
        out.write_all(&buffer[..read])
            .map_err(|error| Failure::Fatal(format!("can't write {}: {error}", part.display())))?;
        have += read as u64;
        if last.elapsed() >= Duration::from_millis(250) {
            last = Instant::now();
            progress(options, file, have);
        }
    }
    out.flush()
        .map_err(|error| Failure::Fatal(error.to_string()))?;
    progress(options, file, have);
    if have != file.bytes {
        return Err(Failure::Retry(format!(
            "{} ended at {have} of {} bytes",
            file.name, file.bytes
        )));
    }
    Ok(())
}

/// The SHA-256 as lowercase hex, read in 8 MB pieces.
pub fn sha256(path: &Path) -> Result<String, String> {
    let file =
        File::open(path).map_err(|error| format!("can't read {}: {error}", path.display()))?;
    let mut reader = BufReader::with_capacity(8 << 20, file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 8 << 20];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("can't read {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Copies a checked file in under a temporary name, then gives it its final one.
fn copy_verified(source: &Path, target: &Path) -> Result<(), String> {
    let temporary = target.with_extension("copying");
    std::fs::copy(source, &temporary)
        .map_err(|error| format!("can't copy {}: {error}", source.display()))?;
    std::fs::rename(&temporary, target)
        .map_err(|error| format!("can't rename {}: {error}", temporary.display()))
}

fn progress(options: &Options, file: ModelFile, bytes: u64) {
    if options.json {
        println!(
            "{}",
            serde_json::json!({"event": "progress", "name": file.name, "bytes": bytes, "total": file.bytes})
        );
    } else {
        print!(
            "\r  {:5.1}%  {} / {} MB   ",
            bytes as f64 * 100.0 / file.bytes as f64,
            bytes / 1_000_000,
            file.bytes / 1_000_000
        );
        let _ = std::io::stdout().flush();
    }
}

fn verified(options: &Options, file: ModelFile, source: &str) {
    say(
        options,
        &format!("{} verified ({source})", file.name),
        serde_json::json!({"event": "verified", "name": file.name, "source": source}),
    );
}

fn say(options: &Options, line: &str, event: serde_json::Value) {
    if options.json {
        println!("{event}");
    } else {
        println!("\n{line}");
    }
}
