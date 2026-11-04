//! Wiki: docs/wiki/robot.md

use super::{Acknowledged, Device, Error, Pairing, Session, Source, random_id, store};
use serde_json::Value;
use std::path::{Path, PathBuf};

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

/// Read the approved device of a state file, for its id and workspace.
/// # Errors
/// Reports unreadable state or a refused credential.
pub async fn device(path: &Path) -> Result<Device, Error> {
    let _guard = store::lock(path)?;
    let state: Session = store::read(path)?;
    state.device().await
}

/// A declared device's state for a command door, its approval checked; no lock, the state is written once at pairing.
async fn declared(path: &Path, expected_workspace: &str) -> Result<Session, Error> {
    let state: Session = store::read(path)?;
    if !matches!(state.source, Source::Declared { .. }) {
        return Err(Error::Contract(
            "Commands serve a declared device; this state file belongs to another adapter",
        ));
    }
    state.approved(expected_workspace).await?;
    Ok(state)
}

/// Wait up to `wait` seconds for the device's approved commands and the stops of its accepted ones.
/// # Errors
/// Reports unreadable state, a refused credential, transport failures and the workspace's refusals.
pub async fn commands(path: &Path, expected_workspace: &str, wait: u64) -> Result<Value, Error> {
    declared(path, expected_workspace)
        .await?
        .poll_commands(wait)
        .await
}

/// Read one of the device's commands.
/// # Errors
/// Reports unreadable state, a refused credential, transport failures and the workspace's refusals.
pub async fn command(
    path: &Path,
    expected_workspace: &str,
    command_id: &str,
) -> Result<Value, Error> {
    declared(path, expected_workspace)
        .await?
        .read_command(command_id)
        .await
}

/// Send the executor's acknowledgement of a command.
/// # Errors
/// Reports unreadable state, a refused credential, transport failures and the workspace's refusals.
pub async fn acknowledge(
    path: &Path,
    expected_workspace: &str,
    command_id: &str,
    body: &Value,
) -> Result<Value, Error> {
    declared(path, expected_workspace)
        .await?
        .acknowledge_command(command_id, body)
        .await
}

/// Send the executor's outcome of a command.
/// # Errors
/// Reports unreadable state, a refused credential, transport failures and the workspace's refusals.
pub async fn report(
    path: &Path,
    expected_workspace: &str,
    command_id: &str,
    body: &Value,
) -> Result<Value, Error> {
    declared(path, expected_workspace)
        .await?
        .report_command(command_id, body)
        .await
}

/// What one round of outbox delivery did.
#[derive(Debug, Default)]
pub struct Delivery {
    /// Events the workspace stored, or already held, and that left the outbox.
    pub delivered: usize,
    /// Events the workspace judged unacceptable, moved to the outbox's `rejected` directory.
    pub rejected: Vec<String>,
    /// Events still queued.
    pub remaining: usize,
    /// When the workspace asked to wait, the seconds it named.
    pub retry_after: Option<u64>,
}

/// Beat, then deliver a declared device's queued events, oldest name first, each removed once stored.
/// # Errors
/// Stops the round at the first transport or contract failure; the event stays queued.
pub async fn deliver(
    path: &Path,
    expected_workspace: &str,
    outbox: &Path,
) -> Result<Delivery, Error> {
    let _guard = store::lock(path)?;
    let state: Session = store::read(path)?;
    if !matches!(state.source, Source::Declared { .. }) {
        return Err(Error::Contract(
            "deliver serves a declared device; use observe for this adapter",
        ));
    }
    let device = state.present(expected_workspace).await?;
    let queued = queued(outbox)?;
    let mut delivery = Delivery {
        remaining: queued.len(),
        ..Delivery::default()
    };
    for file in queued {
        let event: Value = serde_json::from_slice(&std::fs::read(&file)?)?;
        match state.deliver_as(&device, &event).await {
            Ok(_) => {
                std::fs::remove_file(&file)?;
                delivery.delivered += 1;
            }
            Err(Error::Rejected(_)) => {
                let aside = outbox.join("rejected");
                std::fs::create_dir_all(&aside)?;
                let name = file
                    .file_name()
                    .ok_or(Error::Contract("Queued event has no name"))?;
                std::fs::rename(&file, aside.join(name))?;
                delivery.rejected.push(name.to_string_lossy().into_owned());
            }
            Err(Error::RateLimited(seconds)) => {
                delivery.retry_after = Some(seconds);
                return Ok(delivery);
            }
            Err(other) => return Err(other),
        }
        delivery.remaining -= 1;
    }
    Ok(delivery)
}

/// The outbox's queued events: regular `.json` files, never hidden staging files or subdirectories.
fn queued(outbox: &Path) -> Result<Vec<PathBuf>, Error> {
    let metadata = std::fs::symlink_metadata(outbox)?;
    if !metadata.is_dir() {
        return Err(Error::Contract(
            "The outbox must be a real directory, not a symlink",
        ));
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(outbox)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| !name.starts_with('.'))
        })
        .collect();
    files.sort();
    Ok(files)
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

    #[test]
    fn the_outbox_queues_only_its_own_event_files_in_name_order() -> Result<(), Error> {
        let dir = std::env::temp_dir().join(format!("nexa-outbox-{}", random_id()?));
        std::fs::create_dir_all(dir.join("rejected"))?;
        for name in ["b-2.json", "a-1.json", ".a-0.json", "notes.txt"] {
            std::fs::write(dir.join(name), "{}")?;
        }
        let names: Vec<String> = queued(&dir)?
            .iter()
            .filter_map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .collect();
        assert_eq!(names, ["a-1.json", "b-2.json"]);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
