// Trovara's app (TRD §10): the tray icon, the hotkey, the search panel and the search window, setup and settings.
// The work is the helper's, `trovara-helper`, run as processes speaking JSON lines (helper.rs): `search-service`
// answers the panel, `index-service --watch` keeps the index current and watches the folders, `download-model`
// fetches the model, `estimate` sizes up folders, `usn` turns fast catch-up on or off.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod editors;
mod helper;
mod options;
mod settings;
mod updates;
mod win;

use helper::{IndexService, Launch, ModelDownload, SearchService};
use options::{Options, DEFAULT_HOTKEY};
use serde_json::{json, Value};
use settings::Settings;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
    WindowEvent, Wry,
};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_opener::OpenerExt;

const PANEL_WIDTH: f64 = 780.0;
const FIELD_HEIGHT: f64 = 56.0;
/// Both model files together (TRD §12.4), for one progress bar over the two downloads.
const MODEL_BYTES: f64 = 864_676_480.0;
const MODEL_FILES: [&str; 2] = ["embeddinggemma-2-Q8_0.gguf", "mmproj-embeddinggemma-2-Q8_0.gguf"];
/// The index helper is started again after it stops on its own (a crash, the watchdog), but not more than this often.
const RESTARTS_PER_TEN_MINUTES: usize = 5;

/// What the commands and events share.
struct Shell {
    options: Options,
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    search: SearchService,
    index: IndexService,
    /// The code index's helper (`index-service --code`), when code search is on.
    code_index: IndexService,
    model: ModelDownload,
    /// The model for setup, settings and the tray: {"state": "checking|downloading|ready|failed", …}.
    model_state: Mutex<Value>,
    /// Bytes in hand per model file while downloading.
    model_bytes: Mutex<HashMap<String, (u64, u64)>>,
    /// The running index helpers started with the model there (one started without it never loads it).
    index_has_model: AtomicBool,
    code_has_model: AtomicBool,
    /// The main index's status line, for the tray to go back to after code indexing has said its piece.
    main_status: Mutex<String>,
    /// The hotkey registered now (empty: none is).
    hotkey_now: Mutex<String>,
    tray: Mutex<Option<TrayItems>>,
    /// When the search service last reloaded each index (while indexing, at most every few seconds): main, code.
    reloaded: Mutex<[Instant; 2]>,
    /// When each index helper last had to be started again: main, code.
    restarts: Mutex<[Vec<Instant>; 2]>,
    quitting: AtomicBool,
    /// Debug runs with setup: the panel's query waits until the first index is in.
    debug_query_sent: AtomicBool,
    logs: PathBuf,
}

struct TrayItems {
    status: MenuItem<Wry>,
    search: MenuItem<Wry>,
    pause: MenuItem<Wry>,
    /// "Check for Updates…", then "Restart to Update to 1.0.1" once one is downloaded.
    update: MenuItem<Wry>,
}

impl Shell {
    /// The folders searched: the launch arguments' in dev runs, else the settings'.
    fn folders(&self) -> Vec<String> {
        if !self.options.roots.is_empty() {
            return self.options.roots.iter().map(|root| root.display().to_string()).collect();
        }
        self.settings.lock().unwrap().folders.clone()
    }

    fn is_set_up(&self) -> bool {
        !self.options.roots.is_empty() || self.settings.lock().unwrap().set_up
    }

    /// The hotkey wanted: the launch argument's, the person's, or the default.
    fn hotkey(&self) -> String {
        let chosen = self.settings.lock().unwrap().hotkey.clone();
        self.options.hotkey.clone().unwrap_or(if chosen.is_empty() { DEFAULT_HOTKEY.to_string() } else { chosen })
    }

    fn env(&self) -> Vec<(String, String)> {
        self.options.models.iter().map(|models| ("TROVARA_MODELS".to_string(), models.display().to_string())).collect()
    }

    fn launch(&self, args: Vec<String>, log: &str) -> Launch {
        Launch { program: self.options.helper.clone(), args, env: self.env(), log: Some(self.logs.join(log)) }
    }

    fn index_arguments(&self) -> Vec<String> {
        self.options.index.iter().flat_map(|index| ["--index".to_string(), index.display().to_string()]).collect()
    }

    /// `index-service` for the folders and settings as they are now. The same arguments as the running helper's: it
    /// carries on (and syncs); others: it's started over with them.
    fn index_launch(&self) -> Launch {
        let settings = self.settings.lock().unwrap().clone();
        let yes_no = |on: bool| if on { "yes" } else { "no" }.to_string();
        let mut args = vec!["index-service".to_string(), "--watch".into(), "--backfill".into(), "--prune".into()];
        args.extend(self.index_arguments());
        let priority = if settings.priority.is_empty() { "auto".to_string() } else { settings.priority.clone() };
        args.extend(["--priority".into(), priority]);
        args.extend(["--on-battery".into(), yes_no(settings.index_on_battery)]);
        args.extend(["--pause-fullscreen".into(), yes_no(settings.pause_fullscreen)]);
        if !settings.hours.trim().is_empty() {
            args.extend(["--hours".into(), settings.hours.trim().to_string()]);
        }
        for folder in &settings.skipped {
            args.extend(["--exclude".to_string(), folder.clone()]);
        }
        for extension in &settings.skip_types {
            args.extend(["--exclude-ext".to_string(), extension.clone()]);
        }
        args.extend(self.folders());
        self.launch(args, "index-service.stderr.log")
    }

    /// `index-service --code` for the code folders, or None while code search is off (or has no folders).
    fn code_launch(&self) -> Option<Launch> {
        let settings = self.settings.lock().unwrap().clone();
        if !settings.code.on || settings.code.folders.is_empty() {
            return None;
        }
        let yes_no = |on: bool| if on { "yes" } else { "no" }.to_string();
        let mut args = vec!["index-service".to_string(), "--code".into(), "--watch".into(), "--backfill".into(), "--prune".into()];
        args.extend(self.index_arguments());
        let priority = if settings.priority.is_empty() { "auto".to_string() } else { settings.priority.clone() };
        args.extend(["--priority".into(), priority]);
        args.extend(["--on-battery".into(), yes_no(settings.index_on_battery)]);
        args.extend(["--pause-fullscreen".into(), yes_no(settings.pause_fullscreen)]);
        if !settings.hours.trim().is_empty() {
            args.extend(["--hours".into(), settings.hours.trim().to_string()]);
        }
        for folder in &settings.code.skipped {
            args.extend(["--exclude".to_string(), folder.clone()]);
        }
        args.extend(settings.code.folders.iter().cloned());
        Some(self.launch(args, "code-index-service.stderr.log"))
    }

    fn model_ready(&self) -> bool {
        self.model_state.lock().unwrap()["state"] == "ready"
    }

    /// Both model files in the Models folder (where download-model puts them), before it has even said so.
    fn model_files_there(&self) -> bool {
        let folder = self.options.models.clone().unwrap_or_else(|| self.options.data_root().join("Models"));
        MODEL_FILES.iter().all(|name| folder.join(name).is_file())
    }

    fn save_settings(&self) -> Result<(), String> {
        self.settings.lock().unwrap().save(&self.settings_path).map_err(|error| format!("can't save the settings: {error}"))
    }
}

// MARK: Commands: searching

/// The panel's questions for `search-service`.
#[tauri::command]
async fn ask(shell: State<'_, Arc<Shell>>, request: Value) -> Result<Value, String> {
    let op = request.get("op").and_then(Value::as_str).unwrap_or_default();
    if !["search", "embed_ahead", "text", "render", "prewarm", "status"].contains(&op) {
        return Err(format!("not a question the panel asks: {op}"));
    }
    shell.search.ask(request).await
}

/// Opens a result in its default app, at the page or moment that matched when the app can be told where.
#[tauri::command]
fn open_path(app: AppHandle, path: String, kind: Option<String>, loc: Option<f64>) -> Result<(), String> {
    hide_panel(app.clone());
    if let (Some(kind), Some(loc)) = (kind.as_deref(), loc) {
        let worth_it = match kind {
            "pdf" => loc > 1.0,
            "audio" | "video" => loc >= 1.0,
            _ => false,
        };
        if worth_it && win::open_at(&path, kind, loc) {
            return Ok(());
        }
    }
    app.opener().open_path(path, None::<&str>).map_err(|error| error.to_string())
}

/// A code result: opened in the person's code editor at the line that matched (else the file's default app).
#[tauri::command]
fn open_code(app: AppHandle, shell: State<'_, Arc<Shell>>, path: String, line: Option<f64>) -> Result<(), String> {
    hide_panel(app.clone());
    let chosen = shell.settings.lock().unwrap().code.editor.clone();
    match editors::pick(&chosen) {
        Some(editor) => editors::open(&editor, &path, line.filter(|l| *l >= 1.0).map(|l| l as u64)),
        None => app.opener().open_path(path, None::<&str>).map_err(|error| error.to_string()),
    }
}

/// The code editors installed here, and the one code opens in now.
#[tauri::command]
async fn code_editors(shell: State<'_, Arc<Shell>>) -> Result<Value, String> {
    let chosen = shell.settings.lock().unwrap().code.editor.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let picked = editors::pick(&chosen);
        json!({"installed": editors::installed(), "chosen": chosen, "opensIn": editors::name_of(picked.as_ref()),
               "picked": picked.map(|e| e.id)})
    })
    .await
    .map_err(|error| error.to_string())
}

/// What reading these code folders takes, one answer per folder (`trovara-helper estimate --code --json`): time, and
/// the room the code index needs.
#[tauri::command]
async fn code_estimate(shell: State<'_, Arc<Shell>>, folders: Vec<String>) -> Result<Vec<Value>, String> {
    let (helper, env) = (shell.options.helper.clone(), shell.env());
    let mut args = vec!["estimate".to_string(), "--code".into()];
    args.extend(shell.index_arguments());
    args.extend(folders);
    args.push("--json".into());
    tauri::async_runtime::spawn_blocking(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = helper::run_once(&helper, &args, &env)?;
        Ok(output.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// "Turn Off Code Search…": after a confirmation, code indexing stops and the code index is deleted (the code folders
/// are kept, for turning it on again). False: not confirmed.
#[tauri::command]
async fn code_off(app: AppHandle) -> Result<bool, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .message("Trovara stops reading your code and deletes the code index. Your code itself isn't touched. You can turn code search on again later; it reads the code again then.")
        .title("Turn Off Code Search?")
        .kind(tauri_plugin_dialog::MessageDialogKind::Warning)
        .buttons(tauri_plugin_dialog::MessageDialogButtons::OkCancelCustom("Turn Off".into(), "Cancel".into()))
        .show(move |confirmed| {
            let _ = sender.send(confirmed);
        });
    if !receiver.await.unwrap_or(false) {
        return Ok(false);
    }
    let shell = app.state::<Arc<Shell>>().inner().clone();
    shell.settings.lock().unwrap().code.on = false;
    shell.save_settings()?;
    let stopping = shell.clone();
    tauri::async_runtime::spawn_blocking(move || stopping.code_index.stop()).await.map_err(|error| error.to_string())?;
    shell.search.ask(json!({"op": "code_off"})).await?;
    set_status(&app, &shell.main_status.lock().unwrap().clone());
    Ok(true)
}

/// Shows a result in Explorer, selected.
#[tauri::command]
fn reveal(app: AppHandle, path: String) -> Result<(), String> {
    hide_panel(app.clone());
    app.opener().reveal_item_in_dir(path).map_err(|error| error.to_string())
}

/// Ctrl+C on a result: the file itself on the clipboard, to paste into Explorer, a mail or a chat.
#[tauri::command]
fn copy_files(paths: Vec<String>) -> Result<(), String> {
    win::copy_files(&paths)
}

#[tauri::command]
fn hide_panel(app: AppHandle) {
    if let Some(panel) = app.get_webview_window("panel") {
        let _ = panel.hide();
    }
}

/// The panel grows to show results and shrinks back to its field.
#[tauri::command]
fn panel_height(app: AppHandle, height: f64) {
    if let Some(panel) = app.get_webview_window("panel") {
        let _ = panel.set_size(LogicalSize::new(PANEL_WIDTH, height.clamp(FIELD_HEIGHT, 640.0)));
    }
}

/// Ctrl+O in the panel: the search window, with the panel's query.
#[tauri::command]
fn open_search_window(app: AppHandle, query: String) -> Result<(), String> {
    hide_panel(app.clone());
    show_search_window(&app, &query).map_err(|error| error.to_string())
}

/// A page closing its own window (the search window on Esc, settings on Done).
#[tauri::command]
fn close_window(window: WebviewWindow) {
    let _ = window.close();
}

// MARK: Commands: setup and settings

/// What indexing these folders would take, one answer per folder (`trovara-helper estimate --json`).
#[tauri::command]
async fn estimate(shell: State<'_, Arc<Shell>>, folders: Vec<String>) -> Result<Vec<Value>, String> {
    let (helper, env) = (shell.options.helper.clone(), shell.env());
    let mut args = vec!["estimate".to_string()];
    args.extend(shell.index_arguments());
    args.extend(folders);
    args.push("--json".into());
    tauri::async_runtime::spawn_blocking(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let output = helper::run_once(&helper, &args, &env)?;
        Ok(output.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// What the setup window starts from: the folders it offers, the hotkey, the model, the folders already chosen.
#[tauri::command]
fn setup_info(app: AppHandle, shell: State<'_, Arc<Shell>>) -> Value {
    let offered: Vec<PathBuf> = match &shell.options.offered {
        Some(folders) => folders.clone(),
        None => {
            let paths = app.path();
            [paths.document_dir(), paths.picture_dir(), paths.video_dir(), paths.audio_dir(), paths.desktop_dir(),
             paths.download_dir()]
                .into_iter()
                .filter_map(Result::ok)
                .filter(|folder| folder.is_dir())
                .collect()
        }
    };
    let hotkey = shell.hotkey();
    json!({
        "offered": offered.iter().map(|folder| folder.display().to_string()).collect::<Vec<_>>(),
        "chosen": shell.folders(),
        "setUp": shell.is_set_up(),
        "hotkey": hotkey,
        "hotkeyText": shortcut_text(&hotkey),
        "defaultHotkey": DEFAULT_HOTKEY,
        "model": shell.model_state.lock().unwrap().clone(),
        "auto": shell.options.debug_setup,
    })
}

/// Folders chosen in the system's folder picker (none when it's cancelled).
#[tauri::command]
async fn pick_folders(app: AppHandle) -> Vec<String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog().file().set_title("Add Folders").pick_folders(move |folders| {
        let _ = sender.send(folders);
    });
    let picked = receiver.await.ok().flatten().unwrap_or_default();
    picked.into_iter().filter_map(|folder| folder.into_path().ok()).map(|path| path.display().to_string()).collect()
}

/// "Use Files…": the model files downloaded elsewhere (another network), checked and copied in.
#[tauri::command]
async fn use_model_files(app: AppHandle) -> Result<bool, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog().file().set_title("The Folder With the Model Files").pick_folder(move |folder| {
        let _ = sender.send(folder);
    });
    let Some(folder) = receiver.await.ok().flatten().and_then(|folder| folder.into_path().ok()) else {
        return Ok(false);
    };
    let shell = app.state::<Arc<Shell>>().inner().clone();
    shell.model.stop();
    shell.model_bytes.lock().unwrap().clear();
    let launch = shell.launch(
        vec!["download-model".into(), "--json".into(), "--from".into(), folder.display().to_string()],
        "model.stderr.log",
    );
    shell.model.run(&launch)?;
    Ok(true)
}

/// The download again, after it failed.
#[tauri::command]
fn retry_model(app: AppHandle) -> Result<(), String> {
    start_model(&app)
}

/// Setup's last step: the folders and the shortcut are saved, indexing starts, and fast catch-up is turned on if
/// asked (Windows asks for administrator permission once, then).
#[tauri::command]
fn finish_setup(app: AppHandle, shell: State<'_, Arc<Shell>>, folders: Vec<String>, fast_catch_up: bool) -> Result<(), String> {
    {
        let mut settings = shell.settings.lock().unwrap();
        settings.folders = folders;
        settings.set_up = true;
    }
    shell.save_settings()?;
    apply_launch_at_login(&app);
    apply_folders(&app);
    if let Some(window) = app.get_webview_window("setup") {
        let _ = window.close();
    }
    if fast_catch_up && !shell.options.debug_setup {
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            if let Err(error) = set_fast_catch_up_now(&app, true) {
                set_status(&app, &format!("Fast catch-up isn't on: {error}"));
            }
        });
    }
    Ok(())
}

/// What the settings window shows.
#[tauri::command]
fn get_settings(shell: State<'_, Arc<Shell>>) -> Value {
    let settings = shell.settings.lock().unwrap().clone();
    let hotkey = shell.hotkey();
    json!({
        "settings": settings,
        "folders": shell.folders(),
        "hotkey": hotkey,
        "hotkeyText": shortcut_text(&hotkey),
        "defaultHotkey": DEFAULT_HOTKEY,
        "defaultHotkeyText": shortcut_text(DEFAULT_HOTKEY),
        "model": shell.model_state.lock().unwrap().clone(),
        "version": env!("CARGO_PKG_VERSION"),
        "updates": updates::ENDPOINT.is_some_and(|e| !e.is_empty()),
        "logs": shell.logs.display().to_string(),
        "index": shell.options.index_folder().display().to_string(),
    })
}

/// The settings window's changes (all but the hotkey and fast catch-up, which have their own commands): saved, then
/// the helpers follow them (a new index helper when its arguments changed, the search service's folders).
#[tauri::command]
fn save_settings(app: AppHandle, shell: State<'_, Arc<Shell>>, settings: Settings) -> Result<(), String> {
    let before = shell.settings.lock().unwrap().clone();
    {
        let mut kept = shell.settings.lock().unwrap();
        let (hotkey, set_up) = (kept.hotkey.clone(), kept.set_up);
        *kept = settings;
        kept.hotkey = hotkey;
        kept.set_up = set_up;
        kept.priority = match kept.priority.as_str() {
            "auto" | "normal" | "low" | "eco" => kept.priority.clone(),
            _ => "auto".into(),
        };
        kept.skip_types = kept
            .skip_types
            .iter()
            .map(|extension| extension.trim().trim_start_matches('.').to_ascii_lowercase())
            .filter(|extension| !extension.is_empty())
            .collect();
    }
    shell.save_settings()?;
    let after = shell.settings.lock().unwrap().clone();
    if after.launch_at_login != before.launch_at_login {
        apply_launch_at_login(&app);
    }
    if after.paused != before.paused {
        set_paused(&app, after.paused);
    }
    if after.folders != before.folders
        || after.skipped != before.skipped
        || after.skip_types != before.skip_types
        || after.code.folders != before.code.folders
        || after.code.skipped != before.code.skipped
        || after.code.on != before.code.on
    {
        apply_folders(&app);
    } else {
        run_index(&app, false);
    }
    Ok(())
}

/// A new hotkey from the recorder: kept only if Windows lets Trovara have it (another app may hold it); otherwise the
/// old one stays. Empty: back to the default.
#[tauri::command]
fn set_hotkey(app: AppHandle, shell: State<'_, Arc<Shell>>, accelerator: String) -> Result<String, String> {
    let wanted = if accelerator.trim().is_empty() { DEFAULT_HOTKEY.to_string() } else { accelerator.trim().to_string() };
    let old = shell.hotkey_now.lock().unwrap().clone();
    let shortcuts = app.global_shortcut();
    if !old.is_empty() {
        let _ = shortcuts.unregister(old.as_str());
    }
    if let Err(error) = shortcuts.register(wanted.as_str()) {
        if !old.is_empty() {
            let _ = shortcuts.register(old.as_str());
        }
        return Err(format!("{} can't be used: {}", shortcut_text(&wanted), plain_shortcut_error(&error.to_string())));
    }
    *shell.hotkey_now.lock().unwrap() = wanted.clone();
    shell.settings.lock().unwrap().hotkey = if wanted == DEFAULT_HOTKEY { String::new() } else { wanted.clone() };
    shell.save_settings()?;
    if let Some(tray) = shell.tray.lock().unwrap().as_ref() {
        let _ = tray.search.set_accelerator(Some(wanted.as_str()));
    }
    Ok(shortcut_text(&wanted))
}

/// While the recorder listens, the hotkey is let go (pressing it then must reach the recorder, not open the panel).
#[tauri::command]
fn hotkey_recording(app: AppHandle, shell: State<'_, Arc<Shell>>, on: bool) {
    let current = shell.hotkey_now.lock().unwrap().clone();
    if current.is_empty() {
        return;
    }
    let shortcuts = app.global_shortcut();
    if on {
        let _ = shortcuts.unregister(current.as_str());
    } else if !shortcuts.is_registered(current.as_str()) {
        let _ = shortcuts.register(current.as_str());
    }
}

/// Whether fast catch-up (the change journal reader, TRD §9.3) is on.
#[tauri::command]
async fn fast_catch_up(shell: State<'_, Arc<Shell>>) -> Result<bool, String> {
    let (helper, env) = (shell.options.helper.clone(), shell.env());
    tauri::async_runtime::spawn_blocking(move || {
        let output = helper::run_once(&helper, &["usn", "status"], &env)?;
        Ok(output.lines().any(|line| line.trim() == "fast catch-up: on"))
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Turns fast catch-up on or off: Windows asks for administrator permission (once each way).
#[tauri::command]
async fn set_fast_catch_up(app: AppHandle, on: bool) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || set_fast_catch_up_now(&app, on)).await.map_err(|error| error.to_string())?
}

fn set_fast_catch_up_now(app: &AppHandle, on: bool) -> Result<bool, String> {
    let shell = app.state::<Arc<Shell>>().inner().clone();
    helper::run_once(&shell.options.helper, &["usn", if on { "install" } else { "uninstall" }], &shell.env())?;
    let output = helper::run_once(&shell.options.helper, &["usn", "status"], &shell.env())?;
    let now_on = output.lines().any(|line| line.trim() == "fast catch-up: on");
    if now_on != on {
        return Err(if on { "permission wasn't given".into() } else { "it's still on".into() });
    }
    Ok(now_on)
}

/// The settings window (tray: Settings…).
#[tauri::command]
fn choose_folders(app: AppHandle) -> Result<(), String> {
    show_setup(&app).map_err(|error| error.to_string())
}

#[tauri::command]
fn open_logs(app: AppHandle, shell: State<'_, Arc<Shell>>) -> Result<(), String> {
    app.opener().open_path(shell.logs.display().to_string(), None::<&str>).map_err(|error| error.to_string())
}

/// Settings → "Check Now": {"state": "off" | "current" | "ready" | "busy", "version"?, "message"?}.
#[tauri::command]
async fn check_updates(app: AppHandle) -> Result<Value, String> {
    let result = updates::check(&app).await?;
    show_update(&app);
    Ok(result)
}

/// "Restart to Update": the downloaded version installs now.
#[tauri::command]
fn restart_to_update(app: AppHandle) -> bool {
    stop_helpers(&app);
    updates::install_ready(&app)
}

/// Settings → About → Licenses: the notices of everything Trovara is made with (Acknowledgements.txt).
#[tauri::command]
fn open_acknowledgements(app: AppHandle) -> Result<(), String> {
    let installed = app.path().resource_dir().map(|folder| folder.join("Acknowledgements.txt")).ok();
    // A dev run: the copy the release script makes beside the sources.
    let dev = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources").join("Acknowledgements.txt");
    let file = installed.filter(|f| f.is_file()).unwrap_or(dev);
    if !file.is_file() {
        return Err("the licenses file isn't there (scripts\\acknowledgements.ps1 makes it)".into());
    }
    app.opener().open_path(file.display().to_string(), None::<&str>).map_err(|error| error.to_string())
}

// MARK: Commands: debug runs

/// Debug runs: the query the panel types in when it loads (with setup, it comes later as a "debug-query" event).
#[tauri::command]
fn debug_query(shell: State<'_, Arc<Shell>>) -> Option<String> {
    if shell.options.debug_setup {
        return None;
    }
    shell.options.debug_query.clone()
}

/// Debug runs: what the page showed for the debug query, printed (and the app quits, if asked).
#[tauri::command]
fn debug_report(app: AppHandle, shell: State<'_, Arc<Shell>>, report: Value) {
    println!("{report}");
    if shell.options.debug_quit {
        quit(&app);
    }
}

// MARK: Starting and stopping

fn main() {
    harden_dll_search();
    let options = match Options::from_args() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("trovara: {error}");
            std::process::exit(2);
        }
    };
    let app = tauri::Builder::default()
        // A second launch (the Start menu while Trovara runs) opens the panel of the one running.
        .plugin(tauri_plugin_single_instance::init(|app, args, _| {
            if !args.iter().any(|arg| arg == "--background" || arg == "--autostart") {
                show_panel(app);
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--background"])))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _, event| {
                    if event.state == ShortcutState::Pressed {
                        toggle_panel(app);
                    }
                })
                .build(),
        )
        .setup(move |app| setup(app, options))
        .invoke_handler(tauri::generate_handler![
            ask, open_path, reveal, copy_files, hide_panel, panel_height, open_search_window, close_window, estimate,
            setup_info, pick_folders, use_model_files, retry_model, finish_setup, get_settings, save_settings,
            set_hotkey, hotkey_recording, fast_catch_up, set_fast_catch_up, choose_folders, open_logs, debug_query,
            debug_report, open_code, code_editors, code_estimate, code_off, check_updates, restart_to_update,
            open_acknowledgements
        ])
        .build(tauri::generate_context!())
        .expect("Trovara couldn't start");
    app.run(|app, event| match event {
        tauri::RunEvent::Exit => stop_helpers(app),
        // Closing a window isn't quitting: Trovara lives in the tray.
        tauri::RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
        _ => {}
    });
}

fn setup(app: &mut tauri::App, options: Options) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let logs = options.data_root().join("logs");
    std::fs::create_dir_all(&logs)?;
    let settings_path = options.settings.clone().unwrap_or_else(|| options.data_root().join("settings.json"));
    let settings = Settings::load(&settings_path);
    let roots: Vec<String> = if options.roots.is_empty() {
        settings.folders.clone()
    } else {
        options.roots.iter().map(|root| root.display().to_string()).collect()
    };
    let mut search_args = vec!["search-service".to_string()];
    search_args.extend(options.index.iter().flat_map(|index| ["--index".to_string(), index.display().to_string()]));
    for folder in &settings.skipped {
        search_args.extend(["--exclude".to_string(), folder.clone()]);
    }
    for extension in &settings.skip_types {
        search_args.extend(["--exclude-ext".to_string(), extension.clone()]);
    }
    for folder in &settings.code.folders {
        search_args.extend(["--code-root".to_string(), folder.clone()]);
    }
    for folder in &settings.code.skipped {
        search_args.extend(["--code-exclude".to_string(), folder.clone()]);
    }
    search_args.extend(roots.iter().cloned());
    let search = SearchService::new(
        Launch {
            program: options.helper.clone(),
            args: search_args,
            env: options.models.iter().map(|models| ("TROVARA_MODELS".to_string(), models.display().to_string())).collect(),
            log: Some(logs.join("search-service.stderr.log")),
        },
        {
            let app = handle.clone();
            move |event| search_event(&app, event)
        },
    );
    let index = IndexService::new({
        let app = handle.clone();
        move |event| index_event(&app, event, false)
    });
    let code_index = IndexService::new({
        let app = handle.clone();
        move |event| index_event(&app, event, true)
    });
    let model = ModelDownload::new({
        let app = handle.clone();
        move |event| model_event(&app, event)
    });
    // Previews show files from the chosen folders (and the helper's renders, once it says where the index is).
    for root in &roots {
        let _ = app.asset_protocol_scope().allow_directory(root, true);
    }
    let shell = Arc::new(Shell {
        options,
        settings: Mutex::new(settings),
        settings_path,
        search,
        index,
        code_index,
        model,
        model_state: Mutex::new(json!({"state": "checking"})),
        model_bytes: Mutex::new(HashMap::new()),
        index_has_model: AtomicBool::new(false),
        code_has_model: AtomicBool::new(false),
        main_status: Mutex::new(String::new()),
        hotkey_now: Mutex::new(String::new()),
        tray: Mutex::new(None),
        reloaded: Mutex::new([Instant::now(); 2]),
        restarts: Mutex::new([Vec::new(), Vec::new()]),
        quitting: AtomicBool::new(false),
        debug_query_sent: AtomicBool::new(false),
        logs,
    });
    app.manage(shell.clone());
    app.manage(updates::Updates::default());

    make_panel(&handle)?;
    // No tray (a session without a taskbar): the hotkey and the panel still work.
    if let Err(error) = make_tray(&handle, &shell) {
        eprintln!("trovara: no tray icon: {error}");
    }
    let hotkey = shell.hotkey();
    match app.global_shortcut().register(hotkey.as_str()) {
        Ok(()) => *shell.hotkey_now.lock().unwrap() = hotkey,
        Err(error) => {
            // Another app holds it: say so; Settings… lets the person pick another.
            set_status(&handle, &format!("{} is taken by another app: pick another in Settings", shortcut_text(&hotkey)));
            eprintln!("trovara: can't use {hotkey} as the shortcut: {error}");
        }
    }

    // The search service starts now (the first search is then quick); the model is checked (and fetched if it's
    // missing: on first launch that runs while setup is open); indexing starts at once (names are searchable before
    // the model is in) and starts over when the model arrives.
    let warm = shell.clone();
    tauri::async_runtime::spawn(async move {
        let _ = warm.search.ask(json!({"op": "status"})).await;
    });
    updates::start_daily(
        handle.clone(),
        |app| app.state::<Arc<Shell>>().settings.lock().unwrap().check_updates,
        show_update,
    );
    if let Err(error) = start_model(&handle) {
        model_event(&handle, json!({"event": "error", "message": error}));
    }
    if shell.is_set_up() {
        run_index(&handle, false);
    } else {
        show_setup(&handle)?;
    }
    if shell.options.debug_show {
        show_panel(&handle);
    }
    if shell.options.debug_update {
        let app = handle.clone();
        tauri::async_runtime::spawn(async move {
            let answer = updates::check(&app).await;
            println!("update check: {answer:?}");
            if matches!(&answer, Ok(found) if found["state"] == "ready") {
                stop_helpers(&app);
                println!("installing: {}", updates::install_ready(&app));
            }
        });
    }
    match shell.options.debug_open.as_deref().map(|open| open.split_once(':').unwrap_or((open, ""))) {
        Some(("window", query)) => show_search_window(&handle, query)?,
        Some(("settings", _)) => show_settings(&handle)?,
        Some(("setup", _)) => show_setup(&handle)?,
        _ => {}
    }
    Ok(())
}

fn start_model(app: &AppHandle) -> Result<(), String> {
    let shell = app.state::<Arc<Shell>>().inner().clone();
    shell.model_bytes.lock().unwrap().clear();
    shell.model.run(&shell.launch(vec!["download-model".into(), "--json".into()], "model.stderr.log"))
}

/// After setup or a change of folders: the search service and indexing follow the new folders.
fn apply_folders(app: &AppHandle) {
    let shell = app.state::<Arc<Shell>>().inner().clone();
    let folders = shell.folders();
    let (skipped, types, code) = {
        let settings = shell.settings.lock().unwrap();
        (settings.skipped.clone(), settings.skip_types.clone(), settings.code.clone())
    };
    for folder in &folders {
        let _ = app.asset_protocol_scope().allow_directory(folder, true);
    }
    let search = shell.clone();
    tauri::async_runtime::spawn(async move {
        let _ = search
            .search
            .ask(json!({"op": "folders", "roots": folders, "skipped": skipped, "types": types,
                        "code_roots": code.folders, "code_skipped": code.skipped}))
            .await;
    });
    run_index(app, false);
}

/// Starts the index helpers for the folders as they are (or has the running ones sync); `restart`: start them over.
fn run_index(app: &AppHandle, restart: bool) {
    let shell = app.state::<Arc<Shell>>().inner().clone();
    if !shell.is_set_up() {
        return;
    }
    run_code_index(app, restart);
    if shell.folders().is_empty() {
        shell.index.stop();
        set_status(app, "No folders chosen yet");
        return;
    }
    let launch = shell.index_launch();
    let model = shell.model_ready() || shell.model_files_there();
    // Off the main thread: starting over waits for the old helper to finish the file it's reading.
    std::thread::spawn({
        let app = app.clone();
        move || {
            let starts_fresh = restart || !shell.index.is_running();
            match shell.index.run(&launch, restart) {
                Ok(()) => {
                    if starts_fresh || restart {
                        shell.index_has_model.store(model, Ordering::SeqCst);
                    }
                    if shell.settings.lock().unwrap().paused {
                        shell.index.send("pause");
                    }
                }
                Err(error) => set_status(&app, &format!("Indexing couldn't start: {error}")),
            }
        }
    });
}

/// The code index's helper: started (or told to sync) while code search is on, stopped when it's off.
fn run_code_index(app: &AppHandle, restart: bool) {
    let shell = app.state::<Arc<Shell>>().inner().clone();
    let model = shell.model_ready() || shell.model_files_there();
    let app = app.clone();
    std::thread::spawn(move || {
        let Some(launch) = shell.code_launch() else {
            shell.code_index.stop();
            return;
        };
        let starts_fresh = restart || !shell.code_index.is_running();
        match shell.code_index.run(&launch, restart) {
            Ok(()) => {
                if starts_fresh {
                    shell.code_has_model.store(model, Ordering::SeqCst);
                }
                if shell.settings.lock().unwrap().paused {
                    shell.code_index.send("pause");
                }
            }
            Err(error) => set_status(&app, &format!("Code indexing couldn't start: {error}")),
        }
    });
}

/// Pause or resume from the tray or settings: the running helper is told, and the choice is kept.
fn set_paused(app: &AppHandle, paused: bool) {
    let shell = app.state::<Arc<Shell>>().inner().clone();
    {
        let mut settings = shell.settings.lock().unwrap();
        if settings.paused != paused {
            settings.paused = paused;
            drop(settings);
            let _ = shell.save_settings();
        }
    }
    if let Some(tray) = shell.tray.lock().unwrap().as_ref() {
        let _ = tray.pause.set_text(if paused { "Resume Indexing" } else { "Pause Indexing" });
    }
    shell.index.send(if paused { "pause" } else { "resume" });
    shell.code_index.send(if paused { "pause" } else { "resume" });
    let _ = app.emit("paused", paused);
}

/// The sign-in entry follows the setting (installed builds only: a dev build must not start at sign-in).
fn apply_launch_at_login(app: &AppHandle) {
    if cfg!(debug_assertions) || !app.state::<Arc<Shell>>().options.roots.is_empty() {
        return;
    }
    let wanted = app.state::<Arc<Shell>>().settings.lock().unwrap().launch_at_login;
    let autostart = app.autolaunch();
    let result = if wanted { autostart.enable() } else { autostart.disable() };
    if let Err(error) = result {
        eprintln!("trovara: launch at sign-in: {error}");
    }
}

fn stop_helpers(app: &AppHandle) {
    let shell = app.state::<Arc<Shell>>();
    shell.quitting.store(true, Ordering::SeqCst);
    shell.model.stop();
    shell.index.stop();
    shell.code_index.stop();
    shell.search.stop();
}

fn quit(app: &AppHandle) {
    stop_helpers(app);
    // A downloaded update installs as Trovara quits (it starts again after).
    if updates::install_ready(app) {
        return;
    }
    app.exit(0);
}

/// The tray's update item: "Restart to Update to X" once a version is downloaded.
fn show_update(app: &AppHandle) {
    let Some(version) = app.state::<updates::Updates>().ready_version() else { return };
    if let Some(tray) = app.state::<Arc<Shell>>().tray.lock().unwrap().as_ref() {
        let _ = tray.update.set_text(format!("Restart to Update to {version}"));
    }
}

/// DLLs from the program's folder and System32 only, never the current folder or PATH (TRD §14).
fn harden_dll_search() {
    #[cfg(windows)]
    // SAFETY: a process-wide setting with valid flags, made before anything else loads.
    unsafe {
        use windows::Win32::System::LibraryLoader::{
            SetDefaultDllDirectories, LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32,
            LOAD_LIBRARY_SEARCH_USER_DIRS,
        };
        let _ = SetDefaultDllDirectories(
            LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_USER_DIRS,
        );
    }
}

// MARK: Windows

fn make_panel(app: &AppHandle) -> tauri::Result<()> {
    let panel = WebviewWindowBuilder::new(app, "panel", WebviewUrl::App("index.html".into()))
        .title("Trovara")
        .inner_size(PANEL_WIDTH, FIELD_HEIGHT)
        .decorations(false)
        .transparent(true)
        .shadow(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .build()?;
    let app = app.clone();
    panel.on_window_event(move |event| {
        // It steps away when you click elsewhere.
        if let WindowEvent::Focused(false) = event {
            if !app.state::<Arc<Shell>>().options.debug_show {
                hide_panel(app.clone());
            }
        }
    });
    Ok(())
}

fn show_panel(app: &AppHandle) {
    if !app.state::<Arc<Shell>>().is_set_up() {
        let _ = show_setup(app);
        return;
    }
    let Some(panel) = app.get_webview_window("panel") else { return };
    // On the screen with the pointer, centered, a fifth of the way down.
    if let Ok(pointer) = app.cursor_position() {
        if let Ok(Some(screen)) = app.monitor_from_point(pointer.x, pointer.y) {
            let width = (PANEL_WIDTH * screen.scale_factor()) as i32;
            let x = screen.position().x + (screen.size().width as i32 - width) / 2;
            let y = screen.position().y + (screen.size().height as f64 * 0.2) as i32;
            let _ = panel.set_position(PhysicalPosition::new(x, y));
        }
    }
    let _ = panel.show();
    let _ = panel.set_focus();
    let _ = panel.emit("panel-shown", ());
}

fn toggle_panel(app: &AppHandle) {
    match app.get_webview_window("panel") {
        Some(panel) if panel.is_visible().unwrap_or(false) => hide_panel(app.clone()),
        _ => show_panel(app),
    }
}

/// The search window: the panel's results with room (a grid for pictures, filters by kind).
fn show_search_window(app: &AppHandle, query: &str) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("search") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        let _ = window.emit("search-query", query);
        return Ok(());
    }
    let url = format!("search.html?q={}", encode_component(query));
    WebviewWindowBuilder::new(app, "search", WebviewUrl::App(url.into()))
        .title("Trovara")
        .inner_size(1120.0, 740.0)
        .min_inner_size(760.0, 480.0)
        .center()
        .build()?;
    Ok(())
}

/// The setup window (first launch, and Choose Folders… later). Debug runs that click through it keep it hidden.
fn show_setup(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("setup") {
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    let hidden = app.state::<Arc<Shell>>().options.debug_setup;
    WebviewWindowBuilder::new(app, "setup", WebviewUrl::App("setup.html".into()))
        .title("Set Up Trovara")
        .inner_size(720.0, 620.0)
        .resizable(false)
        .maximizable(false)
        .center()
        .visible(!hidden)
        .build()?;
    Ok(())
}

fn show_settings(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }
    WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("Trovara Settings")
        .inner_size(700.0, 680.0)
        .min_inner_size(560.0, 480.0)
        .center()
        .build()?;
    Ok(())
}

/// "Control+Shift+Space" as people read it: "Ctrl+Shift+Space".
fn shortcut_text(accelerator: &str) -> String {
    accelerator
        .split('+')
        .map(|part| match part {
            "CommandOrControl" | "CmdOrCtrl" | "Control" | "Ctrl" => "Ctrl",
            "Super" | "Meta" | "Cmd" | "Command" => "Win",
            "Option" => "Alt",
            other => other,
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Windows' "already registered" in words.
fn plain_shortcut_error(error: &str) -> String {
    if error.to_ascii_lowercase().contains("already") {
        "another app is using it".into()
    } else {
        error.to_string()
    }
}

fn encode_component(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (byte as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}

// MARK: The tray

fn make_tray(app: &AppHandle, shell: &Arc<Shell>) -> tauri::Result<()> {
    let paused = shell.settings.lock().unwrap().paused;
    let status = MenuItem::with_id(app, "status", "Starting…", false, None::<&str>)?;
    let search = MenuItem::with_id(app, "search", "Search…", true, Some(shell.hotkey().as_str()))?;
    let window = MenuItem::with_id(app, "window", "Search Window", true, None::<&str>)?;
    let pause = MenuItem::with_id(app, "pause", if paused { "Resume Indexing" } else { "Pause Indexing" }, true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let folders = MenuItem::with_id(app, "folders", "Choose Folders…", true, None::<&str>)?;
    let update = MenuItem::with_id(app, "update", "Check for Updates…", updates::ENDPOINT.is_some(), None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit Trovara", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &search,
            &window,
            &pause,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &folders,
            &update,
            &PredefinedMenuItem::separator(app)?,
            &quit_item,
        ],
    )?;
    TrayIconBuilder::with_id("trovara")
        .icon(tauri::include_image!("icons/tray.png"))
        .tooltip("Trovara")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "search" => show_panel(app),
            "window" => {
                let _ = show_search_window(app, "");
            }
            "pause" => {
                let paused = app.state::<Arc<Shell>>().settings.lock().unwrap().paused;
                set_paused(app, !paused);
            }
            "settings" => {
                let _ = show_settings(app);
            }
            "folders" => {
                let _ = show_setup(app);
            }
            "update" => {
                if app.state::<updates::Updates>().ready_version().is_some() {
                    stop_helpers(app);
                    updates::install_ready(app);
                    return;
                }
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    match updates::check(&app).await {
                        Ok(answer) if answer["state"] == "current" => show_status(&app, "Trovara is up to date"),
                        Ok(_) => show_update(&app),
                        Err(error) => show_status(&app, &format!("Updates: {error}")),
                    }
                });
            }
            "quit" => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // A click opens the panel; the menu is on the other button.
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                toggle_panel(tray.app_handle());
            }
        })
        .build(app)?;
    *shell.tray.lock().unwrap() = Some(TrayItems { status, search, pause, update });
    Ok(())
}

fn set_status(app: &AppHandle, text: &str) {
    let shell = app.state::<Arc<Shell>>();
    *shell.main_status.lock().unwrap() = text.to_string();
    show_status(app, text);
}

/// Shows a line in the tray without making it the main index's status (code indexing's progress).
fn show_status(app: &AppHandle, text: &str) {
    let shell = app.state::<Arc<Shell>>();
    if let Some(tray) = shell.tray.lock().unwrap().as_ref() {
        let _ = tray.status.set_text(text);
    }
    if let Some(tray) = app.tray_by_id("trovara") {
        let _ = tray.set_tooltip(Some(format!("Trovara · {text}")));
    }
    let _ = app.emit("status", text);
}

// MARK: The helpers' events

fn search_event(app: &AppHandle, event: Value) {
    if cfg!(debug_assertions) {
        println!("search: {event}");
    }
    if event["event"] == "ready" {
        if let Some(index) = event["index"].as_str() {
            let _ = app.asset_protocol_scope().allow_directory(Path::new(index).join("previews"), false);
        }
    }
    let _ = app.emit("helper", json!({"from": "search", "event": event}));
}

fn index_event(app: &AppHandle, event: Value, code: bool) {
    if cfg!(debug_assertions) {
        println!("{}: {event}", if code { "code" } else { "index" });
    }
    if code {
        return code_index_event(app, event);
    }
    let count = |key: &str| event[key].as_u64().unwrap_or(0);
    let files = |n: u64| format!("{} {}", n, if n == 1 { "file" } else { "files" });
    match event["event"].as_str().unwrap_or_default() {
        "file" => {
            set_status(app, &format!("Indexing {} of {}", count("done"), count("total")));
            reload(app, false, false);
        }
        "rest" => {
            set_status(app, &format!("Reading the rest of long files ({} of {})", count("done"), count("total")));
            reload(app, false, false);
        }
        "sync" => reload(app, false, false),
        "summary" => reload(app, true, false),
        "idle" => {
            set_status(app, &format!("Up to date · {}", files(count("searchable"))));
            reload(app, true, false);
            // A debug run through setup searches once the first index is in.
            let shell = app.state::<Arc<Shell>>();
            if let Some(query) = &shell.options.debug_query {
                if shell.options.debug_setup && !shell.debug_query_sent.swap(true, Ordering::SeqCst) {
                    let _ = app.emit_to("panel", "debug-query", query.clone());
                }
            }
        }
        "waiting" => {
            let left = count("pending") + count("unfinished");
            let text = match event["for"].as_str().unwrap_or_default() {
                "model" => format!("Waiting for the model · {} searchable by name", files(count("files"))),
                "paused" => format!("Indexing paused · {} to go", files(left)),
                "battery" => format!("Waiting for mains power · {} to go", files(left)),
                "saver" => format!("Battery Saver is on · {} to go", files(left)),
                "fullscreen" => format!("Waiting while a fullscreen app is open · {} to go", files(left)),
                "hours" => format!("Waiting for indexing hours · {} to go", files(left)),
                other => format!("Waiting ({other})"),
            };
            set_status(app, &text);
            reload(app, true, false);
        }
        "missing" => {
            let folders = event["folders"].as_array().map(|f| f.len()).unwrap_or(0);
            if folders > 0 {
                set_status(app, &format!("{folders} folder{} can't be found (a drive unplugged?)", if folders == 1 { "" } else { "s" }));
            }
        }
        "error" => set_status(app, &format!("Indexing: {}", event["message"].as_str().unwrap_or("error"))),
        "gone" => index_gone(app, false),
        _ => {}
    }
    let _ = app.emit("helper", json!({"from": "index", "event": event}));
}

/// The code index's helper: its progress shows in the tray while it reads, then the main status comes back.
fn code_index_event(app: &AppHandle, event: Value) {
    let count = |key: &str| event[key].as_u64().unwrap_or(0);
    let main = || app.state::<Arc<Shell>>().main_status.lock().unwrap().clone();
    match event["event"].as_str().unwrap_or_default() {
        "file" => {
            show_status(app, &format!("Reading code · {} of {}", count("done"), count("total")));
            reload(app, false, true);
        }
        "sync" => reload(app, false, true),
        "summary" => reload(app, true, true),
        "idle" | "waiting" => {
            show_status(app, &main());
            reload(app, true, true);
        }
        "error" => show_status(app, &format!("Code indexing: {}", event["message"].as_str().unwrap_or("error"))),
        "gone" => index_gone(app, true),
        _ => {}
    }
    let _ = app.emit("helper", json!({"from": "code", "event": event}));
}

/// The index helper stopped on its own (a crash; the watchdog stopping a file that hung it): started again, unless it
/// keeps stopping.
fn index_gone(app: &AppHandle, code: bool) {
    let shell = app.state::<Arc<Shell>>().inner().clone();
    reload(app, true, code);
    if shell.quitting.load(Ordering::SeqCst) {
        return;
    }
    let too_often = {
        let mut restarts = shell.restarts.lock().unwrap();
        let restarts = &mut restarts[code as usize];
        restarts.retain(|at| at.elapsed() < Duration::from_secs(600));
        restarts.push(Instant::now());
        restarts.len() > RESTARTS_PER_TEN_MINUTES
    };
    if too_often {
        let what = if code { "Code indexing" } else { "Indexing" };
        set_status(app, &format!("{what} keeps stopping: see the logs in Settings"));
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        if !app.state::<Arc<Shell>>().quitting.load(Ordering::SeqCst) {
            if code {
                run_code_index(&app, false);
            } else {
                run_index(&app, false);
            }
        }
    });
}

fn model_event(app: &AppHandle, event: Value) {
    if cfg!(debug_assertions) && event["event"] != "progress" {
        println!("model: {event}");
    }
    let shell = app.state::<Arc<Shell>>().inner().clone();
    let name = event["name"].as_str().unwrap_or_default().to_string();
    let overall = |shell: &Shell| -> f64 { shell.model_bytes.lock().unwrap().values().map(|(bytes, _)| *bytes as f64).sum() };
    let state = match event["event"].as_str().unwrap_or_default() {
        "checking" => json!({"state": "checking"}),
        "file" | "progress" => {
            let (bytes, total) = (event["bytes"].as_u64().unwrap_or(0), event["total"].as_u64().unwrap_or(0));
            shell.model_bytes.lock().unwrap().insert(name, (bytes, total));
            let done = overall(&shell);
            let percent = (100.0 * done / MODEL_BYTES).min(100.0);
            set_status(app, &format!("Downloading the model · {percent:.0}%"));
            json!({"state": "downloading", "bytes": done, "total": MODEL_BYTES})
        }
        "verified" => {
            if let Some(entry) = shell.model_bytes.lock().unwrap().get_mut(&name) {
                entry.0 = entry.1;
            }
            json!({"state": "downloading", "bytes": overall(&shell), "total": MODEL_BYTES})
        }
        "retry" => json!({"state": "downloading", "bytes": overall(&shell), "total": MODEL_BYTES, "problem": event["message"]}),
        "done" => json!({"state": "ready"}),
        "error" => {
            set_status(app, "The model couldn't download");
            json!({"state": "failed", "message": event["message"]})
        }
        _ => return,
    };
    let ready = state["state"] == "ready";
    *shell.model_state.lock().unwrap() = state.clone();
    let _ = app.emit("model", state);
    if ready && shell.is_set_up() {
        // A helper started before the model was in only reads names: it starts over to read what's in the files.
        let restart = shell.index.is_running() && !shell.index_has_model.load(Ordering::SeqCst);
        run_index(app, restart);
        if shell.code_index.is_running() && !shell.code_has_model.load(Ordering::SeqCst) {
            run_code_index(app, true);
        }
    }
}

/// Has the search service read an index again (`code`: the code index): at once when indexing settles, else at most
/// every three seconds.
fn reload(app: &AppHandle, now: bool, code: bool) {
    let shell = app.state::<Arc<Shell>>().inner().clone();
    {
        let mut reloaded = shell.reloaded.lock().unwrap();
        let last = &mut reloaded[code as usize];
        if !now && last.elapsed() < Duration::from_secs(3) {
            return;
        }
        *last = Instant::now();
    }
    tauri::async_runtime::spawn(async move {
        let reply = shell.search.ask(json!({"op": "reload", "code": code})).await;
        if cfg!(debug_assertions) {
            println!("search: reload {reply:?}");
        }
    });
}
