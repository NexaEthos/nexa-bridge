//! Desktop side of the managed link: settings and keychain around the shared client. Wiki: docs/wiki/managed-auth.md

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use nexa_link::agora::{self, PairingEnded};
use nexa_link::guides::WorkspacePreset;
use nexa_link::runtimes::{self, Flavor};
use nexa_link::secret;

use crate::settings::{self, ManagedIdentity, PendingPairing, Settings};

/// Forget a pairing that cannot complete: the settings row and the pending proof.
pub fn clear_pending(dir: &Path, cfg: &mut Settings) -> Result<()> {
    cfg.pending_pairing = None;
    cfg.save(dir)?;
    settings::set_managed_pending("")?;
    Ok(())
}

pub async fn begin(
    dir: &Path,
    cfg: &mut Settings,
    agora_url: &str,
    approval_url: &str,
    label: &str,
) -> Result<PendingPairing> {
    anyhow::ensure!(
        cfg.pending_pairing.is_none(),
        "Complete or cancel the pending approval before starting another"
    );
    let origin = agora::origin(agora_url)?;
    let approval = agora::origin(if approval_url.trim().is_empty() {
        agora_url
    } else {
        approval_url
    })?;
    let proof = secret::generate()?;
    settings::set_managed_pending(&proof)?;
    let started = agora::start_pairing(&origin, &approval, label, &proof).await?;
    let pending = PendingPairing {
        agora_url: origin,
        approval_url: Some(approval),
        code: started.code,
        verification_uri: started.verification_uri,
        expires_at: started.expires_at,
    };
    cfg.pending_pairing = Some(pending.clone());
    cfg.save(dir)?;
    Ok(pending)
}

pub async fn poll(dir: &Path, cfg: &mut Settings) -> Result<()> {
    let pending = cfg
        .pending_pairing
        .as_ref()
        .context("No linking request is pending")?;
    if pending.expires_at <= SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() {
        clear_pending(dir, cfg)?;
        anyhow::bail!("Approval expired. Click Connect to Agora to start again.");
    }
    let proof = settings::managed_pending()?
        .context("Pending device credential is missing; start linking again")?;
    let origin = agora::origin(&pending.agora_url)?;
    let device = match agora::device_status(&origin, &proof).await {
        Ok(device) => device,
        Err(error) if error.downcast_ref::<PairingEnded>().is_some() => {
            clear_pending(dir, cfg)?;
            return Err(error);
        }
        Err(error) => return Err(error),
    };
    if device.status == "pending" {
        return Ok(());
    }
    if device.status != "active" {
        clear_pending(dir, cfg)?;
        anyhow::bail!("Computer was not approved; link it again");
    }
    let identity = ManagedIdentity {
        confirmed: false,
        device_id: device.id,
        tenant_slug: device
            .tenant_slug
            .context("Approved device has no workspace")?,
        connection_slug: device
            .connection_slug
            .context("Approved device has no connection")?,
    };
    let relay = device
        .relay_url
        .context("Approved device has no relay address")?;
    let mut updated = cfg.clone();
    updated.relay_url = relay;
    updated.approval_url = agora::origin(
        pending
            .approval_url
            .as_deref()
            .unwrap_or(&pending.agora_url),
    )?;
    updated.agora_url = origin;
    updated.managed = Some(identity);
    updated.pending_pairing = None;
    updated.validate()?;
    settings::ensure_managed_local()?;
    settings::set_managed_device(
        &updated
            .managed
            .as_ref()
            .context("Approved identity is missing")?
            .device_id,
        &proof,
    )?;
    updated.save(dir)?;
    *cfg = updated;
    settings::set_managed_pending("")?;
    Ok(())
}

pub async fn guides(cfg: &Settings) -> Result<Vec<WorkspacePreset>> {
    let identity = cfg
        .managed
        .as_ref()
        .context("Link this computer to Agora to read your workspace's models")?;
    let proof = settings::managed_device(&identity.device_id)?
        .context("Device credential is missing; link this computer again")?;
    agora::guides(&agora::origin(&cfg.agora_url)?, &proof).await
}

/// The window LM Studio loaded for the selected id, reported when readable; never a condition to connect.
async fn loaded_context(cfg: &Settings, runtime: Flavor) -> Option<u32> {
    if runtime != Flavor::LmStudio {
        return None;
    }
    match runtimes::lmstudio_loaded_context(&cfg.upstream_url, &cfg.selected_model).await {
        Ok(context) => Some(context),
        Err(error) => {
            tracing::warn!(error = %error, "loaded context unavailable; published without it");
            None
        }
    }
}

pub async fn prepare(cfg: &Settings, runtime: Flavor) -> Result<()> {
    let identity = cfg.managed.as_ref().context("Computer is not linked")?;
    let origin = agora::origin(&cfg.agora_url)?;
    let proof = settings::managed_device(&identity.device_id)?
        .context("Device credential is missing; link this computer again")?;
    let device = agora::device_status(&origin, &proof).await?;
    anyhow::ensure!(
        device.status == "active"
            && device.id == identity.device_id
            && device.tenant_slug.as_deref() == Some(&identity.tenant_slug)
            && device.connection_slug.as_deref() == Some(&identity.connection_slug)
            && agora::same_relay(device.relay_url.as_deref(), &cfg.relay_url),
        "The approved connection changed. Link this computer again before connecting."
    );
    let context_window = loaded_context(cfg, runtime).await;
    agora::publish_model(
        &origin,
        &proof,
        &cfg.selected_model,
        runtime,
        context_window,
    )
    .await
}
