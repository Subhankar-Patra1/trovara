//! Where Trovara keeps its own files (TRD §4.1): `%LOCALAPPDATA%\Trovara`, or `Data\` beside the executable in
//! portable mode (a `portable` file next to the exe, or `TROVARA_DATA` for dev runs).

use std::path::PathBuf;

pub const APP_FOLDER: &str = "Trovara";

/// The user's home folder.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// The data root: the model, the indexes, the logs and the settings.
pub fn data_root() -> PathBuf {
    if let Some(chosen) = std::env::var_os("TROVARA_DATA").filter(|value| !value.is_empty()) {
        return PathBuf::from(chosen);
    }
    if let Some(folder) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
    {
        if folder.join("portable").is_file() {
            return folder.join("Data");
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|value| !value.is_empty()) {
        return PathBuf::from(local).join(APP_FOLDER);
    }
    home()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("AppData")
        .join("Local")
        .join(APP_FOLDER)
}

pub fn models() -> PathBuf {
    data_root().join("Models")
}

pub fn index() -> PathBuf {
    data_root().join("index")
}

pub fn logs() -> PathBuf {
    data_root().join("logs")
}

pub fn settings_file() -> PathBuf {
    data_root().join("settings.json")
}

pub const MAIN_INDEX_FILE: &str = "index.sqlite";
pub const CODE_INDEX_FILE: &str = "code.sqlite";
