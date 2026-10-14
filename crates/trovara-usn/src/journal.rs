//! The journal reader and the scheduled task that runs it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FileIdType, GetFinalPathNameByHandleW, OpenFileById, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Ioctl::{
    FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL, READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0,
};
use windows::Win32::System::IO::DeviceIoControl;

/// The task's name: one per Windows user.
pub fn task_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
    let safe: String = user
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    format!("\\Trovara\\UsnReader-{safe}")
}

fn data_folder() -> PathBuf {
    let local = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    local.join("Trovara").join("usn")
}

pub fn install() -> i32 {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    let user = std::env::var("USERNAME").unwrap_or_default();
    let domain = std::env::var("USERDOMAIN").unwrap_or_default();
    let account = if domain.is_empty() {
        user
    } else {
        format!("{domain}\\{user}")
    };
    // On demand only (a start date in the past and no repeat), with the highest privileges the user has, as the user,
    // only while they're signed in (/IT: no password stored).
    let status = hidden("schtasks")
        .args([
            "/Create",
            "/F",
            "/TN",
            &task_name(),
            "/SC",
            "ONCE",
            "/ST",
            "00:00",
            "/SD",
            "01/01/2000",
            "/RL",
            "HIGHEST",
            "/IT",
            "/RU",
            &account,
            "/TR",
        ])
        .arg(format!("\"{}\" --serve", exe.display()))
        .status();
    match status {
        Ok(status) if status.success() => {
            println!("installed {}", task_name());
            0
        }
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!("can't run schtasks: {error}");
            1
        }
    }
}

pub fn uninstall() -> i32 {
    let status = hidden("schtasks")
        .args(["/Delete", "/F", "/TN", &task_name()])
        .status();
    match status {
        Ok(status) if status.success() => 0,
        Ok(status) => status.code().unwrap_or(1),
        Err(_) => 1,
    }
}

pub fn serve() -> i32 {
    let folder = data_folder();
    let request: Value = match std::fs::read_to_string(folder.join("usn-request.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
    {
        Some(request) => request,
        None => {
            write_answer(&folder, json!({"error": "no request"}));
            return 1;
        }
    };
    let mut volumes = Vec::new();
    for volume in request["volumes"].as_array().cloned().unwrap_or_default() {
        volumes.push(answer_volume(&volume));
    }
    write_answer(&folder, json!({"volumes": volumes}));
    0
}

fn write_answer(folder: &Path, answer: Value) {
    let _ = std::fs::create_dir_all(folder);
    let temporary = folder.join("usn-answer.tmp");
    if std::fs::write(&temporary, answer.to_string()).is_ok() {
        let _ = std::fs::rename(&temporary, folder.join("usn-answer.json"));
    }
}

/// Only drive letters: the request can't point the reader anywhere else.
fn valid_volume(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    (letter.is_ascii_alphabetic() && chars.next() == Some(':') && chars.next().is_none())
        .then_some(letter)
}

fn answer_volume(request: &Value) -> Value {
    let name = request["volume"].as_str().unwrap_or_default();
    let Some(letter) = valid_volume(name) else {
        return json!({"volume": name, "error": "not a drive letter"});
    };
    let roots: Vec<String> = request["roots"]
        .as_array()
        .map(|r| r.iter().filter_map(|v| v.as_str().map(key)).collect())
        .unwrap_or_default();
    match read_volume(
        letter,
        request["journal"].as_u64(),
        request["from"].as_i64(),
        &roots,
    ) {
        Ok(answer) => answer,
        Err(error) => json!({"volume": format!("{letter}:"), "error": error}),
    }
}

/// A path as compared here: lowercase, backslashes, no trailing backslash, no \\?\.
fn key(path: &str) -> String {
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    path.to_lowercase()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_string()
}

struct Volume(HANDLE);

impl Drop for Volume {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn read_volume(
    letter: char,
    journal: Option<u64>,
    from: Option<i64>,
    roots: &[String],
) -> Result<Value, String> {
    let volume = unsafe {
        CreateFileW(
            &HSTRING::from(format!(r"\\.\{letter}:")),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            Default::default(),
            None,
        )
    }
    .map_err(|e| {
        format!(
            "can't open the volume (needs administrator rights): {}",
            e.message()
        )
    })?;
    let volume = Volume(volume);
    let mut data = USN_JOURNAL_DATA_V0::default();
    let mut returned = 0u32;
    unsafe {
        DeviceIoControl(
            volume.0,
            FSCTL_QUERY_USN_JOURNAL,
            None,
            0,
            Some(&mut data as *mut _ as *mut _),
            std::mem::size_of::<USN_JOURNAL_DATA_V0>() as u32,
            Some(&mut returned),
            None,
        )
    }
    .map_err(|e| format!("no change journal on {letter}: ({})", e.message()))?;
    let base =
        json!({"volume": format!("{letter}:"), "journal": data.UsnJournalID, "next": data.NextUsn});
    let (Some(journal), Some(from)) = (journal, from) else {
        return Ok(base); // just where the journal is now
    };
    if journal != data.UsnJournalID || from < data.FirstUsn || from > data.NextUsn {
        let mut answer = base;
        answer["reset"] = json!(true);
        return Ok(answer);
    }

    // The parent folders of every change since `from`, then which of them are inside the roots.
    let mut parents: HashSet<u64> = HashSet::new();
    let mut start = from;
    let mut buffer = vec![0u8; 1 << 16];
    let mut read_records = 0u64;
    while start < data.NextUsn {
        let request = READ_USN_JOURNAL_DATA_V0 {
            StartUsn: start,
            ReasonMask: u32::MAX,
            ReturnOnlyOnClose: 0,
            Timeout: 0,
            BytesToWaitFor: 0,
            UsnJournalID: data.UsnJournalID,
        };
        let mut got = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                volume.0,
                FSCTL_READ_USN_JOURNAL,
                Some(&request as *const _ as *const _),
                std::mem::size_of::<READ_USN_JOURNAL_DATA_V0>() as u32,
                Some(buffer.as_mut_ptr().cast()),
                buffer.len() as u32,
                Some(&mut got),
                None,
            )
        };
        if ok.is_err() || got <= 8 {
            break;
        }
        let next = i64::from_le_bytes(buffer[..8].try_into().unwrap());
        for record in records(&buffer[8..got as usize]) {
            parents.insert(record.parent);
            if record.directory {
                parents.insert(record.file); // a folder that was itself created, renamed or deleted
            }
            read_records += 1;
        }
        if next <= start {
            break;
        }
        start = next;
    }
    let mut folders = Vec::new();
    let mut cache: HashMap<u64, Option<String>> = HashMap::new();
    for parent in parents {
        let path = cache
            .entry(parent)
            .or_insert_with(|| path_of(&volume, parent))
            .clone();
        if let Some(path) = path {
            let k = key(&path);
            if roots
                .iter()
                .any(|root| k == *root || k.starts_with(&format!("{root}\\")))
            {
                folders.push(path.strip_prefix(r"\\?\").unwrap_or(&path).to_string());
            }
        }
    }
    let mut answer = base;
    answer["reset"] = json!(false);
    answer["records"] = json!(read_records);
    answer["folders"] = json!(folders);
    Ok(answer)
}

struct Record {
    file: u64,
    parent: u64,
    directory: bool,
}

/// USN_RECORD_V2s (NTFS): length, version, file and parent references, …, attributes, name. Others are skipped.
fn records(bytes: &[u8]) -> Vec<Record> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 60 <= bytes.len() {
        let length = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        if length < 60 || at + length > bytes.len() {
            break;
        }
        let major = u16::from_le_bytes(bytes[at + 4..at + 6].try_into().unwrap());
        if major == 2 {
            let file = u64::from_le_bytes(bytes[at + 8..at + 16].try_into().unwrap());
            let parent = u64::from_le_bytes(bytes[at + 16..at + 24].try_into().unwrap());
            let attributes = u32::from_le_bytes(bytes[at + 52..at + 56].try_into().unwrap());
            out.push(Record {
                file,
                parent,
                directory: attributes & 0x10 != 0,
            });
        }
        at += length;
    }
    out
}

/// A folder's path from its file reference (None when it's gone).
fn path_of(volume: &Volume, reference: u64) -> Option<String> {
    let descriptor = FILE_ID_DESCRIPTOR {
        dwSize: std::mem::size_of::<FILE_ID_DESCRIPTOR>() as u32,
        Type: FileIdType,
        Anonymous: FILE_ID_DESCRIPTOR_0 {
            FileId: reference as i64,
        },
    };
    unsafe {
        let handle = OpenFileById(
            volume.0,
            &descriptor,
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            FILE_FLAG_BACKUP_SEMANTICS,
        )
        .ok()?;
        let mut buffer = vec![0u16; 1024];
        let length = GetFinalPathNameByHandleW(handle, &mut buffer, FILE_NAME_NORMALIZED);
        let _ = CloseHandle(handle);
        (length > 0 && (length as usize) < buffer.len())
            .then(|| String::from_utf16_lossy(&buffer[..length as usize]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_are_read_and_requests_checked() {
        // One USN_RECORD_V2 (a folder), 64 bytes with a 2-character name.
        let mut record = vec![0u8; 64];
        record[0..4].copy_from_slice(&64u32.to_le_bytes());
        record[4..6].copy_from_slice(&2u16.to_le_bytes());
        record[8..16].copy_from_slice(&7u64.to_le_bytes());
        record[16..24].copy_from_slice(&5u64.to_le_bytes());
        record[52..56].copy_from_slice(&0x10u32.to_le_bytes());
        let parsed = records(&record);
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            (parsed[0].file, parsed[0].parent, parsed[0].directory),
            (7, 5, true)
        );
        assert_eq!(valid_volume("c:"), Some('C'));
        assert_eq!(valid_volume(r"C:\Windows"), None);
        assert_eq!(valid_volume(r"\\server"), None);
        assert_eq!(key(r"\\?\C:\Users\Me\"), r"c:\users\me");
    }
}

/// A command that opens no console window (this program has none, so a console program it starts would get its own).
fn hidden(program: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut command = std::process::Command::new(program);
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    command
}
