//! Updates (PRD F-UPD-01/02, TRD §15.4): a check once a day (and on "Check Now"), against the release feed baked in at
//! build time (`TROVARA_UPDATE_URL`: GitHub Releases' latest.json). A new version downloads in the background, is
//! checked against the minisign key compiled in, and installs on "Restart to Update" or when Trovara quits. Nothing
//! about the person's files is sent: the request is a plain download of latest.json.

use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

/// The release feed, or None in builds without one (dev builds): no checks then.
pub const ENDPOINT: Option<&str> = option_env!("TROVARA_UPDATE_URL");
/// The update signing key's public half (`tauri signer generate`; the private one never leaves the release machine).
const PUBLIC_KEY: &str = include_str!("../updater.pub");

#[derive(Default)]
pub struct Updates {
    /// A downloaded, verified update waiting to be installed.
    ready: Mutex<Option<(Update, Vec<u8>)>>,
    checking: Mutex<bool>,
}

impl Updates {
    pub fn ready_version(&self) -> Option<String> {
        self.ready.lock().unwrap().as_ref().map(|(update, _)| update.version.clone())
    }
}

/// Checks for a new version and downloads it. {"state": "off" | "current" | "ready" | "busy", "version"?, "message"?}
pub async fn check(app: &AppHandle) -> Result<Value, String> {
    let Some(endpoint) = ENDPOINT.filter(|e| !e.is_empty()) else {
        return Ok(json!({"state": "off", "message": "This build doesn't check for updates."}));
    };
    let updates = app.state::<Updates>();
    if let Some(version) = updates.ready_version() {
        return Ok(json!({"state": "ready", "version": version}));
    }
    {
        let mut checking = updates.checking.lock().unwrap();
        if *checking {
            return Ok(json!({"state": "busy"}));
        }
        *checking = true;
    }
    let result = fetch(app, endpoint).await;
    *updates.checking.lock().unwrap() = false;
    let found = result?;
    let Some((update, bytes)) = found else {
        return Ok(json!({"state": "current", "version": app.package_info().version.to_string()}));
    };
    let version = update.version.clone();
    *updates.ready.lock().unwrap() = Some((update, bytes));
    let _ = app.emit("update", json!({"state": "ready", "version": version}));
    Ok(json!({"state": "ready", "version": version}))
}

async fn fetch(app: &AppHandle, endpoint: &str) -> Result<Option<(Update, Vec<u8>)>, String> {
    let url = endpoint.parse().map_err(|error| format!("the update address is wrong: {error}"))?;
    let updater = app
        .updater_builder()
        .endpoints(vec![url])
        .map_err(|error| error.to_string())?
        .pubkey(PUBLIC_KEY.trim())
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|error| error.to_string())?;
    let Some(update) = updater.check().await.map_err(|error| format!("couldn't check: {error}"))? else {
        return Ok(None);
    };
    // The signature is checked as it downloads; a download that doesn't match fails here.
    let bytes = update.download(|_, _| {}, || {}).await.map_err(|error| format!("couldn't download: {error}"))?;
    Ok(Some((update, bytes)))
}

/// Installs the downloaded update, if there is one: the installer runs (passive, keeping the index and the model) and
/// starts Trovara again. False: nothing to install.
pub fn install_ready(app: &AppHandle) -> bool {
    let Some((update, bytes)) = app.state::<Updates>().ready.lock().unwrap().take() else {
        return false;
    };
    match update.install(bytes) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("trovara: the update didn't install: {error}");
            false
        }
    }
}

/// The daily check, a minute after start and then every 24 hours, while the setting allows it.
/// `on_ready`: a version was downloaded (the tray says so).
pub fn start_daily(
    app: AppHandle,
    allowed: impl Fn(&AppHandle) -> bool + Send + 'static,
    on_ready: impl Fn(&AppHandle) + Send + 'static,
) {
    if ENDPOINT.is_none_or(str::is_empty) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        loop {
            if allowed(&app) {
                match check(&app).await {
                    Ok(answer) if answer["state"] == "ready" => on_ready(&app),
                    Ok(_) => {}
                    Err(error) => eprintln!("trovara: {error}"),
                }
            }
            tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
        }
    });
}
