//! The server's own files: its link in `server.json`, its secrets in owner-only env files. Wiki: docs/wiki/server-setup.md

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Where the server keeps its link: `$XDG_CONFIG_HOME/nexa-bridge`, else `~/.config/nexa-bridge`.
pub struct Paths {
    pub dir: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Self> {
        let base = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
            Some(dir) => PathBuf::from(dir),
            None => {
                PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".config")
            }
        };
        Ok(Self {
            dir: base.join("nexa-bridge"),
        })
    }

    pub fn server(&self) -> PathBuf {
        self.dir.join("server.json")
    }

    pub fn bridge_env(&self) -> PathBuf {
        self.dir.join("bridge.env")
    }

    pub fn connector_env(&self) -> PathBuf {
        self.dir.join("connector.env")
    }

    /// The pool's members, with their keys: the source the bridge's `config.toml` is written from.
    pub fn pool(&self) -> PathBuf {
        self.dir.join("pool.json")
    }

    /// The bridge's `config.toml`, read from its working directory: the pool's routes.
    pub fn bridge_config(&self) -> PathBuf {
        self.dir.join("config.toml")
    }

    pub fn pending(&self) -> PathBuf {
        self.dir.join("pending-proof")
    }
}

/// The non-secret description of the link and of what this server serves.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Server {
    pub agora_url: String,
    pub approval_url: String,
    pub device_id: String,
    pub tenant_slug: String,
    pub connection_slug: String,
    pub relay_url: String,
    pub upstream_url: String,
    pub runtime: String,
    pub selected_model: String,
    pub listen_addr: String,
}

impl Server {
    pub fn load(paths: &Paths) -> Result<Option<Self>> {
        match fs::read_to_string(paths.server()) {
            Ok(json) => Ok(Some(serde_json::from_str(&json).context(
                "server.json is invalid; repair it or unlink and set up again",
            )?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).context("cannot read server.json"),
        }
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        write_private(&paths.server(), &serde_json::to_string_pretty(self)?)
    }
}

/// Write a file only its owner can read, through a temporary file renamed into place.
pub fn write_private(path: &Path, contents: &str) -> Result<()> {
    let dir = path.parent().context("a private file needs a directory")?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
        .create(dir)
        .with_context(|| format!("cannot create {}", dir.display()))?;
    let temporary = path.with_extension("tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options
        .open(&temporary)
        .with_context(|| format!("cannot write {}", temporary.display()))?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    fs::rename(&temporary, path).with_context(|| format!("cannot replace {}", path.display()))
}

/// Read a secret file written by [`write_private`]; absent is `None`.
pub fn read_private(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value.trim().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// Encode variables for systemd's `EnvironmentFile=`: every value double-quoted and escaped.
pub fn encode_env(vars: &BTreeMap<&str, String>) -> String {
    let mut out = String::new();
    for (key, value) in vars {
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
        let _ = writeln!(out, "{key}=\"{escaped}\"");
    }
    out
}

/// Read back an env file written by [`encode_env`].
pub fn decode_env(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, raw)| {
            let inner = raw
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(raw);
            (
                key.to_string(),
                inner.replace("\\\"", "\"").replace("\\\\", "\\"),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_values_survive_quotes_and_backslashes() {
        let mut vars = BTreeMap::new();
        vars.insert("BRIDGE_SELECTED_MODEL", r#"pub/model "q4" \ x"#.to_string());
        vars.insert("CONNECTOR_MANAGED_AUTH", "true".to_string());
        let text = encode_env(&vars);
        let back = decode_env(&text);
        assert_eq!(back["BRIDGE_SELECTED_MODEL"], r#"pub/model "q4" \ x"#);
        assert_eq!(back["CONNECTOR_MANAGED_AUTH"], "true");
    }

    #[cfg(unix)]
    #[test]
    fn a_private_file_is_owner_only() -> Result<()> {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = std::env::temp_dir().join(format!(
            "nexa-store-{}",
            nexa_link::secret::random_hex::<8>()?
        ));
        let path = dir.join("connector.env");
        write_private(&path, "CONNECTOR_TUNNEL_TOKEN=\"x\"\n")?;
        assert_eq!(fs::metadata(&dir)?.permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            read_private(&path)?.as_deref(),
            Some("CONNECTOR_TUNNEL_TOKEN=\"x\"")
        );
        fs::remove_dir_all(dir)?;
        Ok(())
    }
}
