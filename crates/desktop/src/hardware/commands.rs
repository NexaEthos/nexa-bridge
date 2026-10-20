//! Wiki: docs/wiki/desktop.md

use super::{Card, OBSERVATION_SECONDS, View, cards, send_recorded, state};
use crate::{DesktopContext, err, settings};
use anyhow::{Context, Result};
use nexa_robot::{board, board_inventory};
use serde::Deserialize;
use tauri::State;
use tauri_plugin_opener::OpenerExt;

#[derive(Deserialize)]
pub struct Begin {
    api_origin: String,
    approval_origin: String,
    serial: String,
    label: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Check,
    Send,
    EnableAutomatic,
    Pause,
    OpenApproval,
    Manage,
    Forget,
}

#[tauri::command]
pub async fn hardware_state(context: State<'_, DesktopContext>) -> Result<View, String> {
    let _operation = context.hardware_operations.lock().await;
    Ok(View {
        cards: cards(state::load(&context.dir).map_err(err)?),
        scheduler_error: context.hardware_failure.lock().await.clone(),
    })
}

#[tauri::command]
pub async fn hardware_inventory(
    context: State<'_, DesktopContext>,
) -> Result<Vec<board_inventory::AttachedDevice>, String> {
    let _operation = context.hardware_operations.lock().await;
    board_inventory::inventory().await.map_err(err)
}

#[tauri::command]
pub async fn hardware_inspect(
    context: State<'_, DesktopContext>,
    serial: String,
) -> Result<board::BoardSnapshot, String> {
    let _operation = context.hardware_operations.lock().await;
    board::inspect_uno_q(&serial).await.map_err(err)
}

#[tauri::command]
pub async fn hardware_begin(
    context: State<'_, DesktopContext>,
    request: Begin,
) -> Result<Vec<Card>, String> {
    let context = context.inner();
    let _operation = context.hardware_operations.lock().await;
    begin(context, request).await.map_err(err)?;
    state::load(&context.dir).map(cards).map_err(err)
}

async fn begin(context: &DesktopContext, request: Begin) -> Result<()> {
    let mut devices = state::load(&context.dir)?;
    let api_origin = nexa_link::agora::origin(&request.api_origin)?;
    let approval_origin = nexa_link::agora::origin(&request.approval_origin)?;
    anyhow::ensure!(
        !request.label.trim().is_empty(),
        "Enter a recognizable asset name before pairing"
    );
    let inventory = board_inventory::inventory().await?;
    anyhow::ensure!(
        inventory
            .iter()
            .any(|d| d.serial == request.serial && d.state == "device"),
        "Select an available USB device; refresh the inventory after authorizing USB access"
    );
    let id = settings::generate_api_key()?;
    let token = settings::generate_api_key()?;
    state::store_credential(id.clone(), token).await?;
    let device = state::Record {
        local_id: id.clone(),
        api_origin,
        approval_origin,
        serial: request.serial,
        label: request.label,
        pairing: None,
        expires_at: None,
        device_id: None,
        workspace: None,
        automatic: false,
        next_observation: 0,
        retry_not_before: 0,
        pending_event: None,
        receipt: None,
        error: None,
    };
    devices.push(device);
    if let Err(error) = state::save(&context.dir, &devices) {
        state::remove_credential(id)
            .await
            .context("State write failed; hardware proof cleanup also failed")?;
        return Err(error);
    }
    let index = devices.len() - 1;
    let session = devices[index].session().await?;
    match session.pair(&devices[index].approval_origin).await {
        Ok(pairing) => {
            let uri = pairing.verification_uri.clone();
            devices[index].expires_at = Some(state::now()?.saturating_add(pairing.expires_in));
            devices[index].pairing = Some(pairing);
            state::save(&context.dir, &devices)?;
            context.app.opener().open_url(uri, None::<&str>)?;
            Ok(())
        }
        Err(error) => {
            devices[index].error = Some(error.to_string());
            state::save(&context.dir, &devices)?;
            Err(error.into())
        }
    }
}

#[tauri::command]
pub async fn hardware_action(
    context: State<'_, DesktopContext>,
    local_id: String,
    action: Action,
) -> Result<Vec<Card>, String> {
    let context = context.inner();
    let _operation = context.hardware_operations.lock().await;
    perform(context, &local_id, action).await.map_err(err)?;
    state::load(&context.dir).map(cards).map_err(err)
}

async fn perform(context: &DesktopContext, id: &str, action: Action) -> Result<()> {
    let mut devices = state::load(&context.dir)?;
    let index = devices
        .iter()
        .position(|d| d.local_id == id)
        .context("Device no longer exists; refresh the panel")?;
    match action {
        Action::Send => return send_recorded(context, &mut devices, index).await,
        Action::Check => {
            let response = devices[index].session().await?.device().await;
            match response {
                Ok(device) => {
                    if device.status == "active" {
                        let workspace = device
                            .tenant_slug
                            .context("Approved endpoint has no workspace")?;
                        anyhow::ensure!(
                            devices[index]
                                .device_id
                                .as_ref()
                                .is_none_or(|id| id == &device.id)
                                && devices[index]
                                    .workspace
                                    .as_ref()
                                    .is_none_or(|previous| previous == &workspace),
                            "Approved identity changed; pause and inspect the endpoint in Agora"
                        );
                        devices[index].device_id = Some(device.id);
                        devices[index].workspace = Some(workspace);
                        devices[index].pairing = None;
                        devices[index].expires_at = None;
                        devices[index].error = None;
                    } else {
                        devices[index].error = Some(
                            "Waiting for browser approval. Verify the code and workspace in Agora."
                                .into(),
                        );
                    }
                }
                Err(error) => {
                    let error = anyhow::Error::from(error);
                    devices[index].record_failure(&error, state::now()?);
                    state::save(&context.dir, &devices)?;
                    return Err(error);
                }
            }
        }
        Action::EnableAutomatic => {
            anyhow::ensure!(
                devices[index].workspace.is_some(),
                "Check browser approval before enabling observations"
            );
            devices[index].automatic = true;
            devices[index].next_observation = state::now()?.saturating_add(OBSERVATION_SECONDS);
        }
        Action::Pause => devices[index].automatic = false,
        Action::OpenApproval => {
            let device = &devices[index];
            anyhow::ensure!(
                device
                    .expires_at
                    .is_some_and(|expiry| state::now().is_ok_and(|now| now < expiry)),
                "Approval expired; remove this local request and start pairing again"
            );
            let pairing = device
                .pairing
                .as_ref()
                .context("No browser approval is pending")?;
            context
                .app
                .opener()
                .open_url(pairing.approval_uri(&device.approval_origin)?, None::<&str>)?;
        }
        Action::Manage => {
            let origin = nexa_link::agora::origin(&devices[index].approval_origin)?;
            context
                .app
                .opener()
                .open_url(format!("{origin}/settings/bridge"), None::<&str>)?;
        }
        Action::Forget => forget(context, &mut devices, index).await?,
    }
    state::save(&context.dir, &devices)?;
    context.hardware_wake.notify_one();
    Ok(())
}

async fn forget(
    context: &DesktopContext,
    devices: &mut Vec<state::Record>,
    index: usize,
) -> Result<()> {
    let archive = if devices[index].pending_event.is_some() {
        Some(state::archive(&context.dir, &devices[index])?)
    } else {
        None
    };
    devices[index].automatic = false;
    state::save(&context.dir, devices)?;
    state::remove_credential(devices[index].local_id.clone())
        .await
        .context(
            "Device paused, but keychain cleanup failed; unlock the keychain and retry removal",
        )?;
    devices.remove(index);
    state::save(&context.dir, devices)?;
    if let Some(path) = archive {
        context.app.opener().reveal_item_in_dir(path)?;
    }
    Ok(())
}
