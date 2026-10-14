//! What the app is told at launch: mostly nothing (the settings say it all); dev runs and tests point it elsewhere.

use std::path::PathBuf;

/// The panel's hotkey unless the person chose another.
pub const DEFAULT_HOTKEY: &str = "Control+Shift+Space";

pub struct Options {
    /// Trovara's helper, `trovara-helper.exe` (default: beside this program, where the installer puts it).
    pub helper: PathBuf,
    /// The index folder. None: the helper's default (%LOCALAPPDATA%\Trovara\index).
    pub index: Option<PathBuf>,
    /// Where the model files are (TROVARA_MODELS for the helpers); None: the helper looks where it always does.
    pub models: Option<PathBuf>,
    /// The folders to search and keep indexed, instead of the settings' (dev runs; setup is then skipped).
    pub roots: Vec<PathBuf>,
    /// The hotkey, instead of the settings'.
    pub hotkey: Option<String>,
    /// The settings file, instead of %LOCALAPPDATA%\Trovara\settings.json (a fresh first launch in tests).
    pub settings: Option<PathBuf>,
    /// The folders setup offers, instead of the usual ones (Documents, Pictures…): dev runs never offer real folders.
    pub offered: Option<Vec<PathBuf>>,
    /// Started at sign-in (the autostart entry passes it): stay in the tray.
    pub background: bool,
    /// Debug runs: show the panel at launch and keep it up when it loses focus.
    pub debug_show: bool,
    /// Debug runs: setup clicks itself through (every offered folder, then Start) without showing.
    pub debug_setup: bool,
    /// Debug runs: the panel types this query in when it loads and prints what it showed.
    pub debug_query: Option<String>,
    /// Debug runs: open this window at launch ("window", "settings", "setup"); "window:QUERY" searches for QUERY.
    pub debug_open: Option<String>,
    /// Update tests: check for an update at launch and install it if there is one (prints what happened).
    pub debug_update: bool,
    /// Debug runs: quit once the report is in.
    pub debug_quit: bool,
}

impl Options {
    pub fn from_args() -> Result<Options, String> {
        let mut options = Options {
            helper: default_helper(),
            index: None,
            models: None,
            roots: Vec::new(),
            hotkey: None,
            settings: None,
            offered: None,
            background: false,
            debug_show: false,
            debug_setup: false,
            debug_query: None,
            debug_open: None,
            debug_update: false,
            debug_quit: false,
        };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "--helper" => options.helper = value()?.into(),
                "--index" => options.index = Some(value()?.into()),
                "--models" => options.models = Some(value()?.into()),
                // Separated as PATH is (";").
                "--roots" => options.roots = std::env::split_paths(&value()?).collect(),
                "--hotkey" => options.hotkey = Some(value()?),
                "--settings" => options.settings = Some(value()?.into()),
                "--offer" => options.offered = Some(std::env::split_paths(&value()?).collect()),
                "--background" | "--autostart" => options.background = true,
                "--debug-show" => options.debug_show = true,
                "--debug-setup" => options.debug_setup = true,
                "--debug-query" => options.debug_query = Some(value()?),
                "--debug-open" => options.debug_open = Some(value()?),
                "--debug-update" => options.debug_update = true,
                "--debug-quit" => options.debug_quit = true,
                _ => return Err(format!("unknown argument {arg}")),
            }
        }
        Ok(options)
    }

    /// Trovara's data folder: %LOCALAPPDATA%\Trovara (TROVARA_DATA in dev runs), as the helper's.
    pub fn data_root(&self) -> PathBuf {
        if let Some(chosen) = std::env::var_os("TROVARA_DATA").filter(|v| !v.is_empty()) {
            return PathBuf::from(chosen);
        }
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("Trovara")
    }

    /// The index folder the helpers use: the one given, else the helper's default.
    pub fn index_folder(&self) -> PathBuf {
        self.index.clone().unwrap_or_else(|| self.data_root().join("index"))
    }
}

/// The helper beside this program, or in its `helper` folder.
fn default_helper() -> PathBuf {
    let name = "trovara-helper.exe";
    let Some(folder) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(PathBuf::from)) else {
        return PathBuf::from(name);
    };
    let inside = folder.join("helper").join(name);
    if inside.is_file() {
        inside
    } else {
        folder.join(name)
    }
}
