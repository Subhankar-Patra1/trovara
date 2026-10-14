//! `trovara-usn`: reads the NTFS change journal for Trovara's index service (TRD §9.3). Reading the journal needs
//! administrator rights, so this small program does only that, run on demand by a scheduled task the person approves
//! once (one UAC prompt); the index service itself never runs elevated.
//!
//!   trovara-usn --install     (elevated) registers \Trovara\UsnReader-<user>, run on demand with highest privileges
//!   trovara-usn --uninstall   (elevated) removes it
//!   trovara-usn --serve       (the task) answers the request in %LOCALAPPDATA%\Trovara\usn\usn-request.json
//!
//! (The request and the answer always live in %LOCALAPPDATA%\Trovara\usn, wherever the index is: the task's command
//! line is fixed when it's installed.)
//! Request: {"volumes": [{"volume": "C:", "journal": 123, "from": 456, "roots": ["C:\\Users\\me\\Pictures"]}]}
//!   ("journal"/"from" left out: just say where the journal is now).
//! Answer, written beside it as usn-answer.json: {"volumes": [{"volume": "C:", "journal": …, "next": …, "reset": bool,
//!   "folders": ["C:\\Users\\me\\Pictures\\Trip", …]}]}: the folders inside the roots where something changed since
//!   `from`. "reset": the journal was recreated or has moved past `from`: crawl everything.
//! It reads names only to tell which folders changed, never file contents, and reports only folders inside the roots.

#[cfg(windows)]
mod journal;

fn main() {
    // DLLs from this program's folder and System32 only (TRD §14): it runs with administrator rights.
    // SAFETY: a process-wide setting with valid flags, made before anything else loads.
    unsafe {
        use windows::Win32::System::LibraryLoader::{
            SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
        };
        let _ = SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
    }
    let command = std::env::args().nth(1).unwrap_or_default();
    #[cfg(windows)]
    let code = match command.as_str() {
        "--install" => journal::install(),
        "--uninstall" => journal::uninstall(),
        "--serve" => journal::serve(),
        _ => {
            eprintln!("usage: trovara-usn --install | --uninstall | --serve");
            64
        }
    };
    #[cfg(not(windows))]
    let code = {
        eprintln!("trovara-usn {command}: Windows only");
        64
    };
    std::process::exit(code);
}
