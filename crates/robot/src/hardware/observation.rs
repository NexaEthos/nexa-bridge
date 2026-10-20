//! Wiki: docs/wiki/robot.md

use crate::{
    board::{BoardSnapshot, inspect_uno_q},
    ros::{BatterySnapshot, inspect_battery},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Error, random_id, timestamp};

/// An explicitly selected, read-only local observation source.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    /// Linux diagnostics from the explicitly selected USB device.
    UnoQ {
        /// Exact local ADB device selector; never uploaded.
        serial: String,
    },
    /// Battery voltage collected on the local ROS 2 host.
    RosBattery,
}

impl Source {
    /// Hardware identity class declared during approval.
    #[must_use]
    pub const fn class(&self) -> &'static str {
        match self {
            Self::UnoQ { .. } => "lab_board",
            Self::RosBattery => "robot",
        }
    }

    pub(super) async fn capture(&self) -> Result<Observation, Error> {
        match self {
            Self::UnoQ { serial } => Ok(Observation::Board(inspect_uno_q(serial).await?)),
            Self::RosBattery => Ok(Observation::Battery(inspect_battery().await?)),
        }
    }
}

pub(super) enum Observation {
    Board(BoardSnapshot),
    Battery(BatterySnapshot),
}

impl Observation {
    fn version(&self) -> &str {
        match self {
            Self::Board(board) => &board.router_version,
            Self::Battery(battery) => battery.adapter_version,
        }
    }

    pub fn declaration(&self) -> Value {
        let (platform, name, params) = match self {
            Self::Board(b) => (
                json!({"os":"Linux", "os_version":b.kernel, "arch":b.architecture,"board":b.board_model}),
                "board.observe",
                json!({"transport":"usb_adb", "readings":["uptime","memory","router_state"]}),
            ),
            Self::Battery(b) => (
                json!({"os":std::env::consts::OS,"os_version":b.kernel_release,"arch":b.architecture}),
                "robot.observe",
                json!({"transport":"local_ros2", "topic":"/battery", "message_type":"sensor_msgs/msg/BatteryState", "field":"voltage", "unit":"V", "ros_distribution":b.ros_distribution}),
            ),
        };
        json!({"firmware_version":self.version(),"platform":platform,"capabilities":[{"name":name,"kind":"observe","params":params}]})
    }

    pub fn event(&self, asset: &str, device_id: &str) -> Result<Value, Error> {
        let (description, severity, kind, summary) = match self {
            Self::Board(b) => (
                format!(
                    "Linux board observed over USB: uptime {} seconds, available memory {} bytes; Arduino router {}.",
                    b.uptime_seconds, b.memory_available_bytes, b.router_state
                ),
                if b.router_state == "active" {
                    "info"
                } else {
                    "warning"
                },
                "linux_snapshot",
                serde_json::to_string(b)?,
            ),
            Self::Battery(b) => (
                format!("ROS battery voltage observed: {} V.", b.voltage),
                "info",
                "ros_battery_voltage",
                serde_json::to_string(b)?,
            ),
        };
        Ok(
            json!({"event_version":0,"event_id":random_id()?,"timestamp":timestamp()?,"asset":asset,
            "observation_type":"telemetry_summary","semantic_description":description,"severity":severity,"confidence":1.0,
            "evidence":[{"kind":kind,"ref":format!("device:{device_id}"),"summary":summary}],
            "provenance":{"device_id":device_id,"firmware_version":self.version()}}),
        )
    }
}
