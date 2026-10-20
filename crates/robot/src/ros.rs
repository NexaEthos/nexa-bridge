//! Wiki: docs/wiki/robot.md

use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::process::Command;

/// One actual ROS battery reading from the local robot graph.
#[derive(Debug, Serialize)]
pub struct BatterySnapshot {
    /// Host receipt time in milliseconds since the Unix epoch.
    pub observed_at_ms: u128,
    /// The voltage field of `sensor_msgs/msg/BatteryState`, in volts.
    pub voltage: f64,
    /// ROS distribution selected by the operator's sourced environment.
    pub ros_distribution: String,
    /// Kernel release of the robot host running this adapter.
    pub kernel_release: String,
    /// Processor architecture of this adapter's build.
    pub architecture: &'static str,
    /// Observing software version, not a claim about motor-controller firmware.
    pub adapter_version: &'static str,
}

/// An unavailable or malformed ROS observation is never replaced by an assumed voltage.
#[derive(Debug, thiserror::Error)]
pub enum RosError {
    /// ROS must already be installed and selected by the operator.
    #[error(
        "ROS_DISTRO is missing or invalid; source your installed ROS 2 setup before inspecting the battery"
    )]
    Environment,
    /// A fixed local read-only command could not run.
    #[error("Could not run {operation}; check your ROS 2 installation and sourced environment")]
    Command {
        /// Fixed operation that failed.
        operation: &'static str,
        /// Operating-system error without captured command output.
        #[source]
        source: std::io::Error,
    },
    /// The local graph or command did not respond.
    #[error("{0} timed out; check the local /battery publisher and retry")]
    Timeout(&'static str),
    /// A fixed command returned an error.
    #[error("{operation} exited with {code:?}; check the local ROS 2 graph and /battery publisher")]
    Refused {
        /// Fixed operation that failed.
        operation: &'static str,
        /// Child exit code, if present.
        code: Option<i32>,
    },
    /// No complete measurement can be formed from the response.
    #[error("Invalid {0}; no observation was produced. Inspect /battery locally before retrying")]
    Invalid(&'static str),
    /// The host clock must provide a usable receipt time.
    #[error("Host clock is before the Unix epoch; correct it before collecting observations")]
    Clock,
}

#[doc = "Read one battery voltage using the local ROS CLI.\n# Errors\nSee [robot.md](../../../docs/wiki/robot.md)."]
pub async fn inspect_battery() -> Result<BatterySnapshot, RosError> {
    let distro = std::env::var("ROS_DISTRO").map_err(|_| RosError::Environment)?;
    if distro.is_empty()
        || !distro
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(RosError::Environment);
    }
    let kernel = read("uname", &["-r"], "kernel release").await?;
    let kernel = kernel.trim();
    if kernel.is_empty() || kernel.chars().any(char::is_control) {
        return Err(RosError::Invalid("kernel release"));
    }
    let measurement = read(
        "ros2",
        &[
            "topic",
            "echo",
            "--no-daemon",
            "--once",
            "--timeout",
            "10",
            "/battery",
            "sensor_msgs/msg/BatteryState",
            "--field",
            "voltage",
            "--csv",
        ],
        "ROS battery observation",
    )
    .await?;
    Ok(BatterySnapshot {
        observed_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| RosError::Clock)?
            .as_millis(),
        voltage: parse_voltage(&measurement)?,
        ros_distribution: distro,
        kernel_release: kernel.into(),
        architecture: std::env::consts::ARCH,
        adapter_version: concat!("nexa-robot/", env!("CARGO_PKG_VERSION")),
    })
}

async fn read(program: &str, args: &[&str], operation: &'static str) -> Result<String, RosError> {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|source| RosError::Command { operation, source })?;
    let output = tokio::time::timeout(Duration::from_secs(15), child.wait_with_output())
        .await
        .map_err(|_| RosError::Timeout(operation))?
        .map_err(|source| RosError::Command { operation, source })?;
    if !output.status.success() {
        return Err(RosError::Refused {
            operation,
            code: output.status.code(),
        });
    }
    String::from_utf8(output.stdout).map_err(|_| RosError::Invalid("ROS command encoding"))
}

fn parse_voltage(text: &str) -> Result<f64, RosError> {
    let mut lines = text.lines();
    let voltage = lines
        .next()
        .ok_or(RosError::Invalid("missing battery voltage"))?
        .trim()
        .parse::<f64>()
        .map_err(|_| RosError::Invalid("battery voltage"))?;
    if !voltage.is_finite() || lines.next() != Some("---") || lines.next().is_some() {
        return Err(RosError::Invalid(
            "battery measurement framing or non-finite voltage",
        ));
    }
    Ok(voltage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_unknown_or_ambiguous_voltage_is_never_reported_as_a_measurement()
    -> Result<(), RosError> {
        assert!((parse_voltage("7.8\n---\n")? - 7.8).abs() < f64::EPSILON);
        assert_eq!(parse_voltage("0.0\n---\n")?.to_bits(), 0.0_f64.to_bits());
        for raw in [
            "",
            "nan\n---\n",
            "inf\n---\n",
            "7.8",
            "7.8\n---\n7.7\n---\n",
            "Warning\n7.8\n---\n",
            "7.8,7.7\n---\n",
        ] {
            assert!(parse_voltage(raw).is_err());
        }
        Ok(())
    }
}
