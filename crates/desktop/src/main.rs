//! # Wiki: docs/wiki/managed-auth.md
#![forbid(unsafe_code)]
#![deny(warnings, rust_2018_idioms, rust_2021_compatibility)]
#![warn(missing_docs, missing_debug_implementations)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod diagnostics;
mod hardware;
mod managed;
mod settings;
mod supervisor;
mod tray;
mod updates;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State, WindowEvent};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_opener::OpenerExt;

use nexa_link::{guides, runtimes};

use crate::settings::Settings;
use crate::supervisor::{Status, Supervisor};

struct DesktopContext {
    dir: PathBuf,
    app: AppHandle,
    operations: tokio::sync::Mutex<()>,
    hardware_operations: tokio::sync::Mutex<()>,
    hardware_wake: tokio::sync::Notify,
    hardware_failure: tokio::sync::Mutex<Option<String>>,
}

#[derive(Serialize)]
struct UiState {
    settings: Settings,
    has_token: bool,
    has_api_key: bool,
    ready: bool,
    next_step: String,
    status: Status,
    build: nexa_tunnel_protocol::BuildInfo,
}

#[derive(Deserialize)]
struct SaveRequest {
    settings: Settings,
}

fn ui_state(app: &AppHandle) -> Result<UiState, String> {
    let dir = app.state::<DesktopContext>().dir.clone();
    let settings = Settings::load(&dir).map_err(err)?;
    let has_token = match &settings.managed {
        Some(identity) => settings::managed_device(&identity.device_id)
            .map_err(err)?
            .is_some(),
        None => false,
    };
    let has_api_key =
        settings.managed.is_some() && settings::managed_local().map_err(err)?.is_some();
    let next_step = if settings.pending_pairing.is_some() {
        "Approve this computer in your browser."
    } else if !has_token || !has_api_key {
        "Connect to Agora to link this computer."
    } else if settings.selected_model.is_empty() {
        "Use Find local models, then select a loaded model."
    } else {
        "Ready to connect. Credentials are managed automatically; LM Studio needs no API key."
    }
    .to_string();
    Ok(UiState {
        ready: settings.pending_pairing.is_none()
            && settings
                .managed
                .as_ref()
                .is_none_or(|identity| identity.confirmed)
            && settings.is_complete(has_token, has_api_key),
        next_step,
        settings,
        has_token,
        has_api_key,
        status: app.state::<Supervisor>().status(),
        build: diagnostics::build(),
    })
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
async fn get_state(context: State<'_, DesktopContext>) -> Result<UiState, String> {
    let context = context.inner();
    let _operation = context.operations.lock().await;
    ui_state(&context.app)
}

#[tauri::command]
async fn save(context: State<'_, DesktopContext>, mut req: SaveRequest) -> Result<UiState, String> {
    let context = context.inner();
    let app = &context.app;
    let _operation = context.operations.lock().await;
    if app.state::<Supervisor>().is_running() {
        return Err("Disconnect before saving settings or rotating credentials.".into());
    }
    let previous = Settings::load(&context.dir).map_err(err)?;
    req.settings.managed = previous.managed;
    req.settings.pending_pairing = previous.pending_pairing;
    req.settings.relay_url = previous.relay_url;
    if req.settings.managed.is_some() {
        req.settings.agora_url = previous.agora_url;
        req.settings.approval_url = previous.approval_url;
    }
    req.settings.validate().map_err(err)?;
    let dir = context.dir.clone();
    let manager = app.autolaunch();
    if req.settings.autostart {
        manager.enable()
    } else {
        manager.disable()
    }.map_err(|e| format!("Autostart was not applied: {e}. Connection remains stopped; check settings and save again."))?;
    req.settings.save(&dir).map_err(err)?;
    ui_state(app)
}

#[tauri::command]
async fn detect_runtimes() -> Vec<runtimes::Runtime> {
    runtimes::detect().await
}

#[tauri::command]
async fn probe_runtime(url: String) -> Result<runtimes::Runtime, String> {
    runtimes::probe_one(&url).await
}

#[tauri::command]
async fn get_guides(
    context: State<'_, DesktopContext>,
) -> Result<Vec<guides::WorkspacePreset>, String> {
    let context = context.inner();
    let _operation = context.operations.lock().await;
    let cfg = Settings::load(&context.dir).map_err(err)?;
    managed::guides(&cfg).await.map_err(err)
}

#[tauri::command]
async fn set_running(context: State<'_, DesktopContext>, running: bool) -> Result<UiState, String> {
    let context = context.inner();
    let app = &context.app;
    let _operation = context.operations.lock().await;
    let sup = app.state::<Supervisor>();
    if running {
        start(app, &sup, true).await?;
    } else {
        sup.stop(app)?;
    }
    ui_state(app)
}

async fn start(
    app: &AppHandle,
    sup: &State<'_, Supervisor>,
    confirm_workspace: bool,
) -> Result<(), String> {
    let dir = app.state::<DesktopContext>().dir.clone();
    let mut cfg = Settings::load(&dir).map_err(err)?;
    cfg.validate().map_err(err)?;
    if cfg.pending_pairing.is_some() {
        return Err("Complete or restart the pending browser approval before connecting.".into());
    }
    let identity = cfg
        .managed
        .as_ref()
        .ok_or("Link this computer to Agora before connecting.")?;
    let token = settings::managed_device(&identity.device_id)
        .map_err(err)?
        .unwrap_or_default();
    let key = settings::managed_local().map_err(err)?.unwrap_or_default();
    if !cfg.is_complete(!token.is_empty(), !key.is_empty()) {
        return Err("settings are incomplete".to_string());
    }
    let runtime = runtimes::probe_one(&cfg.upstream_url).await?;
    let available = runtime.loaded_models.as_ref().unwrap_or(&runtime.models);
    if !available.contains(&cfg.selected_model) {
        return Err("The selected model is not available. Load it in your model server, then use Find local models again.".into());
    }
    if let Some(identity) = &mut cfg.managed {
        if !identity.confirmed && !confirm_workspace {
            return Err(
                "Confirm the approved workspace in the desktop panel before connecting.".into(),
            );
        }
        identity.confirmed = true;
    }
    managed::prepare(&cfg, runtime.flavor).await.map_err(err)?;
    cfg.save(&dir).map_err(err)?;
    sup.start(app, &cfg, &token, &key)
}

#[tauri::command]
async fn begin_link(
    context: State<'_, DesktopContext>,
    agora_url: String,
    approval_url: String,
    label: String,
) -> Result<UiState, String> {
    let context = context.inner();
    let _operation = context.operations.lock().await;
    if context.app.state::<Supervisor>().is_running() {
        return Err("Disconnect before linking a computer.".into());
    }
    let mut cfg = Settings::load(&context.dir).map_err(err)?;
    let pending = managed::begin(&context.dir, &mut cfg, &agora_url, &approval_url, &label)
        .await
        .map_err(err)?;
    context
        .app
        .opener()
        .open_url(pending.verification_uri, None::<&str>)
        .map_err(err)?;
    ui_state(&context.app)
}

#[tauri::command]
async fn poll_link(context: State<'_, DesktopContext>) -> Result<UiState, String> {
    let context = context.inner();
    let _operation = context.operations.lock().await;
    if context.app.state::<Supervisor>().is_running() {
        return Err("Disconnect before completing linking.".into());
    }
    let mut cfg = Settings::load(&context.dir).map_err(err)?;
    managed::poll(&context.dir, &mut cfg).await.map_err(err)?;
    ui_state(&context.app)
}

#[tauri::command]
async fn cancel_link(context: State<'_, DesktopContext>) -> Result<UiState, String> {
    let context = context.inner();
    let _operation = context.operations.lock().await;
    if context.app.state::<Supervisor>().is_running() {
        return Err("Disconnect before canceling linking.".into());
    }
    let mut cfg = Settings::load(&context.dir).map_err(err)?;
    managed::clear_pending(&context.dir, &mut cfg).map_err(err)?;
    ui_state(&context.app)
}

#[tauri::command]
async fn open_approval(context: State<'_, DesktopContext>) -> Result<(), String> {
    let context = context.inner();
    let _operation = context.operations.lock().await;
    let cfg = Settings::load(&context.dir).map_err(err)?;
    let pending = cfg.pending_pairing.ok_or("No linking request is pending")?;
    context
        .app
        .opener()
        .open_url(pending.verification_uri, None::<&str>)
        .map_err(err)
}

fn toggle(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let context = app.state::<DesktopContext>();
        let _operation = context.operations.lock().await;
        let sup = app.state::<Supervisor>();
        let result = if sup.is_running() {
            sup.stop(&app)
        } else {
            start(&app, &sup, false).await
        };
        if let Err(error) = result {
            tracing::warn!(%error, "connection action failed");
            tray::show_panel(&app);
        }
    });
}

#[tauri::command]
async fn export_diagnostics(context: State<'_, DesktopContext>) -> Result<String, String> {
    let context = context.inner();
    let _operation = context.operations.lock().await;
    let settings = Settings::load(&context.dir).map_err(err)?;
    let status = context.app.state::<Supervisor>().status();
    let path = diagnostics::export(&context.dir, &settings, &status).map_err(err)?;
    context
        .app
        .opener()
        .reveal_item_in_dir(&path)
        .map_err(err)?;
    Ok(path.display().to_string())
}

fn main() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("could not install the desktop TLS provider"))?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            tray::show_panel(app);
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(Supervisor::default())
        .manage(updates::Pending::default())
        .invoke_handler(tauri::generate_handler![
            hardware::hardware_state,
            hardware::hardware_inventory,
            hardware::hardware_inspect,
            hardware::hardware_begin,
            hardware::hardware_action,
            get_state,
            save,
            set_running,
            detect_runtimes,
            probe_runtime,
            get_guides,
            begin_link,
            poll_link,
            cancel_link,
            open_approval,
            export_diagnostics,
            updates::check_update,
            updates::install_update
        ])
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            app.manage(DesktopContext {
                dir,
                app: app.handle().clone(),
                operations: tokio::sync::Mutex::new(()),
                hardware_operations: tokio::sync::Mutex::new(()),
                hardware_wake: tokio::sync::Notify::new(),
                hardware_failure: tokio::sync::Mutex::new(None),
            });

            hardware::start_scheduler(app.handle());
            let menu = tray::build(app.handle())?;
            if let Some(icon) = app.tray_by_id("main") {
                icon.set_menu(Some(menu))?;
                icon.on_menu_event(|app, event| tray::on_menu_event(app, event.id().as_ref()));
                icon.on_tray_icon_event(|icon, event| {
                    tray::on_tray_event(icon.app_handle(), &event);
                });
            }

            let handle = app.handle().clone();
            match ui_state(&handle) {
                Ok(state) if state.ready => {
                    toggle(&handle);
                }
                Ok(_) => tray::show_panel(&handle),
                Err(e) => {
                    tracing::error!(error = %e, "cannot load desktop state");
                    tray::show_panel(&handle);
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Err(error) = window.hide() {
                    tracing::error!(error = %error, "could not hide settings panel");
                }
            }
        })
        .build(tauri::generate_context!())?
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                supervisor::shutdown(app);
            }
        });
    Ok(())
}
