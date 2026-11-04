//! Wiki: docs/wiki/robot.md

use crate::{
    board::{BoardSnapshot, inspect_uno_q},
    ros::{BatterySnapshot, inspect_battery},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

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
    /// A device that describes itself in a declaration file and writes its own events to an outbox.
    Declared {
        /// Absolute path of the JSON declaration, read at pairing and at every heartbeat.
        declaration: PathBuf,
        /// The hardware class the device pairs as.
        class: DeclaredClass,
    },
}

/// The hardware classes a declared device may pair as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclaredClass {
    /// A board beside a bench.
    LabBoard,
    /// A robot.
    Robot,
}

impl DeclaredClass {
    /// Read a class from its wire name.
    /// # Errors
    /// Rejects any class other than `lab_board` and `robot`.
    pub fn parse(name: &str) -> Result<Self, Error> {
        match name {
            "lab_board" => Ok(Self::LabBoard),
            "robot" => Ok(Self::Robot),
            _ => Err(Error::Contract(
                "A declared device pairs as lab_board or robot",
            )),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::LabBoard => "lab_board",
            Self::Robot => "robot",
        }
    }
}

impl Source {
    /// Hardware identity class declared during approval.
    #[must_use]
    pub const fn class(&self) -> &'static str {
        match self {
            Self::UnoQ { .. } => "lab_board",
            Self::RosBattery => "robot",
            Self::Declared { class, .. } => class.as_str(),
        }
    }

    pub(super) async fn capture(&self) -> Result<Observation, Error> {
        match self {
            Self::UnoQ { serial } => Ok(Observation::Board(inspect_uno_q(serial).await?)),
            Self::RosBattery => Ok(Observation::Battery(inspect_battery().await?)),
            Self::Declared { declaration, .. } => {
                Ok(Observation::Declared(read_declaration(declaration)?))
            }
        }
    }
}

/// Read and check a declaration: exactly a firmware version, a platform and observe-only capabilities.
fn read_declaration(path: &std::path::Path) -> Result<Value, Error> {
    if !path.is_absolute() {
        return Err(Error::Contract(
            "Name the declaration file by its absolute path",
        ));
    }
    let declaration: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    check_declaration(&declaration)?;
    Ok(declaration)
}

fn check_declaration(declaration: &Value) -> Result<(), Error> {
    let object = declaration
        .as_object()
        .ok_or(Error::Contract("A declaration is a JSON object"))?;
    if object.len() != 3
        || !["firmware_version", "platform", "capabilities"]
            .iter()
            .all(|key| object.contains_key(*key))
    {
        return Err(Error::Contract(
            "A declaration holds exactly firmware_version, platform and capabilities",
        ));
    }
    let version = object["firmware_version"].as_str().unwrap_or_default();
    if version.is_empty() || version.chars().count() > 64 || version.chars().any(char::is_control) {
        return Err(Error::Contract(
            "firmware_version is 1–64 printable characters",
        ));
    }
    let platform = object["platform"]
        .as_object()
        .ok_or(Error::Contract("platform is an object"))?;
    if platform.iter().any(|(key, value)| {
        !matches!(key.as_str(), "os" | "os_version" | "arch" | "board") || !value.is_string()
    }) {
        return Err(Error::Contract(
            "platform holds only os, os_version, arch and board, as strings",
        ));
    }
    let capabilities = object["capabilities"]
        .as_array()
        .filter(|list| !list.is_empty())
        .ok_or(Error::Contract("capabilities is a non-empty list"))?;
    for capability in capabilities {
        let name = capability
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let shaped = name.split_once('.').is_some_and(|pair: (&str, &str)| {
            <[&str; 2]>::from(pair).iter().all(|part| {
                part.starts_with(|c: char| c.is_ascii_lowercase())
                    && part
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            })
        });
        if !shaped || capability.get("kind").and_then(Value::as_str) != Some("observe") {
            return Err(Error::Contract(
                "Each capability is named <domain>.<verb> and observes; actuation is not declared here",
            ));
        }
    }
    Ok(())
}

pub(super) enum Observation {
    Board(BoardSnapshot),
    Battery(BatterySnapshot),
    Declared(Value),
}

impl Observation {
    fn version(&self) -> &str {
        match self {
            Self::Board(board) => &board.router_version,
            Self::Battery(battery) => battery.adapter_version,
            Self::Declared(declaration) => {
                declaration["firmware_version"].as_str().unwrap_or_default()
            }
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
            Self::Declared(declaration) => return declaration.clone(),
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
            Self::Declared(_) => {
                return Err(Error::Contract(
                    "A declared device writes its own events; deliver them from its outbox",
                ));
            }
        };
        Ok(
            json!({"event_version":0,"event_id":random_id()?,"timestamp":timestamp()?,"asset":asset,
            "observation_type":"telemetry_summary","semantic_description":description,"severity":severity,"confidence":1.0,
            "evidence":[{"kind":kind,"ref":format!("device:{device_id}"),"summary":summary}],
            "provenance":{"device_id":device_id,"firmware_version":self.version()}}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{DeclaredClass, check_declaration};
    use serde_json::json;

    fn declaration() -> serde_json::Value {
        json!({"firmware_version": "nexa-edge 0.1.0",
               "platform": {"os": "Ubuntu", "os_version": "24.04", "arch": "aarch64", "board": "Arduino VENTUNO Q"},
               "capabilities": [{"name": "bench.observe", "kind": "observe", "params": {"transport": "modbus_tcp"}}]})
    }

    #[test]
    fn a_declaration_holds_its_three_parts_and_observes_only() {
        assert!(check_declaration(&declaration()).is_ok());
        let mut extra = declaration();
        extra["device_class"] = json!("lab_board");
        assert!(check_declaration(&extra).is_err());
        let mut actuating = declaration();
        actuating["capabilities"][0]["kind"] = json!("actuate");
        assert!(check_declaration(&actuating).is_err());
        let mut misnamed = declaration();
        misnamed["capabilities"][0]["name"] = json!("Bench.Observe");
        assert!(check_declaration(&misnamed).is_err());
        let mut serial = declaration();
        serial["platform"]["serial"] = json!("1195033651");
        assert!(check_declaration(&serial).is_err());
        let mut empty = declaration();
        empty["capabilities"] = json!([]);
        assert!(check_declaration(&empty).is_err());
    }

    #[test]
    fn only_two_classes_may_be_declared() {
        assert_eq!(
            DeclaredClass::parse("lab_board").ok(),
            Some(DeclaredClass::LabBoard)
        );
        assert_eq!(
            DeclaredClass::parse("robot").ok(),
            Some(DeclaredClass::Robot)
        );
        assert!(DeclaredClass::parse("llm_node").is_err());
    }
}
