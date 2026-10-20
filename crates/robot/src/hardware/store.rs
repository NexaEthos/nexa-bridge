//! Wiki: docs/wiki/robot.md

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Serialize, de::DeserializeOwned};

use super::error::Error;
pub fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

pub fn lock(path: &Path) -> Result<File, Error> {
    check_directory(path)?;
    let path = sibling(path, ".lock");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if path.try_exists()? {
        check_file(&path)?;
    }
    let file = options.open(path)?;
    file.try_lock().map_err(|_| {
        Error::Contract("Another hardware command holds this state file; wait for it to finish")
    })?;
    Ok(file)
}

pub fn read<T: DeserializeOwned>(path: &Path) -> Result<T, Error> {
    check_file(path)?;
    Ok(serde_json::from_reader(File::open(path)?)?)
}

pub fn write_new<T: Serialize>(path: &Path, value: &T) -> Result<(), Error> {
    check_directory(path)?;
    let bytes = serde_json::to_vec(value)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    sync_directory(path)
}

pub fn remove(path: &Path) -> Result<(), Error> {
    check_file(path)?;
    std::fs::remove_file(path)?;
    sync_directory(path)
}

fn sync_directory(path: &Path) -> Result<(), Error> {
    let parent = path.parent().ok_or(Error::Contract(
        "Use a state file inside a private directory",
    ))?;
    if !parent.is_dir() {
        return Err(Error::Contract(
            "Hardware state directory disappeared during persistence",
        ));
    }
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn check_directory(path: &Path) -> Result<(), Error> {
    let parent = path.parent().ok_or(Error::Contract(
        "Use a state file inside a private directory",
    ))?;
    let metadata = std::fs::symlink_metadata(parent)?;
    if !metadata.is_dir() {
        return Err(Error::Contract(
            "Hardware state must be inside a real private directory, not a symlink",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::Contract(
                "Create a private state directory with mode 0700 before pairing",
            ));
        }
    }
    Ok(())
}

fn check_file(path: &Path) -> Result<(), Error> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(Error::Contract(
            "Hardware state must be a regular file, not a symlink",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::Contract(
                "Hardware state is not private; restrict the file to mode 0600 before using it",
            ));
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::hardware::{Session as State, Source};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn pending_events_survive_retries_without_overwrite_or_parallel_writers() -> Result<(), Error> {
        let dir =
            std::env::temp_dir().join(format!("nexa-hardware-test-{}", super::super::random_id()?));
        std::fs::create_dir(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        let path = dir.join("device.json");
        let guard = lock(&path)?;
        assert!(lock(&path).is_err());
        let event = serde_json::json!({"event_id":"stable", "value": 1});
        let pending = sibling(&path, ".pending.json");
        write_new(&pending, &event)?;
        assert!(write_new(&pending, &serde_json::json!({"event_id":"replacement"})).is_err());
        assert_eq!(read::<serde_json::Value>(&pending)?, event);
        assert_eq!(
            std::fs::metadata(&pending)?.permissions().mode() & 0o777,
            0o600
        );
        drop(guard);
        let guard = lock(&path)?;
        remove(&pending)?;
        drop(guard);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn credentials_are_redacted_and_public_files_are_refused() -> Result<(), Error> {
        let state = State {
            origin: "https://example.com".into(),
            source: Source::UnoQ {
                serial: "private-serial".into(),
            },
            label: "private-label".into(),
            token: "private-token".into(),
        };
        assert_eq!(format!("{state:?}"), "Session { .. }");
        let dir =
            std::env::temp_dir().join(format!("nexa-hardware-test-{}", super::super::random_id()?));
        std::fs::create_dir(&dir)?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        let path = dir.join("device.json");
        write_new(&path, &state)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
        assert!(read::<State>(&path).is_err());
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
