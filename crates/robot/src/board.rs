//! Wiki: docs/wiki/robot.md

use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::process::Command;

/// A Linux-side observation, without USB identity or microcontroller claims.
#[derive(Debug, Serialize)]
pub struct BoardSnapshot {
    /// Host receipt time in milliseconds since the Unix epoch.
    pub observed_at_ms: u128,
    /// Board model reported by the device tree.
    pub board_model: String,
    /// Kernel name and release.
    pub kernel: String,
    /// Processor architecture reported by the board.
    pub architecture: String,
    /// Elapsed time since the board booted.
    pub uptime_seconds: f64,
    /// Physical memory reported by Linux, in bytes.
    pub memory_total_bytes: u64,
    /// Memory available to applications, in bytes.
    pub memory_available_bytes: u64,
    /// Installed Arduino RPC router version.
    pub router_version: String,
    /// Actual systemd state of the Arduino RPC router.
    pub router_state: String,
}

/// A failed observation never substitutes zero measurements or an online state.
#[derive(Debug, thiserror::Error)]
pub enum BoardError {
    /// The operator must select one exact USB device.
    #[error(
        "Select the board's nonempty ADB serial with inspect-uno-q <serial>; run adb devices -l to find it"
    )]
    MissingDevice,
    /// The local ADB tool could not execute.
    #[error("Could not run {operation}; check the ADB installation and USB access")]
    Command {
        /// Fixed diagnostic operation that failed.
        operation: &'static str,
        /// Operating-system failure, without captured command output.
        #[source]
        source: std::io::Error,
    },
    /// USB or board-side execution stopped responding.
    #[error("{0} timed out; check the USB cable and board, then retry inspection")]
    Timeout(&'static str),
    /// A fixed diagnostic command was refused or unavailable.
    #[error(
        "{operation} failed with exit status {code:?}; check adb devices -l and the board's Arduino installation"
    )]
    Refused {
        /// Fixed diagnostic operation that failed.
        operation: &'static str,
        /// Child exit code, if the process exited normally.
        code: Option<i32>,
    },
    /// An observation cannot be interpreted as the declared measurement.
    #[error("Invalid {0} from the board; no observation was produced")]
    Invalid(&'static str),
    /// Host time must be usable as an observation receipt timestamp.
    #[error("Host clock is before the Unix epoch; correct it before collecting observations")]
    Clock,
}

#[doc = "Inspect UNO Q through fixed read-only ADB operations.\n# Errors\nSee [robot.md](../../../docs/wiki/robot.md)."]
pub async fn inspect_uno_q(serial: &str) -> Result<BoardSnapshot, BoardError> {
    if serial.is_empty() || serial.chars().any(char::is_control) {
        return Err(BoardError::MissingDevice);
    }
    let state = probe(serial, "device state", &["get-state"], false).await?;
    if state.trim() != "device" {
        return Err(BoardError::Invalid("ADB device state"));
    }
    let board_model = probe(
        serial,
        "board model",
        &["shell", "cat", "/proc/device-tree/model"],
        false,
    )
    .await?;
    let kernel = probe(serial, "kernel", &["shell", "uname", "-sr"], false).await?;
    let architecture = probe(serial, "architecture", &["shell", "uname", "-m"], false).await?;
    let uptime = probe(serial, "uptime", &["shell", "cat", "/proc/uptime"], false).await?;
    let memory = probe(serial, "memory", &["shell", "cat", "/proc/meminfo"], false).await?;
    let router_version = probe(
        serial,
        "router version",
        &["shell", "arduino-router", "version"],
        false,
    )
    .await?;
    let router_state = probe(
        serial,
        "router state",
        &["shell", "systemctl", "is-active", "arduino-router"],
        true,
    )
    .await?;
    let (memory_total_bytes, memory_available_bytes) = parse_memory(&memory)?;
    let router_state = clean_text(&router_state, "router state")?;
    if !matches!(
        router_state.as_str(),
        "active"
            | "inactive"
            | "failed"
            | "activating"
            | "deactivating"
            | "reloading"
            | "maintenance"
            | "refreshing"
            | "unknown"
    ) {
        return Err(BoardError::Invalid("router state"));
    }
    Ok(BoardSnapshot {
        observed_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| BoardError::Clock)?
            .as_millis(),
        board_model: clean_text(&board_model, "board model")?,
        kernel: clean_text(&kernel, "kernel")?,
        architecture: clean_text(&architecture, "architecture")?,
        uptime_seconds: parse_uptime(&uptime)?,
        memory_total_bytes,
        memory_available_bytes,
        router_version: clean_text(&router_version, "router version")?,
        router_state,
    })
}

async fn probe(
    serial: &str,
    operation: &'static str,
    args: &[&str],
    state_query: bool,
) -> Result<String, BoardError> {
    let child = Command::new("adb")
        .args(["-s", serial])
        .args(args)
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
    if !(output.status.success() || state_query && matches!(output.status.code(), Some(3 | 4))) {
        return Err(BoardError::Refused {
            operation,
            code: output.status.code(),
        });
    }
    String::from_utf8(output.stdout).map_err(|_| BoardError::Invalid(operation))
}

fn clean_text(value: &str, name: &'static str) -> Result<String, BoardError> {
    let value = value.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(BoardError::Invalid(name));
    }
    Ok(value.to_owned())
}

fn parse_uptime(value: &str) -> Result<f64, BoardError> {
    let seconds: f64 = value
        .split_whitespace()
        .next()
        .ok_or(BoardError::Invalid("uptime"))?
        .parse()
        .map_err(|_| BoardError::Invalid("uptime"))?;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(BoardError::Invalid("uptime"));
    }
    Ok(seconds)
}

fn parse_memory(value: &str) -> Result<(u64, u64), BoardError> {
    let mut total = None;
    let mut available = None;
    for line in value.lines() {
        let mut fields = line.split_whitespace();
        let target = match fields.next() {
            Some("MemTotal:") => &mut total,
            Some("MemAvailable:") => &mut available,
            _ => continue,
        };
        if target.is_some() {
            return Err(BoardError::Invalid("duplicate memory field"));
        }
        let kib: u64 = fields
            .next()
            .ok_or(BoardError::Invalid("memory"))?
            .parse()
            .map_err(|_| BoardError::Invalid("memory"))?;
        if fields.next() != Some("kB") || fields.next().is_some() {
            return Err(BoardError::Invalid("memory unit"));
        }
        *target = Some(
            kib.checked_mul(1024)
                .ok_or(BoardError::Invalid("memory overflow"))?,
        );
    }
    let total = total.ok_or(BoardError::Invalid("total memory"))?;
    let available = available.ok_or(BoardError::Invalid("available memory"))?;
    if total == 0 || available > total {
        return Err(BoardError::Invalid("memory range"));
    }
    Ok((total, available))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_measurements_preserve_units_and_reject_false_values() -> Result<(), BoardError> {
        assert_eq!(
            parse_memory("MemTotal: 3758016 kB\nMemAvailable: 3239852 kB\nMemFree: 2851900 kB")?,
            (3_848_208_384, 3_317_608_448)
        );
        assert_eq!(
            parse_uptime("1003.60 3910.58")?.to_bits(),
            1003.6_f64.to_bits()
        );
        for value in ["NaN", "inf", "-1", ""] {
            assert!(parse_uptime(value).is_err());
        }
        for value in [
            "MemTotal: 1 kB",
            "MemTotal: 1 kB\nMemAvailable: 2 kB",
            "MemTotal: 1 MB\nMemAvailable: 0 kB",
            "MemTotal: 1 kB\nMemTotal: 1 kB\nMemAvailable: 0 kB",
            "MemTotal: 18446744073709551615 kB\nMemAvailable: 0 kB",
        ] {
            assert!(parse_memory(value).is_err());
        }
        assert_eq!(
            clean_text("Arduino SA,Imola\0", "model")?,
            "Arduino SA,Imola"
        );
        assert!(clean_text("\0", "model").is_err());
        assert!(clean_text("active\nfailed", "state").is_err());
        Ok(())
    }
}
