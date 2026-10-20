//! Wiki: docs/wiki/robot.md

use crate::board::BoardError;
use serde::Serialize;
use std::{process::Stdio, time::Duration};
use tokio::process::Command;

/// A local ADB inventory entry, not a claim that this device is an Arduino board.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct AttachedDevice {
    /// Exact selector shown only on the local computer.
    pub serial: String,
    /// ADB transport state, including offline or unauthorized.
    pub state: String,
    /// Optional model advertised by ADB, before diagnostic inspection.
    pub model: Option<String>,
}

/// List attached ADB transports without choosing or authorizing a device.
/// # Errors
/// Reports missing ADB, timeout, failed execution or malformed inventory.
pub async fn inventory() -> Result<Vec<AttachedDevice>, BoardError> {
    let operation = "ADB inventory";
    let child = Command::new("adb")
        .args(["devices", "-l"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|source| BoardError::Command { operation, source })?;
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .map_err(|_| BoardError::Timeout(operation))?
        .map_err(|source| BoardError::Command { operation, source })?;
    if !output.status.success() {
        return Err(BoardError::Refused {
            operation,
            code: output.status.code(),
        });
    }
    parse(&String::from_utf8(output.stdout).map_err(|_| BoardError::Invalid(operation))?)
}

fn parse(text: &str) -> Result<Vec<AttachedDevice>, BoardError> {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("List of devices attached") {
        return Err(BoardError::Invalid("ADB inventory header"));
    }
    let mut devices: Vec<AttachedDevice> = Vec::new();
    for line in lines.filter(|line| !line.trim().is_empty()) {
        let mut fields = line.split_whitespace();
        let serial = fields.next().ok_or(BoardError::Invalid("ADB serial"))?;
        let state = fields.next().ok_or(BoardError::Invalid("ADB state"))?;
        if devices.iter().any(|device| device.serial == serial) {
            return Err(BoardError::Invalid("duplicate ADB selector"));
        }
        let model = fields
            .find_map(|field| field.strip_prefix("model:"))
            .map(str::to_owned);
        devices.push(AttachedDevice {
            serial: serial.into(),
            state: state.into(),
            model,
        });
    }
    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inventory_preserves_unavailable_devices_without_selecting_one() -> Result<(), BoardError> {
        let devices = parse(
            "List of devices attached\nboard-a device usb:1 model:UNO_Q\nboard-b unauthorized usb:2\nboard-c offline\n",
        )?;
        assert_eq!(devices.len(), 3);
        assert_eq!(devices[0].model.as_deref(), Some("UNO_Q"));
        assert_eq!(devices[1].state, "unauthorized");
        assert_eq!(devices[2].state, "offline");
        assert!(parse("List of devices attached\n").is_ok_and(|items| items.is_empty()));
        assert!(parse("error: no device").is_err());
        assert!(parse("List of devices attached\nsame device\nsame offline\n").is_err());
        Ok(())
    }
}
