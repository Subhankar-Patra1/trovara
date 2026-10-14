//! What the person chose (PRD §6.12): the folders Trovara searches and what it skips in them, the hotkey, how indexing
//! behaves, and whether setup is done. %LOCALAPPDATA%\Trovara\settings.json.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// The folders searched and kept indexed.
    pub folders: Vec<String>,
    /// Folders inside them that aren't.
    pub skipped: Vec<String>,
    /// File extensions left out ("heic", "txt").
    pub skip_types: Vec<String>,
    /// The panel's hotkey (an accelerator, "Control+Shift+Space"); empty: the default.
    pub hotkey: String,
    /// Setup is done: the app starts in the tray.
    pub set_up: bool,
    /// Index the backlog on battery too (new files always are).
    pub index_on_battery: bool,
    /// "Index only at set hours": "22:00-06:00", or empty for any time.
    pub hours: String,
    /// The backlog waits while a fullscreen app, a game or a presentation has the screen.
    pub pause_fullscreen: bool,
    /// "auto" (normal on mains power, eco on battery), "normal", "low", "eco".
    pub priority: String,
    /// Start Trovara when Windows starts (so the hotkey always works).
    pub launch_at_login: bool,
    /// Daily check for a new version (nothing about files is sent).
    pub check_updates: bool,
    /// Indexing paused from the tray (kept across restarts).
    pub paused: bool,
    /// Code search (PRD §6.9): off until turned on.
    pub code: CodeSettings,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct CodeSettings {
    pub on: bool,
    /// The folders with code (repos, or folders of them).
    pub folders: Vec<String>,
    /// Folders inside them that aren't read.
    pub skipped: Vec<String>,
    /// The editor code opens in ("vscode", "cursor", "notepad++"…); empty: Trovara picks an installed one.
    pub editor: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            folders: Vec::new(),
            skipped: Vec::new(),
            skip_types: Vec::new(),
            hotkey: String::new(),
            set_up: false,
            index_on_battery: true,
            hours: String::new(),
            pause_fullscreen: true,
            priority: "auto".into(),
            launch_at_login: true,
            check_updates: true,
            paused: false,
            code: CodeSettings::default(),
        }
    }
}

impl Settings {
    /// The file's settings, or the defaults when there's none (or it can't be read).
    pub fn load(path: &Path) -> Settings {
        std::fs::read(path).ok().and_then(|data| serde_json::from_slice(&data).ok()).unwrap_or_default()
    }

    /// Written whole to a temporary file first, so a crash never leaves half of it.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(folder) = path.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let temporary = path.with_extension("json.new");
        std::fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(temporary, path)
    }
}
