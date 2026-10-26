//! Agora's managed doors for a computer: enrollment, status, model publication, guides. Wiki: docs/wiki/managed-auth.md

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use reqwest::{Client, Response, Url};
use serde::Deserialize;

use crate::guides::WorkspacePreset;
use crate::runtimes::Flavor;
use crate::secret;

const API: &str = "/api/v1/bridge/managed";

/// The connector's exit status once Agora no longer accepts its device: a supervisor stops, never restarts it.
pub const ENDED_EXIT_STATUS: u8 = 77;

/// Agora's final word on a pairing: the device is gone, refused or expired — polling must stop.
#[derive(Debug)]
pub struct PairingEnded(pub String);

impl std::fmt::Display for PairingEnded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PairingEnded {}

#[derive(Deserialize)]
struct Started {
    code: String,
    verification_uri: String,
    expires_in: u64,
}

/// A pairing started at Agora: the human code, the page that approves it, when it expires (Unix seconds).
#[derive(Clone, Debug)]
pub struct Pairing {
    /// The code a workspace administrator approves.
    pub code: String,
    /// The approval page, carrying the code in its fragment.
    pub verification_uri: String,
    /// When the code expires, in Unix seconds.
    pub expires_at: u64,
}

/// What Agora knows of this computer.
#[derive(Debug, Deserialize)]
pub struct DeviceStatus {
    /// The device's id.
    pub id: String,
    /// `pending`, `active` or `revoked`.
    pub status: String,
    /// The approving workspace, once approved.
    pub tenant_slug: Option<String>,
    /// The connection the device joined, once approved.
    pub connection_slug: Option<String>,
    /// The relay the connector dials, once approved.
    pub relay_url: Option<String>,
}

#[doc = "Normalize an Agora address into an HTTPS origin without path, credentials or query.\n# Errors\nAn address that is not such an origin."]
pub fn origin(value: &str) -> Result<String> {
    let url = Url::parse(value.trim()).context("Enter your Agora HTTPS address")?;
    anyhow::ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "Agora address must be an HTTPS origin without a path, credentials or query"
    );
    Ok(url.as_str().trim_end_matches('/').into())
}

fn client() -> Result<Client> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .https_only(true)
        .build()
        .context("Cannot initialize the secure enrollment client")
}

fn checked(response: Response) -> Result<Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let code = response.status().as_u16();
    let message = match code {
        401 | 403 => {
            "Computer is not authorized. Approve it in Agora or link it again after revocation."
        }
        404 => {
            "Managed linking is unavailable on this Agora deployment. Update the server before linking."
        }
        409 => "Linking state changed. Check the workspace in Agora and start linking again.",
        410 => "Approval expired. Start linking again.",
        429 => "Too many linking requests. Wait one minute before retrying.",
        503 => "Managed linking is not enabled or configured on this deployment yet.",
        _ => "Agora could not complete linking. Retry when the service is available.",
    };
    if matches!(code, 401 | 403 | 410) {
        return Err(PairingEnded(message.into()).into());
    }
    anyhow::bail!("{message}")
}

/// The approval page must belong to the Agora the person named — never another host.
fn same_origin(uri: &Url, origin: &str) -> bool {
    Url::parse(origin).is_ok_and(|agora| agora.origin() == uri.origin())
}

/// Two relay addresses are the same relay when they parse to the same URL, not the same bytes.
#[must_use]
pub fn same_relay(server: Option<&str>, stored: &str) -> bool {
    match (server.map(Url::parse), Url::parse(stored)) {
        (Some(Ok(a)), Ok(b)) => a == b,
        _ => false,
    }
}

#[doc = "Start enrollment with the proof's digest; the approval page must sit on `approval_origin`.\n# Errors\nAgora's refusal, or an approval address that is unsafe or not on `approval_origin`."]
pub async fn start_pairing(
    origin: &str,
    approval_origin: &str,
    label: &str,
    proof: &str,
) -> Result<Pairing> {
    anyhow::ensure!(
        !label.trim().is_empty()
            && label.chars().count() <= 80
            && !label.chars().any(char::is_control),
        "Give this computer a short recognizable name"
    );
    let started: Started = checked(
        client()?
            .post(format!("{origin}{API}/pairing"))
            .json(&serde_json::json!({ "token_sha256": secret::digest(proof), "label": label }))
            .send()
            .await
            .context("Cannot reach Agora to link this computer")?,
    )?
    .json()
    .await
    .context("Agora returned an invalid linking response")?;
    let uri = Url::parse(&started.verification_uri)
        .context("Agora returned an invalid approval address")?;
    anyhow::ensure!(
        uri.scheme() == "https"
            && uri.host_str().is_some()
            && uri.username().is_empty()
            && uri.password().is_none()
            && uri.query().is_none()
            && uri.path() == "/bridge-pair"
            && uri.fragment() == Some(started.code.as_str())
            && same_origin(&uri, approval_origin)
            && started.expires_in > 0
            && started.expires_in <= 600,
        "Agora returned an unsafe or incompatible approval address"
    );
    Ok(Pairing {
        code: started.code,
        verification_uri: uri.to_string(),
        expires_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() + started.expires_in,
    })
}

#[doc = "Ask Agora what it knows of the device holding `proof`.\n# Errors\nAgora unreachable or refusing; [`PairingEnded`] when the device is gone."]
pub async fn device_status(origin: &str, proof: &str) -> Result<DeviceStatus> {
    checked(
        client()?
            .get(format!("{origin}{API}/device"))
            .bearer_auth(proof)
            .send()
            .await
            .context("Cannot verify device approval with Agora")?,
    )?
    .json()
    .await
    .context("Agora returned an invalid device status")
}

#[doc = "Record the computer's choice at Agora: the exact served id, its runtime and the loaded window when known.\n# Errors\nAgora unreachable or refusing the selection."]
pub async fn publish_model(
    origin: &str,
    proof: &str,
    model: &str,
    runtime: Flavor,
    context_window: Option<u32>,
) -> Result<()> {
    checked(
        client()?
            .put(format!("{origin}{API}/device/model"))
            .bearer_auth(proof)
            .json(&serde_json::json!({
                "model": model, "profile": null, "runtime": runtime, "context_window": context_window
            }))
            .send()
            .await
            .context("Cannot publish the selected model to your workspace")?,
    )
    .context("Your workspace refused the selected model; check the loaded model before reconnecting")?;
    Ok(())
}

async fn presets(origin: &str, proof: &str, door: &str) -> Result<Vec<WorkspacePreset>> {
    let presets: Vec<WorkspacePreset> = checked(
        client()?
            .get(format!("{origin}{API}/device/{door}"))
            .bearer_auth(proof)
            .send()
            .await
            .context("Cannot reach your workspace's models; retry in a moment")?,
    )?
    .json()
    .await
    .context("Agora returned an incompatible list of models")?;
    for preset in &presets {
        preset.validate()?;
    }
    Ok(presets)
}

#[doc = "The presets the workspace runs on this computer's connection, each with Nexa's guide.\n# Errors\nAgora unreachable or refusing, or a preset that fails validation."]
pub async fn guides(origin: &str, proof: &str) -> Result<Vec<WorkspacePreset>> {
    presets(origin, proof, "guides").await
}

#[doc = "Nexa's presets that carry a local guide: what this computer may be set up to serve.\n# Errors\nAgora unreachable or refusing, or a preset that fails validation."]
pub async fn catalog(origin: &str, proof: &str) -> Result<Vec<WorkspacePreset>> {
    presets(origin, proof, "catalog").await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_approval_page_must_sit_on_the_named_agora() -> Result<()> {
        let page = Url::parse("https://agora.example.com/bridge-pair#code")?;
        assert!(same_origin(&page, "https://agora.example.com"));
        assert!(!same_origin(&page, "https://agora.example.org"));
        assert!(!same_origin(&page, "https://agora.example.com:8443"));
        Ok(())
    }

    #[test]
    fn relays_compare_as_urls_not_bytes() {
        assert!(same_relay(
            Some("wss://relay.example.com"),
            "wss://relay.example.com/"
        ));
        assert!(same_relay(
            Some("wss://RELAY.example.com/_tunnel"),
            "wss://relay.example.com/_tunnel"
        ));
        assert!(!same_relay(
            Some("wss://relay.example.com/_tunnel"),
            "wss://relay.example.com/"
        ));
        assert!(!same_relay(None, "wss://relay.example.com/"));
        assert!(!same_relay(Some("not a url"), "wss://relay.example.com/"));
    }

    #[test]
    fn an_agora_address_is_a_bare_https_origin() -> Result<()> {
        assert_eq!(
            origin(" https://agora.example.com/ ")?,
            "https://agora.example.com"
        );
        for bad in [
            "http://agora.example.com",
            "https://agora.example.com/app",
            "https://user@agora.example.com",
            "https://agora.example.com/?x=1",
        ] {
            assert!(origin(bad).is_err(), "{bad}");
        }
        Ok(())
    }
}
