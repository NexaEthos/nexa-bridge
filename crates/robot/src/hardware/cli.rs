//! Wiki: docs/wiki/robot.md

use super::{Acknowledged, Error, Pairing, Session, Source, random_id, store};
use serde_json::Value;
use std::path::Path;

/// Pair a CLI source using the existing private Unix state file.
/// # Errors
/// Rejects an existing state path, unsafe storage or failed approval request.
pub async fn pair(
    path: &Path,
    issuer: &str,
    approval: &str,
    source: Source,
    label: &str,
) -> Result<Pairing, Error> {
    let _guard = store::lock(path)?;
    if path.try_exists()? {
        return Err(Error::Contract(
            "State already exists; use observe or choose a new state file",
        ));
    }
    let state = Session::new(issuer, source, label.into(), random_id()?)?;
    let (approval, body) = state.prepare_pair(approval).await?;
    store::write_new(path, &state)?;
    state.pair_prepared(&approval, &body).await
}

/// Send or retry one durably persisted CLI observation.
/// # Errors
/// Leaves the pending document intact unless its exact receipt is acknowledged.
pub async fn observe(path: &Path, expected_workspace: &str) -> Result<Acknowledged, Error> {
    let _guard = store::lock(path)?;
    let state: Session = store::read(path)?;
    let pending = store::sibling(path, ".pending.json");
    let event: Value = if pending.try_exists()? {
        store::read(&pending)?
    } else {
        let event = state.collect(expected_workspace).await?;
        store::write_new(&pending, &event)?;
        event
    };
    let receipt = state.deliver(expected_workspace, &event).await?;
    store::remove(&pending)?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn local_pairing_failures_leave_the_same_state_path_reusable() -> Result<(), Error> {
        let dir = std::env::temp_dir().join(format!("nexa-pair-preflight-{}", random_id()?));
        std::fs::create_dir(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        let path = dir.join("device.json");
        let invalid_origin = pair(
            &path,
            "https://api.example.com",
            "http://agora.example.com",
            Source::UnoQ {
                serial: String::new(),
            },
            "board",
        )
        .await;
        assert!(matches!(invalid_origin, Err(Error::Contract(_))));
        assert!(!path.try_exists()?);
        let unavailable_source = pair(
            &path,
            "https://api.example.com",
            "https://agora.example.com",
            Source::UnoQ {
                serial: String::new(),
            },
            "board",
        )
        .await;
        assert!(matches!(unavailable_source, Err(Error::Board(_))));
        assert!(!path.try_exists()?);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
