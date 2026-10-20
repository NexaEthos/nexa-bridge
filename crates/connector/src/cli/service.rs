//! The bridge and the connector as systemd user services that survive logout and reboot. Wiki: docs/wiki/server-setup.md

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use super::store::{Paths, write_private};

pub const BRIDGE_UNIT: &str = "nexa-bridge.service";
pub const CONNECTOR_UNIT: &str = "nexa-bridge-connector.service";

fn unit_dir() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".config"),
    };
    Ok(base.join("systemd").join("user"))
}

/// The two unit files: the bridge first, the connector after it and stopped with it.
pub fn units(paths: &Paths, bin_dir: &Path) -> (String, String) {
    let unit = |description: &str, env: &Path, exec: &Path, after: &str| {
        let mut text = String::new();
        let _ = writeln!(
            text,
            "[Unit]\nDescription={description}\nAfter=network-online.target{after}"
        );
        let _ = writeln!(text, "Wants=network-online.target");
        if !after.is_empty() {
            let _ = writeln!(text, "Requires={BRIDGE_UNIT}\nPartOf={BRIDGE_UNIT}");
        }
        let _ = writeln!(
            text,
            "\n[Service]\nType=simple\nEnvironmentFile={}\nWorkingDirectory={}",
            env.display(),
            paths.dir.display()
        );
        let _ = writeln!(
            text,
            "ExecStart={}\nRestart=always\nRestartSec=3\nNoNewPrivileges=true",
            exec.display()
        );
        let _ = writeln!(text, "\n[Install]\nWantedBy=default.target");
        text
    };
    (
        unit(
            "Nexa Bridge: the local gate in front of your model server",
            &paths.bridge_env(),
            &bin_dir.join("nexa-llm-bridge"),
            "",
        ),
        unit(
            "Nexa Bridge connector: the outbound tunnel to your workspace",
            &paths.connector_env(),
            &bin_dir.join("nexa-llm-connector"),
            &format!(" {BRIDGE_UNIT}"),
        ),
    )
}

fn systemctl(args: &[&str]) -> Result<()> {
    let status = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .status()
        .context("systemctl is not available: this setup runs the services with systemd")?;
    anyhow::ensure!(
        status.success(),
        "systemctl --user {} failed",
        args.join(" ")
    );
    Ok(())
}

/// Write the units, enable them at boot and start them now.
pub fn install(paths: &Paths, bin_dir: &Path) -> Result<()> {
    anyhow::ensure!(
        cfg!(target_os = "linux"),
        "Services are installed on Linux with systemd; run with --no-service and start the two programs yourself"
    );
    let dir = unit_dir()?;
    let (bridge, connector) = units(paths, bin_dir);
    write_private(&dir.join(BRIDGE_UNIT), &bridge)?;
    write_private(&dir.join(CONNECTOR_UNIT), &connector)?;
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", BRIDGE_UNIT, CONNECTOR_UNIT])
}

/// Whether the services keep running after logout: systemd's linger for this user.
pub fn lingers() -> bool {
    let user = std::env::var("USER").unwrap_or_default();
    Command::new("loginctl")
        .args(["show-user", &user, "--property=Linger"])
        .output()
        .is_ok_and(|out| String::from_utf8_lossy(&out.stdout).trim() == "Linger=yes")
}

/// Ask systemd to keep this user's services running after logout; may need an administrator.
pub fn enable_linger() -> Result<()> {
    let user = std::env::var("USER").context("USER is not set")?;
    let status = Command::new("loginctl")
        .args(["enable-linger", &user])
        .status()
        .context("loginctl is not available")?;
    anyhow::ensure!(
        status.success(),
        "loginctl enable-linger {user} was refused"
    );
    Ok(())
}

pub fn restart() -> Result<()> {
    systemctl(&["restart", BRIDGE_UNIT, CONNECTOR_UNIT])
}

/// Whether this server runs the two programs as user services: both unit files are installed.
pub fn installed() -> bool {
    cfg!(target_os = "linux")
        && unit_dir()
            .is_ok_and(|dir| dir.join(BRIDGE_UNIT).exists() && dir.join(CONNECTOR_UNIT).exists())
}

/// `active`, `inactive`, `failed`… as systemd reports each unit.
pub fn state(unit: &str) -> String {
    Command::new("systemctl")
        .args(["--user", "is-active", unit])
        .output()
        .map_or_else(
            |_| "unknown".to_string(),
            |out| String::from_utf8_lossy(&out.stdout).trim().to_string(),
        )
}

/// Stop and disable the services and remove their unit files.
pub fn remove() -> Result<()> {
    if !cfg!(target_os = "linux") {
        return Ok(());
    }
    let dir = unit_dir()?;
    if dir.join(BRIDGE_UNIT).exists() || dir.join(CONNECTOR_UNIT).exists() {
        systemctl(&["disable", "--now", CONNECTOR_UNIT, BRIDGE_UNIT])?;
        for unit in [CONNECTOR_UNIT, BRIDGE_UNIT] {
            std::fs::remove_file(dir.join(unit))
                .with_context(|| format!("cannot remove {unit}"))?;
        }
        systemctl(&["daemon-reload"])?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn the_connector_unit_follows_the_bridge_and_reads_its_own_env() {
        let paths = Paths {
            dir: PathBuf::from("/home/u/.config/nexa-bridge"),
        };
        let (bridge, connector) = units(&paths, Path::new("/home/u/.local/bin"));
        assert!(bridge.contains("EnvironmentFile=/home/u/.config/nexa-bridge/bridge.env"));
        assert!(bridge.contains("ExecStart=/home/u/.local/bin/nexa-llm-bridge"));
        assert!(
            bridge.contains("WorkingDirectory=/home/u/.config/nexa-bridge"),
            "the pool's config.toml is read there"
        );
        assert!(!bridge.contains("Requires="));
        assert!(connector.contains("EnvironmentFile=/home/u/.config/nexa-bridge/connector.env"));
        assert!(connector.contains("Requires=nexa-bridge.service"));
        assert!(connector.contains("After=network-online.target nexa-bridge.service"));
        assert!(connector.contains("WantedBy=default.target"));
    }
}
