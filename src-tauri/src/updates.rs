//! Self-update from GitHub releases (tauri-plugin-updater, signed `latest.json`).

use serde_json::{json, Value};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

/// Ask the release feed whether a newer version exists.
pub async fn check(app: &AppHandle) -> Result<Option<Value>, String> {
    let fail = |e: tauri_plugin_updater::Error| format!("Could not check for updates: {e}");
    let updater = app.updater().map_err(fail)?;
    let update = updater.check().await.map_err(fail)?;
    Ok(update.map(|u| json!({"version": u.version, "current": u.current_version, "notes": u.body.clone().unwrap_or_default()})))
}

/// Download and run the installer for the newest version, then restart.
pub async fn install(app: &AppHandle) -> Result<(), String> {
    let fail = |e: tauri_plugin_updater::Error| format!("Could not install the update: {e}");
    let updater = app.updater().map_err(fail)?;
    match updater.check().await.map_err(fail)? {
        Some(update) => {
            update.download_and_install(|_, _| {}, || {}).await.map_err(fail)?;
            app.restart()
        }
        None => Err("GSpot is up to date.".into()),
    }
}

/// Release builds check 20 s after launch, then every 6 h, and announce each new version once.
pub fn spawn_auto_check(app: AppHandle) {
    if cfg!(debug_assertions) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let mut announced = String::new();
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            match check(&app).await {
                Ok(Some(v)) => {
                    let version = v["version"].as_str().unwrap_or_default().to_string();
                    if version != announced {
                        let _ = app.emit("update://available", &v);
                        announced = version;
                    }
                }
                Ok(None) => {}
                Err(e) => log::info!("{e}"),
            }
            tokio::time::sleep(Duration::from_secs(6 * 60 * 60)).await;
        }
    });
}
