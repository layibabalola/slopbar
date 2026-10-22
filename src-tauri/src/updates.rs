//! Self-update from GitHub releases (signed; see `plugins.updater` in tauri.conf.json).

use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::AppState;

const FIRST_CHECK: Duration = Duration::from_secs(20);
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Clone, Serialize)]
pub struct UpdateInfo {
    pub current: String,
    /// The newer version available, if any.
    pub latest: Option<String>,
}

pub async fn check(app: &AppHandle) -> Result<UpdateInfo, String> {
    if !cfg!(feature = "upstream-updates") { return Err("Manual updates only in this build".into()); }
    let current = app.package_info().version.to_string();
    let update = updater(app)?.check().await.map_err(|e| e.to_string())?;
    Ok(UpdateInfo { current, latest: update.map(|u| u.version) })
}

/// Ultra release builds pin the fork's channel and key in code, so a merged config change cannot
/// point them back at upstream, and accept only newer Ultra versions.
fn updater(app: &AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    let builder = app.updater_builder();
    #[cfg(feature = "ultra-updates")]
    let builder = builder
        .endpoints(vec![crate::ultra_release::UPDATE_ENDPOINT.parse::<tauri::Url>().map_err(|e| e.to_string())?])
        .map_err(|e| e.to_string())?
        .pubkey(crate::ultra_release::PUBKEY)
        .version_comparator(|current, remote| {
            crate::ultra_release::is_ultra_version(&remote.version.to_string()) && remote.version > current
        });
    builder.build().map_err(|e| e.to_string())
}

/// Downloads and installs the newest release, then restarts into it.
pub async fn install(app: &AppHandle) -> Result<(), String> {
    if !cfg!(feature = "upstream-updates") { return Err("Manual updates only in this build".into()); }
    let Some(update) = updater(app)?.check().await.map_err(|e| e.to_string())? else {
        return Ok(());
    };
    let _ = app.emit("update-installing", &update.version);
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    app.restart();
}

/// Checks shortly after launch and then every few hours; installs when auto-update is on.
pub async fn run_loop(app: AppHandle) {
    tokio::time::sleep(FIRST_CHECK).await;
    loop {
        let auto = app.state::<AppState>().settings.lock().unwrap().auto_update;
        if auto {
            if let Ok(UpdateInfo { latest: Some(_), .. }) = check(&app).await {
                let _ = install(&app).await;
            }
        }
        tokio::time::sleep(EVERY).await;
    }
}
