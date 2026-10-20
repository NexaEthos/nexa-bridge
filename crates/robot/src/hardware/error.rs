//! Wiki: docs/wiki/robot.md

use serde_json::Value;

/// Hardware observation, transport or persistence failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// State persistence failed.
    #[error("Hardware state file could not be accessed: {0}")]
    Io(#[from] std::io::Error),
    /// The stored document or server response is malformed.
    #[error("Hardware state or response contains invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The local board could not be observed.
    #[error(transparent)]
    Board(#[from] crate::board::BoardError),
    /// The local ROS source could not be observed.
    #[error(transparent)]
    Ros(#[from] crate::ros::RosError),
    /// The HTTPS operation failed without disclosing its URL.
    #[error(
        "Hardware HTTPS request failed: {0}; retry the same observation to preserve its event ID"
    )]
    Transport(reqwest::Error),
    /// An invariant failed; the message describes recovery.
    #[error("{0}")]
    Contract(&'static str),
    /// The platform refused this operation.
    #[error(
        "Hardware API returned HTTP {status}{}; check the issuer, device approval and revocation state; pending observations are retained",
        .refusal.as_ref().map_or_else(String::new, Refusal::suffix)
    )]
    Http {
        /// HTTP status returned by the platform.
        status: u16,
        /// Documented refusal text, when available.
        refusal: Option<Refusal>,
    },
    /// The same pending event must wait before retrying.
    #[error("Observation retained; retry after {0} seconds with the same state file")]
    RateLimited(u64),
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Self::Transport(error.without_url())
    }
}

/// The platform's refusal: its stable code, when it has one, and the sentence that names the way out.
#[derive(Debug, PartialEq, Eq)]
pub struct Refusal {
    /// Stable machine-readable refusal code.
    pub code: Option<String>,
    /// Human-readable recovery instruction.
    pub detail: String,
}

impl Refusal {
    /// Read only the platform's documented text or coded refusal shapes.
    pub fn parse(body: &[u8]) -> Option<Self> {
        let value: Value = serde_json::from_slice(body).ok()?;
        match value.get("detail")? {
            Value::String(detail) => Self::new(None, detail),
            Value::Object(inner) => {
                let detail = inner.get("detail")?.as_str()?;
                let code = inner.get("code").and_then(Value::as_str);
                Self::new(code, detail)
            }
            _ => None,
        }
    }

    fn new(code: Option<&str>, detail: &str) -> Option<Self> {
        let detail = printable(detail);
        if detail.is_empty() {
            return None;
        }
        let code = code.map(printable).filter(|code| !code.is_empty());
        Some(Self { code, detail })
    }

    fn suffix(&self) -> String {
        self.code.as_ref().map_or_else(
            || format!(": {}", self.detail),
            |code| format!(" ({code}): {}", self.detail),
        )
    }
}

/// Server text reaches an operator's terminal: control characters are dropped, never interpreted.
fn printable(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_refusal_shapes_are_read() {
        let coded = br#"{"detail":{"code":"workspace_unavailable","detail":"The workspace is suspended","extra":"x"}}"#;
        assert_eq!(
            Refusal::parse(coded),
            Some(Refusal {
                code: Some("workspace_unavailable".into()),
                detail: "The workspace is suspended".into(),
            })
        );
        assert_eq!(
            Refusal::parse(br#"{"detail":"Device credential is unknown or revoked"}"#),
            Some(Refusal {
                code: None,
                detail: "Device credential is unknown or revoked".into(),
            })
        );
        for body in [
            &b"<html>proxy error</html>"[..],
            br#"{"message":"elsewhere"}"#,
            br#"{"detail":[{"loc":["body"],"msg":"invalid"}]}"#,
            br#"{"detail":{"code":"no_sentence"}}"#,
            br#"{"detail":"   "}"#,
        ] {
            assert_eq!(Refusal::parse(body), None);
        }
    }

    #[test]
    fn control_characters_never_reach_the_terminal() {
        let body =
            b"{\"detail\":{\"code\":\"a\\u001b[2Jb\",\"detail\":\"line\\u0007 one\\nline two\"}}";
        let refusal = Refusal::parse(body).map(|r| (r.code, r.detail));
        assert_eq!(
            refusal,
            Some((Some("a[2Jb".into()), "line oneline two".into()))
        );
        let error = Error::Http {
            status: 409,
            refusal: Refusal::parse(
                br#"{"detail":{"code":"device_not_approved","detail":"Approve it first"}}"#,
            ),
        };
        assert!(error.to_string().starts_with(
            "Hardware API returned HTTP 409 (device_not_approved): Approve it first;"
        ));
        let bare = Error::Http {
            status: 502,
            refusal: None,
        };
        assert!(
            bare.to_string()
                .starts_with("Hardware API returned HTTP 502;")
        );
    }
}
