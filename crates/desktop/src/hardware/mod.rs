//! Wiki: docs/wiki/desktop.md

mod commands;
mod state;

use crate::DesktopContext;
use anyhow::{Context, Result};
pub use commands::*;
use nexa_robot::hardware::{Acknowledged, Error};
use serde::Serialize;
use state::{Record, now};
use std::time::Duration;
use tauri::{AppHandle, Manager};

const OBSERVATION_SECONDS: u64 = 15 * 60;

#[derive(Serialize)]
pub struct View {
    cards: Vec<Card>,
    scheduler_error: Option<String>,
}

#[derive(Serialize)]
pub struct Card {
    local_id: String,
    label: String,
    serial: String,
    workspace: Option<String>,
    device_id: Option<String>,
    automatic: bool,
    next_observation: u64,
    retry_not_before: u64,
    pending: bool,
    receipt: Option<Acknowledged>,
    error: Option<String>,
    pairing_code: Option<String>,
    expires_at: Option<u64>,
}

fn cards(devices: Vec<Record>) -> Vec<Card> {
    devices
        .into_iter()
        .map(|device| Card {
            local_id: device.local_id,
            label: device.label,
            serial: device.serial,
            workspace: device.workspace,
            device_id: device.device_id,
            automatic: device.automatic,
            next_observation: device.next_observation,
            retry_not_before: device.retry_not_before,
            pending: device.pending_event.is_some(),
            receipt: device.receipt,
            error: device.error,
            pairing_code: device.pairing.map(|p| p.code),
            expires_at: device.expires_at,
        })
        .collect()
}

async fn send(context: &DesktopContext, devices: &mut [Record], index: usize) -> Result<()> {
    let current = now()?;
    anyhow::ensure!(
        current >= devices[index].retry_not_before,
        "Wait until the retry time shown on this device; the pending observation is retained"
    );
    let workspace = devices[index]
        .workspace
        .clone()
        .context("Check browser approval before sending an observation")?;
    let session = devices[index].session().await?;
    let actual = session.device().await?;
    anyhow::ensure!(
        Some(&actual.id) == devices[index].device_id.as_ref(),
        "Hardware identity changed; preserve pending events and pair this device again"
    );
    devices[index].next_observation = current.saturating_add(OBSERVATION_SECONDS);
    state::save(&context.dir, devices)?;
    if devices[index].pending_event.is_none() {
        let event = session.collect(&workspace).await?;
        devices[index].pending_event = Some(event);
        state::save(&context.dir, devices)?;
    }
    let event = devices[index]
        .pending_event
        .as_ref()
        .context("Pending observation is missing")?;
    match session.deliver(&workspace, event).await {
        Ok(receipt) => {
            devices[index].receipt = Some(receipt);
            devices[index].pending_event = None;
            devices[index].error = None;
            devices[index].retry_not_before = 0;
        }
        Err(error) => {
            if let Error::RateLimited(seconds) = &error {
                devices[index].retry_not_before = now()?.saturating_add(*seconds);
            }
            devices[index].error = Some(error.to_string());
            state::save(&context.dir, devices)?;
            return Err(error.into());
        }
    }
    state::save(&context.dir, devices)
}

async fn send_recorded(
    context: &DesktopContext,
    devices: &mut [Record],
    index: usize,
) -> Result<()> {
    if let Err(error) = send(context, devices, index).await {
        devices[index].record_failure(&error, now()?);
        devices[index].next_observation = now()?.saturating_add(OBSERVATION_SECONDS);
        state::save(&context.dir, devices)?;
        return Err(error);
    }
    Ok(())
}

pub fn start_scheduler(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let delay = scheduler_step(&app).await;
            match delay {
                Ok(seconds) => {
                    let context = app.state::<DesktopContext>();
                    tokio::select! {
                        () = tokio::time::sleep(Duration::from_secs(seconds)) => {},
                        () = context.hardware_wake.notified() => {},
                    }
                }
                Err(error) => {
                    tracing::error!(%error, "hardware scheduler stopped; reopen the app after repairing local state");
                    *app.state::<DesktopContext>().hardware_failure.lock().await = Some(format!(
                        "Automatic hardware observations stopped: {error}. Preserve hardware.json, repair local storage and reopen the app. Manual controls remain available."
                    ));
                    break;
                }
            }
        }
    });
}

async fn scheduler_step(app: &AppHandle) -> Result<u64> {
    let context = app.state::<DesktopContext>();
    let _operation = context.hardware_operations.lock().await;
    let mut devices = state::load(&context.dir)?;
    let current = now()?;
    for index in 0..devices.len() {
        if devices[index].due(current)
            && let Err(error) = send_recorded(&context, &mut devices, index).await
        {
            tracing::warn!(%error, "scheduled hardware observation failed; pending event retained");
        }
    }
    let current = now()?;
    Ok(devices
        .iter()
        .filter(|d| d.automatic && d.workspace.is_some())
        .map(|d| {
            d.next_observation
                .max(d.retry_not_before)
                .saturating_sub(current)
                .max(1)
        })
        .min()
        .unwrap_or(OBSERVATION_SECONDS)
        .min(OBSERVATION_SECONDS))
}
