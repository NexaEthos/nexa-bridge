//! Wiki: docs/wiki/robot.md#foxglove-encoders

use serde_json::{Value, json};

/// First op after a client connects.
#[must_use]
pub fn server_info(name: &str) -> Value {
    json!({
        "op": "serverInfo",
        "name": name,
        "capabilities": [],
        "supportedEncodings": ["json"],
    })
}

/// One advertised channel — an allowlisted topic surfaced to Foxglove.
#[must_use]
pub fn advertise_channel(id: u32, topic: &str, schema_name: &str) -> Value {
    json!({
        "op": "advertise",
        "channels": [{
            "id": id,
            "topic": topic,
            "encoding": "json",
            "schemaName": schema_name,
            "schema": "",
        }],
    })
}

/// Binary frame: opcode 0x01, subscription id LE u32, receive time LE u64 ns, payload.
#[must_use]
pub fn message_data(subscription_id: u32, receive_ns: u64, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(1 + 4 + 8 + payload.len());
    frame.push(0x01);
    frame.extend_from_slice(&subscription_id.to_le_bytes());
    frame.extend_from_slice(&receive_ns.to_le_bytes());
    frame.extend_from_slice(payload);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_info_shape() {
        let v = server_info("nexa-robot");
        assert_eq!(v["op"], "serverInfo");
        assert_eq!(v["name"], "nexa-robot");
    }

    #[test]
    fn advertise_carries_topic_and_id() {
        let v = advertise_channel(7, "/rover/odom", "nav_msgs/msg/Odometry");
        assert_eq!(v["op"], "advertise");
        assert_eq!(v["channels"][0]["id"], 7);
        assert_eq!(v["channels"][0]["topic"], "/rover/odom");
    }

    #[test]
    fn message_data_frame_layout() -> Result<(), std::array::TryFromSliceError> {
        let frame = message_data(7, 1_000, b"{}");
        assert_eq!(frame[0], 0x01);
        assert_eq!(u32::from_le_bytes(frame[1..5].try_into()?), 7);
        assert_eq!(u64::from_le_bytes(frame[5..13].try_into()?), 1_000);
        assert_eq!(&frame[13..], b"{}");
        Ok(())
    }
}
