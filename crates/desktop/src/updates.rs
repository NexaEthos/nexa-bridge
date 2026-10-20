//! Signed self-updates from the published release. See docs/wiki/desktop.md#updates

use serde::Serialize;
use tauri::{Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::supervisor::Supervisor;
use crate::{DesktopContext, err};

#[derive(Default)]
pub struct Pending(tokio::sync::Mutex<Option<Update>>);

#[derive(Serialize)]
pub struct Available {
    version: String,
    current: String,
    notes: Option<String>,
}

#[tauri::command]
pub async fn check_update(
    context: State<'_, DesktopContext>,
    pending: State<'_, Pending>,
) -> Result<Option<Available>, String> {
    let found = context
        .app
        .updater()
        .map_err(err)?
        .check()
        .await
        .map_err(|e| format!("Update check failed: {e}"))?;
    let available = found.as_ref().map(|update| Available {
        version: update.version.clone(),
        current: update.current_version.clone(),
        notes: update.body.clone(),
    });
    *pending.0.lock().await = found;
    Ok(available)
}

#[tauri::command]
pub async fn install_update(
    context: State<'_, DesktopContext>,
    pending: State<'_, Pending>,
) -> Result<(), String> {
    let context = context.inner();
    let update = pending
        .0
        .lock()
        .await
        .clone()
        .ok_or("No update is ready to install; check for updates first.")?;
    let bytes = update.download(|_, _| {}, || {}).await.map_err(|e| {
        format!("The update did not download: {e}. The connection was left as it was.")
    })?;
    let _hardware = context.hardware_operations.lock().await;
    let _operation = context.operations.lock().await;
    context
        .app
        .state::<Supervisor>()
        .stop(&context.app)
        .map_err(|e| {
            format!("The update was not installed because the connection did not stop: {e}")
        })?;
    update
        .install(bytes)
        .map_err(|e| format!("The update was not installed: {e}. Connect again to resume."))?;
    context.app.restart()
}
