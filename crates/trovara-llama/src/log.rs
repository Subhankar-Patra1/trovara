//! llama.cpp's log is process-wide: warnings and errors go to one file (or stderr); the rest is dropped (it logs a
//! lot at load time).

use std::ffi::{c_char, c_int, c_void, CStr};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use crate::ffi;

struct State {
    file: Option<File>,
    /// CONT continues the previous message, at its level.
    level: c_int,
}

static STATE: Mutex<State> = Mutex::new(State {
    file: None,
    level: 2,
});

/// Where llama.cpp's warnings and Trovara's embedder lines go: a file (appended), or stderr when None.
pub fn send_to(path: Option<&Path>) {
    let mut state = STATE.lock().unwrap();
    state.file = path.and_then(|path| OpenOptions::new().create(true).append(true).open(path).ok());
}

/// One of our own lines.
pub(crate) fn line(message: &str) {
    let mut state = STATE.lock().unwrap();
    let text = format!("[llama.cpp] {message}\n");
    match state.file.as_mut() {
        Some(file) => {
            let _ = file.write_all(text.as_bytes());
        }
        None => eprint!("{text}"),
    }
}

pub(crate) unsafe extern "C" fn callback(
    level: c_int,
    text: *const c_char,
    _user_data: *mut c_void,
) {
    if text.is_null() {
        return;
    }
    let text = CStr::from_ptr(text).to_string_lossy();
    let Ok(mut state) = STATE.lock() else { return };
    if level != ffi::GGML_LOG_LEVEL_CONT {
        state.level = level;
    }
    // Warnings and errors only; TROVARA_LLAMA_LOG=all keeps everything (buffer sizes, devices: a developer's look).
    static ALL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let all = *ALL.get_or_init(|| std::env::var("TROVARA_LLAMA_LOG").is_ok_and(|v| v == "all"));
    if state.level < ffi::GGML_LOG_LEVEL_WARN && !all {
        return;
    }
    match state.file.as_mut() {
        Some(file) => {
            let _ = file.write_all(text.as_bytes());
        }
        None => eprint!("{text}"),
    }
}
