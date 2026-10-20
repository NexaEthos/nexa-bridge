//! Wiki: docs/wiki/desktop.md#diagnostics

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use nexa_tunnel_protocol::BuildInfo;

use crate::settings::Settings;
use crate::supervisor::Status;

/// This binary's build block, with the Agora origin it was built for.
pub fn build() -> BuildInfo {
    let mut block = nexa_tunnel_protocol::build_info!();
    block.agora_origin = option_env!("NEXA_AGORA_URL").map(str::to_string);
    block
}

/// The diagnostics text: the build block, the platform, the connection state, the non-secret settings.
pub fn render(settings: &Settings, status: &Status, now: u64) -> String {
    let build = build();
    let mut text = String::new();
    let _ = writeln!(
        text,
        "Nexa Bridge diagnostics — no credential, token or pairing code is written here"
    );
    let _ = writeln!(text, "generated_at: {now}");
    let _ = writeln!(
        text,
        "build: version={} commit={} target={} profile={} channel={} agora_origin={}",
        build.version,
        build.commit,
        build.target,
        build.profile,
        build.channel,
        build.agora_origin.as_deref().unwrap_or("none")
    );
    let _ = writeln!(
        text,
        "platform: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let _ = writeln!(
        text,
        "connection: phase={} bridge_up={} connector_up={} restarts={} detail={}",
        format!("{:?}", status.phase).to_lowercase(),
        status.bridge_up,
        status.connector_up,
        status.restarts,
        status.detail.as_deref().unwrap_or("none")
    );
    let _ = writeln!(text, "agora_url: {}", settings.agora_url);
    let _ = writeln!(text, "relay_url: {}", settings.relay_url);
    let _ = writeln!(text, "upstream_url: {}", settings.upstream_url);
    let _ = writeln!(text, "listen_addr: {}", settings.listen_addr);
    let _ = writeln!(text, "selected_model: {}", settings.selected_model);
    let _ = writeln!(text, "autostart: {}", settings.autostart);
    match &settings.managed {
        Some(identity) => {
            let _ = writeln!(
                text,
                "managed: device_id={} tenant={} connection={} confirmed={}",
                identity.device_id,
                identity.tenant_slug,
                identity.connection_slug,
                identity.confirmed
            );
        }
        None => {
            let _ = writeln!(text, "managed: none");
        }
    }
    let _ = writeln!(
        text,
        "pending_pairing: {}",
        settings.pending_pairing.as_ref().map_or_else(
            || "none".to_string(),
            |p| format!("expires_at={}", p.expires_at)
        )
    );
    text
}

/// Write the text beside the settings and return its path.
pub fn export(dir: &Path, settings: &Settings, status: &Status) -> Result<PathBuf> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let path = dir.join("diagnostics.txt");
    std::fs::write(&path, render(settings, status, now))
        .with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::PendingPairing;

    #[test]
    fn the_text_carries_the_build_block_and_never_the_pairing_code() {
        let settings = Settings {
            pending_pairing: Some(PendingPairing {
                agora_url: "https://agora.example.com".into(),
                approval_url: None,
                code: "SECRET-CODE".into(),
                verification_uri: "https://agora.example.com/bridge-pair#SECRET-CODE".into(),
                expires_at: 42,
            }),
            ..Settings::default()
        };
        let text = render(&settings, &Status::default(), 7);
        assert!(text.contains("generated_at: 7"));
        assert!(text.contains("build: version="));
        assert!(
            text.contains(&format!("channel={}", build().channel)),
            "{text}"
        );
        assert!(text.contains("pending_pairing: expires_at=42"));
        assert!(!text.contains("SECRET-CODE"), "{text}");
    }
}
