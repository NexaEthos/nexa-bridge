//! Wiki: docs/wiki/tunnel-protocol.md

#![forbid(unsafe_code)]
#![deny(warnings, rust_2018_idioms, rust_2021_compatibility)]
#![warn(missing_docs, missing_debug_implementations)]

use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};

/// Multiplexed request identifier; zero is reserved for status frames.
pub type RequestId = u64;

/// Cap on one tunnel message, enforced on BOTH ends. Must stay above the bridge's `max_body_bytes`.
pub const MAX_WS_MSG_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// HTTP request metadata carried separately from the opaque body.
pub struct ReqMeta {
    /// HTTP method.
    pub method: String,
    /// Origin-form request target, including its optional query.
    pub path: String,
    /// Request header pairs; may contain credentials and must not be logged.
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
/// HTTP response metadata preceding streamed body chunks.
pub struct RespMeta {
    /// HTTP status code.
    pub status: u16,
    /// Response header pairs.
    pub headers: Vec<(String, String)>,
}

/// `[u8 type][u64 id big-endian][payload…]` — the request is buffered, the response is streamed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// relay → connector: a buffered request to forward to the local upstream.
    Req {
        /// Multiplexed request identifier.
        id: RequestId,
        /// HTTP request metadata.
        meta: ReqMeta,
        /// Complete, byte-opaque request body.
        body: Bytes,
    },
    /// connector → relay: response status + headers (precedes the body chunks).
    RespStart {
        /// Multiplexed request identifier.
        id: RequestId,
        /// HTTP response metadata.
        meta: RespMeta,
    },
    /// connector → relay: one streamed body chunk.
    RespChunk {
        /// Multiplexed request identifier.
        id: RequestId,
        /// Byte-opaque response chunk.
        data: Bytes,
    },
    /// connector → relay: the response body is complete.
    RespEnd {
        /// Multiplexed request identifier.
        id: RequestId,
    },
    /// connector → relay: the upstream call failed (terminates the stream).
    RespError {
        /// Multiplexed request identifier.
        id: RequestId,
        /// Caller-safe failure description, without internal transport details.
        message: String,
    },
    /// connector → relay: periodic bridge status report (JSON payload, schema-versioned).
    Status {
        /// JSON serving snapshot, not scoped to a request.
        payload: Bytes,
    },
}

const T_REQ: u8 = 0x01;
const T_RESP_START: u8 = 0x02;
const T_RESP_CHUNK: u8 = 0x03;
const T_RESP_END: u8 = 0x04;
const T_RESP_ERROR: u8 = 0x05;
const T_STATUS: u8 = 0x06;

impl Frame {
    /// Return the request identifier, or zero for a serving-status frame.
    pub const fn id(&self) -> RequestId {
        match self {
            Self::Req { id, .. }
            | Self::RespStart { id, .. }
            | Self::RespChunk { id, .. }
            | Self::RespEnd { id }
            | Self::RespError { id, .. } => *id,
            Self::Status { .. } => 0,
        }
    }

    #[doc = "# Errors\nRejects invalid metadata or frames above the message limit; see docs/wiki/tunnel-protocol.md."]
    pub fn encode(&self) -> anyhow::Result<Bytes> {
        let mut buf = BytesMut::new();
        match self {
            Self::Req { id, meta, body } => {
                buf.put_u8(T_REQ);
                buf.put_u64(*id);
                let meta_json = serde_json::to_vec(meta)?;
                ensure_frame_size(&[13, meta_json.len(), body.len()])?;
                buf.put_u32(u32::try_from(meta_json.len())?);
                buf.extend_from_slice(&meta_json);
                buf.extend_from_slice(body);
            }
            Self::RespStart { id, meta } => {
                buf.put_u8(T_RESP_START);
                buf.put_u64(*id);
                let meta_json = serde_json::to_vec(meta)?;
                ensure_frame_size(&[9, meta_json.len()])?;
                buf.extend_from_slice(&meta_json);
            }
            Self::RespChunk { id, data } => {
                ensure_frame_size(&[9, data.len()])?;
                buf.put_u8(T_RESP_CHUNK);
                buf.put_u64(*id);
                buf.extend_from_slice(data);
            }
            Self::RespEnd { id } => {
                buf.put_u8(T_RESP_END);
                buf.put_u64(*id);
            }
            Self::RespError { id, message } => {
                ensure_frame_size(&[9, message.len()])?;
                buf.put_u8(T_RESP_ERROR);
                buf.put_u64(*id);
                buf.extend_from_slice(message.as_bytes());
            }
            Self::Status { payload } => {
                ensure_frame_size(&[9, payload.len()])?;
                buf.put_u8(T_STATUS);
                buf.put_u64(0);
                buf.extend_from_slice(payload);
            }
        }
        Ok(buf.freeze())
    }

    #[doc = "# Errors\nRejects malformed or unknown frames; see docs/wiki/tunnel-protocol.md."]
    pub fn decode(mut bytes: Bytes) -> anyhow::Result<Self> {
        if bytes.len() < 9 {
            anyhow::bail!("frame too short ({} bytes)", bytes.len());
        }
        let t = bytes.get_u8();
        let id = bytes.get_u64();
        match t {
            T_REQ => {
                if bytes.len() < 4 {
                    anyhow::bail!("req frame missing meta length");
                }
                let meta_len = bytes.get_u32() as usize;
                if bytes.len() < meta_len {
                    anyhow::bail!("req frame meta truncated");
                }
                let meta_bytes = bytes.split_to(meta_len);
                let meta: ReqMeta = serde_json::from_slice(&meta_bytes)?;
                Ok(Self::Req {
                    id,
                    meta,
                    body: bytes,
                })
            }
            T_RESP_START => {
                let meta: RespMeta = serde_json::from_slice(&bytes)?;
                Ok(Self::RespStart { id, meta })
            }
            T_RESP_CHUNK => Ok(Self::RespChunk { id, data: bytes }),
            T_RESP_END => Ok(Self::RespEnd { id }),
            T_RESP_ERROR => Ok(Self::RespError {
                id,
                message: String::from_utf8_lossy(&bytes).into_owned(),
            }),
            T_STATUS => Ok(Self::Status { payload: bytes }),
            other => anyhow::bail!("unknown frame type {other:#x}"),
        }
    }
}

impl Frame {
    /// Skips unknown frame types; see docs/wiki/tunnel-protocol.md for compatibility rules.
    #[doc = "# Errors\nRejects malformed known frames and incomplete headers, as does `Frame::decode`."]
    pub fn decode_lenient(bytes: Bytes) -> anyhow::Result<Option<Self>> {
        if bytes.len() >= 9 {
            let t = bytes[0];
            if !matches!(
                t,
                T_REQ | T_RESP_START | T_RESP_CHUNK | T_RESP_END | T_RESP_ERROR | T_STATUS
            ) {
                return Ok(None);
            }
        }
        Self::decode(bytes).map(Some)
    }
}

fn ensure_frame_size(parts: &[usize]) -> anyhow::Result<()> {
    let size = parts.iter().try_fold(0usize, |size, part| {
        size.checked_add(*part)
            .ok_or_else(|| anyhow::anyhow!("tunnel frame length overflow"))
    })?;
    anyhow::ensure!(
        size <= MAX_WS_MSG_BYTES,
        "tunnel frame exceeds message limit"
    );
    Ok(())
}

/// Observed serving facts, not inferred capabilities; see docs/wiki/tunnel-protocol.md.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelStatus {
    /// Exact model identifier reported by the upstream.
    pub id: String,
    /// Runtime-reported task string; consumers must not assume a closed set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// The context length the model was actually LOADED with, when the runtime reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
    /// Quantization label as reported by the runtime (e.g. `Q6_K`, `bf16`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantization: Option<String>,
}

/// One upstream's health and load as seen by the bridge's poller.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpstreamStatus {
    /// Configured model-server origin.
    pub url: String,
    /// Result of the most recent serving probe.
    pub healthy: bool,
    /// Requests the bridge has in flight to it now; absent from bridges older than the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inflight: Option<usize>,
    /// Its concurrency ceiling, when the pool sets one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_inflight: Option<usize>,
    /// True while the circuit breaker sets it aside after consecutive failures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub circuit_open: Option<bool>,
}

/// The periodic status payload a connector reports up the tunnel (`Frame::Status`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeStatus {
    /// Payload schema version — bump only on a BREAKING shape change.
    pub schema: u32,
    /// Bridge package version.
    pub bridge_version: String,
    /// `server` | `desktop`.
    pub flavor: String,
    /// Models currently exposed by this bridge.
    pub models: Vec<ModelStatus>,
    /// Observed upstream health.
    pub upstreams: Vec<UpstreamStatus>,
    /// The build block of the bridge that produced it; absent from bridges older than the block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildInfo>,
}

impl BridgeStatus {
    /// Current serving-status schema version.
    pub const SCHEMA: u32 = 1;
}

/// The build block every binary carries: a claim the client makes about itself, never an authorisation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BuildInfo {
    /// Package version.
    pub version: String,
    /// The build's git revision, `-dirty` when tracked files differed from it, or `unknown`.
    pub commit: String,
    /// Target triple.
    pub target: String,
    /// `release` or `debug`.
    pub profile: String,
    /// Declared by the build configuration: `official` or `custom`.
    pub channel: String,
    /// The declared default Agora origin, when the binary carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agora_origin: Option<String>,
}

impl BuildInfo {
    /// One line for `--version`: `<binary> <version> (<commit>, <target>, <profile>, <channel>)`.
    #[must_use]
    pub fn describe(&self, binary: &str) -> String {
        let origin = self
            .agora_origin
            .as_deref()
            .map_or_else(String::new, |origin| format!(", agora {origin}"));
        format!(
            "{binary} {} ({}, {}, {}, {}{origin})",
            self.version, self.commit, self.target, self.profile, self.channel
        )
    }
}

/// The build block of the crate that expands it, from the values its build script emitted.
#[macro_export]
macro_rules! build_info {
    () => {
        $crate::BuildInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit: env!("NEXA_BUILD_COMMIT").to_string(),
            target: env!("NEXA_BUILD_TARGET").to_string(),
            profile: env!("NEXA_BUILD_PROFILE").to_string(),
            channel: env!("NEXA_BUILD_CHANNEL").to_string(),
            agora_origin: None,
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(f: &Frame) -> anyhow::Result<()> {
        let decoded = Frame::decode(f.encode()?)?;
        assert_eq!(&decoded, f);
        Ok(())
    }

    #[test]
    fn req_roundtrips_with_body() -> anyhow::Result<()> {
        roundtrip(&Frame::Req {
            id: 42,
            meta: ReqMeta {
                method: "POST".into(),
                path: "/v1/chat/completions".into(),
                headers: vec![("content-type".into(), "application/json".into())],
            },
            body: Bytes::from_static(b"{\"model\":\"x\"}"),
        })
    }

    #[test]
    fn stream_options_body_roundtrips_byte_identical() -> anyhow::Result<()> {
        roundtrip(&Frame::Req {
            id: 42,
            meta: ReqMeta {
                method: "POST".into(),
                path: "/v1/chat/completions".into(),
                headers: vec![("content-type".into(), "application/json".into())],
            },
            body: Bytes::from_static(
                br#"{"model":"qwen","stream":true,"stream_options":{"include_usage":true},"messages":[]}"#,
            ),
        })
    }

    #[test]
    fn resp_frames_roundtrip() -> anyhow::Result<()> {
        roundtrip(&Frame::RespStart {
            id: 7,
            meta: RespMeta {
                status: 200,
                headers: vec![("content-type".into(), "text/event-stream".into())],
            },
        })?;
        roundtrip(&Frame::RespChunk {
            id: 7,
            data: Bytes::from_static(b"data: hi\n\n"),
        })?;
        roundtrip(&Frame::RespEnd { id: 7 })?;
        roundtrip(&Frame::RespError {
            id: 7,
            message: "upstream boom".into(),
        })
    }

    #[test]
    fn rejects_short_and_unknown() {
        assert!(Frame::decode(Bytes::from_static(b"\x01\x00")).is_err());
        let mut bad = BytesMut::new();
        bad.put_u8(0xff);
        bad.put_u64(1);
        assert!(Frame::decode(bad.freeze()).is_err());
    }

    #[test]
    fn request_encoding_preserves_wire_bytes() -> anyhow::Result<()> {
        let frame = Frame::Req {
            id: 42,
            meta: ReqMeta {
                method: "POST".into(),
                path: "/v1/chat/completions".into(),
                headers: Vec::new(),
            },
            body: Bytes::from_static(b"{}"),
        };
        let metadata = br#"{"method":"POST","path":"/v1/chat/completions","headers":[]}"#;
        let mut expected = vec![1];
        expected.extend_from_slice(&42u64.to_be_bytes());
        expected.extend_from_slice(&u32::try_from(metadata.len())?.to_be_bytes());
        expected.extend_from_slice(metadata);
        expected.extend_from_slice(b"{}");
        assert_eq!(frame.encode()?.as_ref(), expected);
        Ok(())
    }

    #[test]
    fn encoding_rejects_oversized_frames_and_length_overflow() -> anyhow::Result<()> {
        ensure_frame_size(&[9, MAX_WS_MSG_BYTES - 9])?;
        assert!(ensure_frame_size(&[9, MAX_WS_MSG_BYTES - 8]).is_err());
        assert!(ensure_frame_size(&[usize::MAX, 1]).is_err());
        let payload = Bytes::from(vec![0; MAX_WS_MSG_BYTES]);
        assert!(
            Frame::Status {
                payload: payload.clone()
            }
            .encode()
            .is_err()
        );
        assert!(
            Frame::RespChunk {
                id: 1,
                data: payload
            }
            .encode()
            .is_err()
        );
        Ok(())
    }
}

#[cfg(test)]
mod status_frame_tests {
    use super::*;

    #[test]
    fn status_frame_roundtrips() -> anyhow::Result<()> {
        let status = BridgeStatus {
            schema: BridgeStatus::SCHEMA,
            bridge_version: "0.3.0".into(),
            flavor: "server".into(),
            models: vec![ModelStatus {
                id: "qwen3.8-27b".into(),
                task: Some("text-generation".into()),
                context_length: Some(32768),
                quantization: Some("bf16".into()),
            }],
            build: None,
            upstreams: vec![UpstreamStatus {
                url: "http://127.0.0.1:8000".into(),
                healthy: true,
                inflight: Some(2),
                max_inflight: Some(16),
                circuit_open: Some(false),
            }],
        };
        let payload = Bytes::from(serde_json::to_vec(&status)?);
        let frame = Frame::Status { payload };
        let decoded = Frame::decode(frame.encode()?)?;
        let Frame::Status { payload } = decoded else {
            anyhow::bail!("wrong variant");
        };
        let back: BridgeStatus = serde_json::from_slice(&payload)?;
        assert_eq!(back.models[0].context_length, Some(32768));
        Ok(())
    }

    #[test]
    fn lenient_decode_skips_unknown_frame_types() -> anyhow::Result<()> {
        let mut raw = vec![0x7fu8];
        raw.extend_from_slice(&0u64.to_be_bytes());
        raw.extend_from_slice(b"future frame");
        assert!(Frame::decode_lenient(Bytes::from(raw))?.is_none());
        // Known frames still decode through the lenient door.
        let known = Frame::RespEnd { id: 7 };
        assert_eq!(
            Frame::decode_lenient(known.encode()?)?,
            Some(Frame::RespEnd { id: 7 })
        );
        Ok(())
    }

    #[test]
    fn status_payload_tolerates_unknown_fields() -> anyhow::Result<()> {
        let json = r#"{"schema":1,"bridge_version":"9.9","flavor":"desktop","models":[],"upstreams":[],"future_field":true}"#;
        let parsed: BridgeStatus = serde_json::from_str(json)?;
        assert_eq!(parsed.flavor, "desktop");
        assert!(
            parsed.build.is_none(),
            "a bridge older than the block reports none"
        );
        Ok(())
    }

    #[test]
    fn the_build_block_rides_the_status_and_describes_itself() -> anyhow::Result<()> {
        let build = BuildInfo {
            version: "0.1.0".into(),
            commit: "0123456789ab-dirty".into(),
            target: "aarch64-apple-darwin".into(),
            profile: "release".into(),
            channel: "custom".into(),
            agora_origin: Some("https://agora.example.com".into()),
        };
        assert_eq!(
            build.describe("nexa-llm-bridge"),
            "nexa-llm-bridge 0.1.0 (0123456789ab-dirty, aarch64-apple-darwin, release, custom, agora https://agora.example.com)"
        );
        let json = r#"{"schema":1,"bridge_version":"0.1.0","flavor":"desktop","models":[],"upstreams":[],"build":{"version":"0.1.0","commit":"0123456789ab","target":"x86_64-unknown-linux-musl","profile":"release","channel":"official"}}"#;
        let parsed: BridgeStatus = serde_json::from_str(json)?;
        let carried = parsed
            .build
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("the block"))?;
        assert_eq!(carried.channel, "official");
        assert!(carried.agora_origin.is_none());
        assert_eq!(
            serde_json::to_value(&parsed)?["build"]["commit"],
            "0123456789ab"
        );
        Ok(())
    }
}
