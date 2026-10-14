//! Fast catch-up after a restart (TRD §9.3), the index service's side: asks `trovara-usn` (run by the scheduled task
//! the person allowed once) which folders inside the chosen ones changed since the last checkpoint, so a start syncs
//! those instead of crawling everything. Without the task (declined, a non-admin account), on a volume without a
//! journal (exFAT, network drives), or when the journal moved on, it's a normal full sync: same result, slower.
//!
//! The checkpoint (journal ID and position) is kept per volume in the index's meta, as "usn:C:" → "<journal> <next>".

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use trovara_core::IndexStore;

fn task_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
    let safe: String = user
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    format!("\\Trovara\\UsnReader-{safe}")
}

/// Where the request and the answer go: always %LOCALAPPDATA%\Trovara\usn (the task's command line is fixed when it's
/// installed, so it can't be told about an index somewhere else).
fn exchange_folder() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("Trovara").join("usn")
}

/// `trovara-usn.exe`, beside this helper.
fn reader_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe()
        .ok()?
        .with_file_name("trovara-usn.exe");
    exe.is_file().then_some(exe)
}

fn quiet(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

/// The task is installed (the person said yes once).
pub fn installed() -> bool {
    quiet(Command::new("schtasks").args(["/Query", "/TN", &task_name()]))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The reader the task runs (from its XML's `<Command>`), when there's a task.
pub fn task_program() -> Option<PathBuf> {
    let output = quiet(Command::new("schtasks").args(["/Query", "/TN", &task_name(), "/XML"])).output().ok()?;
    if !output.status.success() {
        return None;
    }
    // schtasks writes the XML in the console's code page or UTF-16; the path is between the tags either way.
    let text = if output.stdout.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = output.stdout[2..].chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    let start = text.find("<Command>")? + "<Command>".len();
    let end = start + text[start..].find("</Command>")?;
    Some(PathBuf::from(text[start..end].trim().trim_matches('"')))
}

/// The task is there and runs this copy's reader (a dev build's task runs the dev build's).
pub fn installed_here() -> bool {
    match (task_program(), reader_exe()) {
        (Some(task), Some(here)) => task.to_string_lossy().eq_ignore_ascii_case(&here.to_string_lossy()),
        _ => false,
    }
}

/// Installs or removes the task, elevated: Windows shows one UAC prompt. True when it worked.
pub fn set_installed(on: bool) -> Result<(), String> {
    let exe = reader_exe().ok_or("trovara-usn.exe isn't beside the helper")?;
    let argument = if on { "--install" } else { "--uninstall" };
    let script = format!(
        "$p = Start-Process -FilePath '{}' -ArgumentList '{argument}' -Verb RunAs -WindowStyle Hidden -Wait -PassThru; exit $p.ExitCode",
        exe.display().to_string().replace('\'', "''")
    );
    let status = quiet(Command::new("powershell").args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &script,
    ]))
    .status()
    .map_err(|e| e.to_string())?;
    if status.success() && installed() == on {
        Ok(())
    } else {
        Err(if on {
            "fast catch-up wasn't allowed (the administrator prompt was declined)".into()
        } else {
            "couldn't remove the task".into()
        })
    }
}

/// The drive letter a path is on ("C:"), when it's a local drive path.
pub fn volume_of(path: &str) -> Option<String> {
    let mut chars = path.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    (letter.is_ascii_alphabetic() && chars.next() == Some(':')).then(|| format!("{letter}:"))
}

/// What the journal says about one volume.
pub struct VolumeAnswer {
    pub volume: String,
    /// Changed folders inside the roots, or None: crawl this volume's roots whole (no checkpoint, journal reset, error).
    pub folders: Option<Vec<String>>,
}

/// Asks the reader. `checkpoints`: false asks only where each journal is now (a fresh checkpoint). Returns None when
/// the task isn't there or didn't answer in time.
fn ask(
    _index_folder: &Path,
    store: &IndexStore,
    roots: &[String],
    with_checkpoints: bool,
) -> Option<Vec<Value>> {
    if reader_exe().is_none() || !installed() {
        return None;
    }
    let mut volumes: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for root in roots {
        if let Some(volume) = volume_of(root) {
            volumes.entry(volume).or_default().push(root.clone());
        }
    }
    if volumes.is_empty() {
        return None;
    }
    let request: Vec<Value> = volumes
        .iter()
        .map(|(volume, roots)| {
            let mut entry = json!({"volume": volume, "roots": roots});
            if with_checkpoints {
                if let Some((journal, from)) = store
                    .meta(&format!("usn:{volume}"))
                    .ok()
                    .flatten()
                    .and_then(|v| {
                        let (j, n) = v.split_once(' ')?;
                        Some((j.parse::<u64>().ok()?, n.parse::<i64>().ok()?))
                    })
                {
                    entry["journal"] = json!(journal);
                    entry["from"] = json!(from);
                }
            }
            entry
        })
        .collect();
    let exchange = exchange_folder();
    let _ = std::fs::create_dir_all(&exchange);
    let request_path = exchange.join("usn-request.json");
    let answer_path = exchange.join("usn-answer.json");
    let _ = std::fs::remove_file(&answer_path);
    std::fs::write(&request_path, json!({"volumes": request}).to_string()).ok()?;
    let asked = SystemTime::now();
    if !quiet(Command::new("schtasks").args(["/Run", "/TN", &task_name()]))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return None;
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let fresh = std::fs::metadata(&answer_path)
            .and_then(|m| m.modified())
            .map(|t| t >= asked)
            .unwrap_or(false);
        if fresh {
            let answer: Value = std::fs::read_to_string(&answer_path)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())?;
            let _ = std::fs::remove_file(&request_path);
            return answer["volumes"].as_array().cloned();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// The changed folders per volume since the saved checkpoints, and the checkpoints moved to now. None: no fast
/// catch-up here (crawl everything).
pub fn catch_up(
    index_folder: &Path,
    store: &IndexStore,
    roots: &[String],
) -> Option<Vec<VolumeAnswer>> {
    let volumes = ask(index_folder, store, roots, true)?;
    let mut out = Vec::new();
    for volume in volumes {
        let name = volume["volume"].as_str().unwrap_or_default().to_string();
        if let (Some(journal), Some(next)) = (volume["journal"].as_u64(), volume["next"].as_i64()) {
            let _ = store.set_meta(&format!("usn:{name}"), Some(&format!("{journal} {next}")));
        }
        let folders = (volume["error"].is_null() && volume["reset"] == json!(false))
            .then(|| {
                volume["folders"].as_array().map(|f| {
                    f.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
            })
            .flatten();
        out.push(VolumeAnswer {
            volume: name,
            folders,
        });
    }
    Some(out)
}

/// Moves the checkpoints to now (while the watcher sees every change, the journal's past isn't needed).
pub fn checkpoint(index_folder: &Path, store: &IndexStore, roots: &[String]) -> bool {
    let Some(volumes) = ask(index_folder, store, roots, false) else {
        return false;
    };
    for volume in volumes {
        if let (Some(name), Some(journal), Some(next)) = (
            volume["volume"].as_str(),
            volume["journal"].as_u64(),
            volume["next"].as_i64(),
        ) {
            let _ = store.set_meta(&format!("usn:{name}"), Some(&format!("{journal} {next}")));
        }
    }
    true
}

/// `trovara-helper usn install | uninstall | status`: the setup step and Settings → Indexing call this.
pub fn command(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("install") => set_installed(true).map(|_| println!("fast catch-up is on")),
        // `--if-installed` (the uninstaller): nothing to do, and no permission asked, when it was never turned on.
        Some("uninstall") if args.iter().any(|a| a == "--if-installed") && !installed_here() => {
            println!("fast catch-up is off");
            Ok(())
        }
        Some("uninstall") => set_installed(false).map(|_| println!("fast catch-up is off")),
        Some("status") | None => {
            // "on" only when the task runs this copy's reader; another copy's (a dev build) shows where it is.
            match task_program() {
                Some(_) if installed_here() => println!("fast catch-up: on"),
                Some(other) => println!("fast catch-up: elsewhere ({})", other.display()),
                None => println!("fast catch-up: off"),
            }
            println!(
                "reader: {}",
                reader_exe().map_or("missing".into(), |p| p.display().to_string())
            );
            Ok(())
        }
        Some(other) => Err(format!("usn {other}? install, uninstall or status")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volumes() {
        assert_eq!(volume_of(r"C:\Users\me").as_deref(), Some("C:"));
        assert_eq!(volume_of(r"d:\photos").as_deref(), Some("D:"));
        assert_eq!(volume_of(r"\\nas\share"), None);
    }
}
