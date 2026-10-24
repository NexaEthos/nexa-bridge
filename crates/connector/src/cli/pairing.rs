//! Wiki: docs/wiki/server-setup.md

use std::time::Duration;

use anyhow::{Context, Result};
use nexa_link::agora::{self, DeviceStatus, PairingEnded};
use nexa_link::secret;
use serde::{Deserialize, Serialize};

use super::qr;
use super::store::{self, Paths};
use super::term::Term;

const POLL: Duration = Duration::from_secs(5);

#[derive(Deserialize, Serialize)]
struct Pending {
    agora_url: String,
    approval_url: String,
    proof: String,
    verification_uri: Option<String>,
}

impl Pending {
    fn save(&self, paths: &Paths) -> Result<()> {
        store::write_private(&paths.pairing(), &serde_json::to_string(self)?)
    }

    fn load(paths: &Paths) -> Result<Option<Self>> {
        store::read_private(&paths.pairing())?
            .map(|json| serde_json::from_str(&json).context("cannot read the unfinished pairing"))
            .transpose()
    }

    fn check_origins(&self, origin: &str, approval: &str) -> Result<()> {
        anyhow::ensure!(
            agora::origin(&self.agora_url)? == origin
                && agora::origin(&self.approval_url)? == approval,
            "An unfinished pairing belongs to another Agora address. Use its original --agora and --approval; \
             to change workspaces, revoke that device in Agora and run `nexa-bridge unlink` first."
        );
        Ok(())
    }
}

pub fn unfinished(paths: &Paths) -> bool {
    paths.pairing().exists()
}

pub fn clear(paths: &Paths) -> Result<()> {
    for path in [paths.pending(), paths.pairing()] {
        match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error).context("cannot remove the completed pairing state");
            }
            _ => {}
        }
    }
    Ok(())
}

async fn status(paths: &Paths, origin: &str, proof: &str) -> Result<DeviceStatus> {
    accept_status(paths, agora::device_status(origin, proof).await)
}

fn accept_status(paths: &Paths, result: Result<DeviceStatus>) -> Result<DeviceStatus> {
    match result {
        Err(error) if error.downcast_ref::<PairingEnded>().is_some() => {
            clear(paths)?;
            Err(error).context(
                "This pairing can no longer be used. Run `nexa-bridge unlink`, then `nexa-bridge setup` to link again",
            )
        }
        result => result,
    }
}

fn require_explicit_origin(explicit: bool) -> Result<()> {
    anyhow::ensure!(
        explicit,
        "An unfinished setup saved a proof without its Agora address. Run setup with --agora <original HTTPS origin>. \
         Use the address where you approved this server, never a different destination."
    );
    Ok(())
}

fn accept_unbound_status(result: Result<DeviceStatus>) -> Result<DeviceStatus> {
    match result {
        Err(error) if error.downcast_ref::<PairingEnded>().is_some() => Err(error).context(
            "This Agora does not recognize the saved proof; it has been retained. \
             Use --agora with the address where you approved this server. \
             If that address is correct, run `nexa-bridge unlink`, then setup to link again",
        ),
        result => result,
    }
}

async fn recover_unbound(
    paths: &Paths,
    origin: &str,
    approval: &str,
    explicit: bool,
    proof: String,
) -> Result<(String, DeviceStatus)> {
    require_explicit_origin(explicit)?;
    let device = accept_unbound_status(agora::device_status(origin, &proof).await)?;
    anyhow::ensure!(
        device.status == "active",
        "The previous pairing is not approved. Complete its original browser approval and run setup again; \
         if you lost that page, wait for the code to expire before retrying."
    );
    Pending {
        agora_url: origin.to_string(),
        approval_url: approval.to_string(),
        proof: proof.clone(),
        verification_uri: None,
    }
    .save(paths)?;
    std::fs::remove_file(paths.pending()).context("cannot remove the recovered unbound proof")?;
    Ok((proof, device))
}

fn show_approval(term: &Term, pending: &Pending) -> Result<()> {
    term.blank()?;
    if let Some(uri) = &pending.verification_uri {
        term.line(
            "A workspace administrator opens this link from any device and approves this server:",
        )?;
        term.line(&format!("  {uri}"))?;
        term.line(&qr::render(uri)?)?;
        let url = reqwest::Url::parse(uri).context("the saved approval link is invalid")?;
        let code = url
            .fragment()
            .context("the saved approval link has no code")?;
        term.line(&format!("  code {code}"))?;
        term.line("The approval code is valid for ten minutes from its creation.")?;
    } else {
        term.note("An earlier pairing is still pending. Complete its original browser approval; \
                   if the link was not received, wait for that request to expire, then run setup again.")?;
    }
    term.progress("Waiting for approval")
}

async fn wait(
    term: &Term,
    paths: &Paths,
    pending: &Pending,
    mut device: DeviceStatus,
) -> Result<DeviceStatus> {
    if device.status == "pending" {
        show_approval(term, pending)?;
    }
    loop {
        match device.status.as_str() {
            "active" => {
                term.done("approved; continuing with the same device")?;
                return Ok(device);
            }
            "pending" => {}
            _ => anyhow::bail!(
                "This device is not active. Check its state in Agora; \
                                revoke it there before using `nexa-bridge unlink` to start again."
            ),
        }
        tokio::time::sleep(POLL).await;
        match status(paths, &pending.agora_url, &pending.proof).await {
            Ok(next) => {
                device = next;
                term.tick()?;
            }
            Err(error) if error.downcast_ref::<PairingEnded>().is_some() => return Err(error),
            Err(error) => term.note(&format!(
                "Agora did not answer ({error}); the pairing is retained"
            ))?,
        }
    }
}

pub async fn link(
    term: &Term,
    paths: &Paths,
    origin: &str,
    approval: &str,
    label: &str,
    explicit_origin: bool,
) -> Result<(String, DeviceStatus)> {
    let pending = if let Some(pending) = Pending::load(paths)? {
        pending.check_origins(origin, approval)?;
        pending
    } else if let Some(proof) = store::read_private(&paths.pending())? {
        return recover_unbound(paths, origin, approval, explicit_origin, proof).await;
    } else {
        let mut pending = Pending {
            agora_url: origin.to_string(),
            approval_url: approval.to_string(),
            proof: secret::generate()?,
            verification_uri: None,
        };
        pending.save(paths)?;
        let started = agora::start_pairing(origin, approval, label, &pending.proof).await?;
        pending.verification_uri = Some(started.verification_uri);
        pending.save(paths)?;
        pending
    };
    let device = status(paths, origin, &pending.proof).await?;
    let device = wait(term, paths, &pending, device).await?;
    Ok((pending.proof, device))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unrecognized_unbound_proof_is_retained_for_the_original_origin() -> Result<()> {
        let paths = Paths {
            dir: std::env::temp_dir().join(format!(
                "nexa-unbound-refused-{}",
                secret::random_hex::<8>()?
            )),
        };
        store::write_private(&paths.pending(), "synthetic-proof")?;
        let error = accept_unbound_status(Err(PairingEnded("refused".into()).into()))
            .err()
            .context("the refusal must remain visible")?;
        assert!(error.downcast_ref::<PairingEnded>().is_some());
        assert!(error.to_string().contains("it has been retained"));
        assert!(error.to_string().contains("where you approved"));
        assert_eq!(
            store::read_private(&paths.pending())?.as_deref(),
            Some("synthetic-proof")
        );
        assert!(!unfinished(&paths));
        clear(&paths)?;
        std::fs::remove_dir(paths.dir)?;
        Ok(())
    }

    #[test]
    fn service_errors_preserve_pairing_but_expiry_clears_only_pending_state() -> Result<()> {
        let paths = Paths {
            dir: std::env::temp_dir()
                .join(format!("nexa-pairing-error-{}", secret::random_hex::<8>()?)),
        };
        let pending = Pending {
            agora_url: "https://agora.example.invalid".into(),
            approval_url: "https://agora.example.invalid".into(),
            proof: "synthetic-proof".into(),
            verification_uri: None,
        };
        pending.save(&paths)?;
        store::write_private(&paths.server(), "configured state")?;
        assert!(accept_status(&paths, Err(anyhow::anyhow!("network unavailable"))).is_err());
        assert_eq!(
            Pending::load(&paths)?
                .context("network error retains state")?
                .proof,
            pending.proof
        );
        assert!(accept_status(&paths, Err(PairingEnded("expired".into()).into())).is_err());
        assert!(!unfinished(&paths));
        assert_eq!(
            store::read_private(&paths.server())?.as_deref(),
            Some("configured state")
        );
        std::fs::remove_dir_all(paths.dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn another_origin_never_receives_a_saved_proof() -> Result<()> {
        let paths = Paths {
            dir: std::env::temp_dir().join(format!(
                "nexa-pairing-origin-{}",
                secret::random_hex::<8>()?
            )),
        };
        let pending = Pending {
            agora_url: "https://agora.example.invalid".into(),
            approval_url: "https://agora.example.invalid".into(),
            proof: "synthetic-proof".into(),
            verification_uri: None,
        };
        pending.save(&paths)?;
        let error = link(
            &Term::new(),
            &paths,
            "https://another.example.invalid",
            "https://another.example.invalid",
            "test",
            true,
        )
        .await
        .err()
        .context("an origin change must fail")?;
        assert!(
            error
                .to_string()
                .contains("belongs to another Agora address")
        );
        assert_eq!(
            Pending::load(&paths)?.context("state retained")?.proof,
            pending.proof
        );
        clear(&paths)?;
        std::fs::remove_dir(paths.dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn an_unbound_proof_is_not_sent_without_an_explicit_origin() -> Result<()> {
        let paths = Paths {
            dir: std::env::temp_dir().join(format!("nexa-unbound-{}", secret::random_hex::<8>()?)),
        };
        store::write_private(&paths.pending(), "synthetic-proof")?;
        let result = link(
            &Term::new(),
            &paths,
            "https://agora.example.invalid",
            "https://agora.example.invalid",
            "test",
            false,
        )
        .await;
        let error = result.err().context("the origin must be explicit")?;
        assert!(
            error
                .to_string()
                .contains("--agora <original HTTPS origin>")
        );
        assert_eq!(
            store::read_private(&paths.pending())?.as_deref(),
            Some("synthetic-proof")
        );
        assert!(!unfinished(&paths));
        clear(&paths)?;
        std::fs::remove_dir(paths.dir)?;
        Ok(())
    }

    #[tokio::test]
    async fn an_approved_device_resumes_without_an_approval_code_or_deadline() -> Result<()> {
        let paths = Paths {
            dir: std::env::temp_dir().join(format!("nexa-approved-{}", secret::random_hex::<8>()?)),
        };
        let pending = Pending {
            agora_url: "https://agora.example.invalid".into(),
            approval_url: "https://agora.example.invalid".into(),
            proof: "synthetic-proof".into(),
            verification_uri: None,
        };
        pending.save(&paths)?;
        let device = DeviceStatus {
            id: "approved-device".into(),
            status: "active".into(),
            tenant_slug: Some("test".into()),
            connection_slug: Some("test-bridge".into()),
            relay_url: Some("wss://relay.example.invalid".into()),
        };
        let resumed = wait(&Term::new(), &paths, &pending, device).await?;
        assert_eq!(resumed.id, "approved-device");
        assert_eq!(
            Pending::load(&paths)?
                .context("retained until setup completes")?
                .proof,
            pending.proof
        );
        clear(&paths)?;
        std::fs::remove_dir(paths.dir)?;
        Ok(())
    }

    #[test]
    fn a_proof_cannot_move_between_origins() -> Result<()> {
        let pending = Pending {
            agora_url: "https://agora.example.com".into(),
            approval_url: "https://approve.example.com".into(),
            proof: "private-proof".into(),
            verification_uri: None,
        };
        pending.check_origins("https://agora.example.com", "https://approve.example.com")?;
        assert!(
            pending
                .check_origins(
                    "https://elsewhere.example.com",
                    "https://approve.example.com"
                )
                .is_err()
        );
        assert!(
            pending
                .check_origins("https://agora.example.com", "https://elsewhere.example.com")
                .is_err()
        );
        assert!(require_explicit_origin(false).is_err());
        require_explicit_origin(true)?;
        Ok(())
    }

    #[test]
    fn interrupted_pairing_keeps_its_proof_and_approval_page() -> Result<()> {
        let paths = Paths {
            dir: std::env::temp_dir().join(format!("nexa-pairing-{}", secret::random_hex::<8>()?)),
        };
        let mut pending = Pending {
            agora_url: "https://agora.example.com".into(),
            approval_url: "https://agora.example.com".into(),
            proof: secret::generate()?,
            verification_uri: None,
        };
        pending.save(&paths)?;
        assert_eq!(
            Pending::load(&paths)?
                .context("saved before enrollment")?
                .proof,
            pending.proof
        );
        pending.verification_uri = Some("https://agora.example.com/bridge-pair#TEST".into());
        pending.save(&paths)?;
        let restored = Pending::load(&paths)?.context("saved after enrollment")?;
        assert_eq!(restored.proof, pending.proof);
        assert_eq!(restored.verification_uri, pending.verification_uri);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(paths.pairing())?.permissions().mode() & 0o777,
                0o600
            );
        }
        clear(&paths)?;
        clear(&paths)?;
        assert!(!unfinished(&paths));
        std::fs::remove_dir(paths.dir)?;
        Ok(())
    }
}
