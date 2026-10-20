//! Wiki: docs/wiki/desktop.md

use anyhow::{Context, Result};
use nexa_robot::hardware::{Acknowledged, Error, Pairing, Session, Source};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{io::Write, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub local_id: String,
    pub api_origin: String,
    pub approval_origin: String,
    pub serial: String,
    pub label: String,
    pub pairing: Option<Pairing>,
    pub expires_at: Option<u64>,
    pub device_id: Option<String>,
    pub workspace: Option<String>,
    pub automatic: bool,
    pub next_observation: u64,
    pub retry_not_before: u64,
    pub pending_event: Option<Value>,
    pub receipt: Option<Acknowledged>,
    pub error: Option<String>,
}

impl Record {
    pub async fn session(&self) -> Result<Session> {
        let id = self.local_id.clone();
        let token = tokio::task::spawn_blocking(move || {
            credential(&id)?.get_password().context(
                "Hardware credential unavailable; unlock the system keychain or pair this device again",
            )
        })
        .await
        .context("Hardware credential reader failed; retry after checking the system keychain")??;
        Ok(Session::new(
            &self.api_origin,
            Source::UnoQ {
                serial: self.serial.clone(),
            },
            self.label.clone(),
            token,
        )?)
    }

    pub fn record_failure(&mut self, error: &anyhow::Error, current: u64) {
        if let Some(Error::RateLimited(seconds)) = error.downcast_ref::<Error>() {
            self.retry_not_before = current.saturating_add(*seconds);
        }
        if matches!(
            error.downcast_ref::<Error>(),
            Some(Error::Http {
                status: 401 | 403 | 410,
                ..
            })
        ) {
            self.automatic = false;
        }
        self.error = Some(error.to_string());
    }

    pub const fn due(&self, now: u64) -> bool {
        self.automatic
            && self.device_id.is_some()
            && self.workspace.is_some()
            && now >= self.next_observation
            && now >= self.retry_not_before
    }
}

fn credential(id: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(
        "ai.nexaethos.bridge.desktop",
        &format!("hardware_device:{id}"),
    )
    .context("Cannot open the hardware credential store")
}

pub async fn store_credential(id: String, token: String) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        credential(&id)?
            .set_password(&token)
            .context("Cannot persist hardware proof; no pairing was sent")
    })
    .await
    .context("Hardware credential writer failed; inspect the system keychain before retrying")?
}

pub async fn remove_credential(id: String) -> Result<()> {
    tokio::task::spawn_blocking(move || match credential(&id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error).context("Unlock the system keychain and retry hardware removal"),
    })
    .await
    .context("Hardware credential cleanup failed; inspect the system keychain and retry removal")?
}

pub fn load(dir: &Path) -> Result<Vec<Record>> {
    match std::fs::read(dir.join("hardware.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .context("hardware.json is invalid; preserve it for recovery"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error).context("Cannot read hardware state"),
    }
}

pub fn save(dir: &Path, devices: &[Record]) -> Result<()> {
    std::fs::create_dir_all(dir).context("Cannot create hardware configuration directory")?;
    let path = dir.join(format!(
        "hardware-{}.pending",
        crate::settings::generate_api_key()?
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .context("Cannot create pending hardware state")?;
    file.write_all(&serde_json::to_vec_pretty(devices)?)
        .context("Cannot write hardware state")?;
    file.sync_all().context("Cannot sync hardware state")?;
    drop(file);
    std::fs::rename(&path, dir.join("hardware.json"))
        .context("Cannot commit hardware state; pending file retained")?;
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

pub fn archive(dir: &Path, record: &Record) -> Result<std::path::PathBuf> {
    let path = dir.join(format!(
        "hardware-archive-{}.json",
        crate::settings::generate_api_key()?
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .context("Cannot archive the pending hardware observation")?;
    file.write_all(&serde_json::to_vec_pretty(record)?)?;
    file.sync_all()?;
    drop(file);
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    Ok(path)
}

pub fn now() -> Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("Correct the system clock before sending hardware observations")?
        .as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_and_retry_delay_override_due_observations() -> Result<()> {
        let mut device: Record = serde_json::from_value(serde_json::json!({
            "local_id":"test", "api_origin":"https://api.example.com", "approval_origin":"https://agora.example.com",
            "serial":"test", "label":"board", "pairing":null, "expires_at":null,
            "device_id":"approved", "workspace":"test", "automatic":true,
            "next_observation":100, "retry_not_before":120, "pending_event":{"event_id":"stable"},
            "receipt":null,"error":null
        }))?;
        let pending = device.pending_event.clone();
        for status in [408, 409, 429, 500, 502, 503] {
            device.record_failure(
                &Error::Http {
                    status,
                    refusal: None,
                }
                .into(),
                110,
            );
            assert!(device.automatic);
            assert_eq!(device.pending_event, pending);
        }
        device.record_failure(&Error::RateLimited(30).into(), 90);
        assert_eq!(device.retry_not_before, 120);
        assert!(device.automatic);
        for status in [401, 403, 410] {
            device.automatic = true;
            device.record_failure(
                &Error::Http {
                    status,
                    refusal: None,
                }
                .into(),
                110,
            );
            assert!(!device.automatic);
            assert_eq!(device.pending_event, pending);
        }
        device.automatic = true;
        assert!(!device.due(110));
        assert!(device.due(120));
        device.automatic = false;
        assert!(!device.due(150));
        device.automatic = true;
        device.workspace = None;
        assert!(!device.due(150));
        let dir = std::env::temp_dir().join(format!(
            "nexa-hardware-state-{}",
            crate::settings::generate_api_key()?
        ));
        save(&dir, &[device])?;
        let mut loaded = load(&dir)?;
        assert_eq!(
            loaded[0]
                .pending_event
                .as_ref()
                .and_then(|e| e.get("event_id"))
                .and_then(Value::as_str),
            Some("stable")
        );
        let archived = archive(&dir, &loaded[0])?;
        let preserved: Record = serde_json::from_slice(&std::fs::read(archived)?)?;
        assert_eq!(preserved.pending_event, loaded[0].pending_event);
        loaded[0].automatic = false;
        save(&dir, &loaded)?;
        assert!(!load(&dir)?[0].automatic);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
