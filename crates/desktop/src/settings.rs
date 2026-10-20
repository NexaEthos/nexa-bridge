//! # Wiki: docs/wiki/managed-auth.md

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const KEYCHAIN_SERVICE: &str = "ai.nexaethos.bridge.desktop";
const MANAGED_PENDING: &str = "managed_pending";
const MANAGED_LOCAL: &str = "managed_local";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManagedIdentity {
    #[serde(default)]
    pub confirmed: bool,
    pub device_id: String,
    pub tenant_slug: String,
    pub connection_slug: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingPairing {
    pub agora_url: String,
    #[serde(default)]
    pub approval_url: Option<String>,
    pub code: String,
    pub verification_uri: String,
    pub expires_at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub agora_url: String,
    pub approval_url: String,
    pub managed: Option<ManagedIdentity>,
    pub pending_pairing: Option<PendingPairing>,
    pub upstream_url: String,
    pub selected_model: String,
    pub listen_addr: String,
    pub relay_url: String,
    pub autostart: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            agora_url: option_env!("NEXA_AGORA_URL")
                .unwrap_or_default()
                .to_string(),
            approval_url: String::new(),
            managed: None,
            pending_pairing: None,
            upstream_url: "http://127.0.0.1:11434".to_string(),
            selected_model: String::new(),
            listen_addr: "127.0.0.1:8088".to_string(),
            relay_url: String::new(),
            autostart: false,
        }
    }
}

impl Settings {
    pub fn load(dir: &Path) -> Result<Self> {
        match std::fs::read_to_string(Self::path(dir)) {
            Ok(json) => serde_json::from_str(&json)
                .context("settings.json is invalid; repair it before connecting"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).context("cannot read settings.json"),
        }
    }

    pub fn validate(&mut self) -> Result<()> {
        anyhow::ensure!(
            self.selected_model.trim() == self.selected_model
                && !self.selected_model.chars().any(char::is_control),
            "selected model must be an exact id without surrounding whitespace or control characters"
        );
        let upstream = nexa_link::runtimes::normalize_url(&self.upstream_url)
            .context("enter the model server address")?;
        let url = reqwest::Url::parse(&upstream).context("invalid model server address")?;
        anyhow::ensure!(
            matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
            "model server must use HTTP or HTTPS"
        );
        anyhow::ensure!(
            url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/",
            "model server must contain only scheme, host and port; no credentials or path"
        );
        self.upstream_url = upstream;
        let address: std::net::SocketAddr = self
            .listen_addr
            .trim()
            .parse()
            .context("invalid bridge listen address")?;
        anyhow::ensure!(
            address.ip().is_loopback() && address.port() != 0,
            "desktop bridge must listen on a loopback address with a nonzero port"
        );
        self.listen_addr = address.to_string();
        let relay = reqwest::Url::parse(self.relay_url.trim()).context("invalid relay URL")?;
        anyhow::ensure!(
            relay.scheme() == "wss" && relay.host_str().is_some(),
            "relay must use encrypted wss:// transport"
        );
        anyhow::ensure!(
            relay.username().is_empty()
                && relay.password().is_none()
                && relay.query().is_none()
                && relay.fragment().is_none(),
            "relay credentials belong in the token field, not in the URL"
        );
        self.relay_url = relay.to_string();
        Ok(())
    }

    /// Written owner-only and synced before the rename: no other user reads it, no power cut halves it.
    pub fn save(&self, dir: &Path) -> Result<()> {
        use std::io::Write;

        std::fs::create_dir_all(dir).context("create config dir")?;
        let json = serde_json::to_string_pretty(self).context("serialize settings")?;
        let temporary = dir.join("settings.json.pending");
        let mut file = std::fs::File::create(&temporary).context("create pending settings")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .context("restrict pending settings")?;
        }
        file.write_all(json.as_bytes())
            .context("write pending settings")?;
        file.sync_all().context("sync pending settings")?;
        drop(file);
        std::fs::rename(temporary, Self::path(dir)).context("commit settings.json")?;
        #[cfg(unix)]
        std::fs::File::open(dir)
            .and_then(|directory| directory.sync_all())
            .context("sync config dir")?;
        Ok(())
    }

    fn path(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }

    pub fn is_complete(&self, has_token: bool, has_api_key: bool) -> bool {
        !self.upstream_url.trim().is_empty()
            && !self.selected_model.is_empty()
            && !self.listen_addr.trim().is_empty()
            && !self.relay_url.trim().is_empty()
            && has_token
            && has_api_key
    }
}

fn entry(name: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYCHAIN_SERVICE, name).context("open keychain entry")
}

fn read(name: &str) -> Result<Option<String>> {
    match entry(name)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e).context("read from keychain"),
    }
}

fn write(name: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return match entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e).context("delete keychain entry"),
        };
    }
    entry(name)?
        .set_password(value)
        .context("write to keychain")
}

pub fn managed_device(device_id: &str) -> Result<Option<String>> {
    read(&format!("managed_device:{device_id}"))
}
pub fn set_managed_device(device_id: &str, value: &str) -> Result<()> {
    write(&format!("managed_device:{device_id}"), value)
}
pub fn managed_pending() -> Result<Option<String>> {
    read(MANAGED_PENDING)
}
pub fn set_managed_pending(value: &str) -> Result<()> {
    write(MANAGED_PENDING, value)
}
pub fn managed_local() -> Result<Option<String>> {
    read(MANAGED_LOCAL)
}
pub fn ensure_managed_local() -> Result<String> {
    if let Some(key) = read(MANAGED_LOCAL)? {
        anyhow::ensure!(!key.is_empty(), "local credential is empty");
        return Ok(key);
    }
    let key = generate_api_key()?;
    write(MANAGED_LOCAL, &key)?;
    Ok(key)
}

pub fn generate_api_key() -> Result<String> {
    nexa_link::secret::generate()
}

/// A token the supervisor gives each bridge it starts, echoed by `/healthz`; never persisted.
pub fn instance_id() -> Result<String> {
    nexa_link::secret::random_hex::<16>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_settings_file_from_the_profile_era_still_loads() -> Result<()> {
        let restored: Settings = serde_json::from_value(serde_json::json!({
            "selected_model": "publisher/model-4bit",
            "selected_profile": {"key": "model", "revision": 1}
        }))?;
        assert_eq!(restored.selected_model, "publisher/model-4bit");
        assert!(
            serde_json::to_value(&restored)?
                .get("selected_profile")
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn selection_is_required_for_connection_and_persists_in_settings() -> Result<()> {
        let mut settings = configured();
        assert!(!settings.is_complete(true, true));
        settings.selected_model = "model/selected".into();
        assert!(settings.is_complete(true, true));
        let restored: Settings = serde_json::from_str(&serde_json::to_string(&settings)?)?;
        assert_eq!(restored.selected_model, "model/selected");
        let missing: Settings = serde_json::from_str("{}")?;
        assert_eq!(missing.selected_model, "");
        settings.selected_model = "bad\nmodel".into();
        assert!(settings.validate().is_err());
        Ok(())
    }

    #[test]
    fn pending_approval_origin_does_not_replace_the_active_origin() -> Result<()> {
        let mut cfg: Settings = serde_json::from_value(serde_json::json!({
            "agora_url": "https://active.example.com",
            "pending_pairing": {
                "agora_url": "https://next.example.com", "code": "code",
                "verification_uri": "https://next.example.com/bridge-pair#code",
                "expires_at": 42
            }
        }))?;
        let pending = cfg
            .pending_pairing
            .as_mut()
            .context("Missing pending pairing")?;
        assert!(pending.approval_url.is_none());
        pending.approval_url = Some("https://approval.example.com".into());
        let restored: Settings = serde_json::from_value(serde_json::to_value(&cfg)?)?;
        assert_eq!(restored.agora_url, "https://active.example.com");
        assert_eq!(restored.approval_url, "");
        assert_eq!(
            restored
                .pending_pairing
                .context("Missing restored pairing")?
                .approval_url
                .as_deref(),
            Some("https://approval.example.com")
        );
        Ok(())
    }

    fn configured() -> Settings {
        Settings {
            relay_url: "wss://relay.example.com/_tunnel".into(),
            ..Settings::default()
        }
    }

    #[test]
    fn validates_and_normalizes_connection_addresses() -> Result<()> {
        let mut cfg = configured();
        cfg.upstream_url = "127.0.0.1:1234/v1/".into();
        cfg.validate()?;
        assert_eq!(cfg.upstream_url, "http://127.0.0.1:1234");
        for address in [
            "ws://relay.example.com/_tunnel",
            "wss://user:secret@relay.example.com/_tunnel",
            "wss://relay.example.com/_tunnel?token=secret",
        ] {
            let mut cfg = configured();
            cfg.relay_url = address.into();
            assert!(cfg.validate().is_err());
        }
        for address in ["0.0.0.0:8088", "127.0.0.1:0", "invalid"] {
            let mut cfg = configured();
            cfg.listen_addr = address.into();
            assert!(cfg.validate().is_err());
        }
        Ok(())
    }

    #[test]
    fn missing_and_corrupt_settings_are_distinct() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("nexa-settings-{}", generate_api_key()?));
        assert_eq!(
            Settings::load(&dir)?.listen_addr,
            Settings::default().listen_addr
        );
        std::fs::create_dir(&dir)?;
        let file = dir.join("settings.json");
        std::fs::write(&file, "{")?;
        assert!(Settings::load(&dir).is_err());
        std::fs::remove_file(file)?;
        std::fs::remove_dir(dir)?;
        Ok(())
    }
}
