//! Wiki: docs/wiki/robot.md

use std::fmt::Write as _;
use std::time::Duration;

use reqwest::{Client, Response, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use super::observation::Observation;
use super::{Error, Refusal, Source};

const API: &str = "/api/v1/bridge/managed";

/// Browser approval request for one hardware endpoint.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Pairing {
    /// Server-reported browser approval URL.
    pub verification_uri: String,
    /// Server-reported approval lifetime in seconds.
    pub expires_in: u64,
    /// Server-reported polling interval in seconds.
    pub interval: u64,
    /// Server-reported approval code.
    pub code: String,
}

impl Pairing {
    /// Validate a stored approval URL before opening it again.
    /// # Errors
    /// Rejects a URL outside the explicitly trusted origin or with a mismatched code.
    pub fn approval_uri(&self, approval_origin: &str) -> Result<&str, Error> {
        validate_pairing(&origin(approval_origin)?, self)?;
        Ok(&self.verification_uri)
    }
}

/// Server-reported hardware approval identity.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Device {
    /// Server-reported identity.
    pub id: String,
    /// Server-reported status.
    pub status: String,
    #[serde(rename = "device_class")]
    /// Server-reported hardware class.
    pub class: String,
    /// Server-reported workspace slug.
    pub tenant_slug: Option<String>,
}

/// Server acknowledgement of hardware presence.
#[derive(Debug, Deserialize)]
pub struct Heartbeat {
    /// Server-reported state.
    pub state: String,
    /// Server-reported server receipt time.
    pub server_time: String,
    /// Server-reported requested heartbeat interval in seconds.
    pub next_heartbeat_seconds: u64,
    /// Server-reported presence expiry in seconds.
    pub offline_after_seconds: u64,
}

/// One event result returned by the hardware API.
#[derive(Debug, Deserialize)]
pub struct Receipt {
    /// Server-reported position in the submitted batch.
    pub index: usize,
    /// Server-reported event identifier.
    pub event_id: Option<String>,
    /// Server-reported status.
    pub status: String,
    /// Server-reported journal id.
    pub journal_id: Option<u64>,
    /// Server-reported receipt timestamp.
    pub received_at: Option<String>,
    /// Server-reported retry delay in seconds.
    pub retry_after_seconds: Option<u64>,
}

/// Durable server acknowledgement of the exact pending event.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Acknowledged {
    /// Server-reported event identifier.
    pub event_id: String,
    /// Server-reported status.
    pub status: String,
    /// Server-reported journal id.
    pub journal_id: u64,
    /// Server-reported receipt timestamp.
    pub received_at: String,
}

#[derive(Deserialize)]
struct Receipts {
    receipts: Vec<Receipt>,
}

/// A credential-bound hardware client whose debug output never contains the proof.
#[derive(Serialize, Deserialize)]
pub struct Session {
    pub(crate) origin: String,
    pub(crate) source: Source,
    pub(crate) label: String,
    pub(crate) token: String,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Session").finish_non_exhaustive()
    }
}

impl Session {
    /// Bind a previously persisted proof to the selected hardware and HTTPS API origin.
    /// # Errors
    /// Rejects invalid origins, empty labels or empty credentials.
    pub fn new(api: &str, source: Source, label: String, token: String) -> Result<Self, Error> {
        let origin = origin(api)?.to_string();
        if label.trim().is_empty() || label.chars().any(char::is_control) || token.is_empty() {
            return Err(Error::Contract(
                "Provide an asset label and persist a hardware proof before pairing",
            ));
        }
        Ok(Self {
            origin,
            source,
            label,
            token,
        })
    }

    /// Request browser approval after the caller has durably stored the proof.
    /// # Errors
    /// Reports source, HTTPS or approval-origin validation failures.
    pub async fn pair(&self, approval: &str) -> Result<Pairing, Error> {
        let (approval, body) = self.prepare_pair(approval).await?;
        self.pair_prepared(&approval, &body).await
    }

    pub(super) async fn prepare_pair(&self, approval: &str) -> Result<(Url, Value), Error> {
        let approval = origin(approval)?;
        let snapshot = self.source.capture().await?;
        let mut body = snapshot.declaration();
        body["token_sha256"] = json!(hex(&Sha256::digest(self.token.as_bytes()))?);
        body["label"] = json!(self.label);
        body["device_class"] = json!(self.source.class());
        Ok((approval, body))
    }

    pub(super) async fn pair_prepared(
        &self,
        approval: &Url,
        body: &Value,
    ) -> Result<Pairing, Error> {
        let response = client()?
            .post(endpoint(self, "/pairing")?)
            .json(body)
            .send()
            .await?;
        let pairing: Pairing = success(response).await?.json().await?;
        validate_pairing(approval, &pairing)?;
        Ok(pairing)
    }

    /// Read approval state with this hardware proof, never with an owner session.
    /// # Errors
    /// Reports a refused credential or incompatible hardware identity.
    pub async fn device(&self) -> Result<Device, Error> {
        let response = client()?
            .get(endpoint(self, "/device")?)
            .bearer_auth(&self.token)
            .send()
            .await?;
        let device: Device = success(response).await?.json().await?;
        if device.class != self.source.class() || device.id.is_empty() {
            return Err(Error::Contract(
                "The credential does not match this hardware adapter; pair it separately",
            ));
        }
        Ok(device)
    }

    /// Collect a new event; the caller must persist it before calling `deliver`.
    /// # Errors
    /// Reports approval, workspace, source or heartbeat failures.
    pub async fn collect(&self, workspace: &str) -> Result<Value, Error> {
        let device = self.device().await?;
        validate_device(&device, workspace, self.source.class())?;
        let snapshot = self.source.capture().await?;
        self.heartbeat(&snapshot).await?;
        snapshot.event(&self.label, &device.id)
    }

    async fn heartbeat(&self, snapshot: &Observation) -> Result<(), Error> {
        let mut beat = snapshot.declaration();
        beat["sent_at"] = json!(timestamp()?);
        let response = client()?
            .post(endpoint(self, "/device/heartbeat")?)
            .bearer_auth(&self.token)
            .json(&beat)
            .send()
            .await?;
        let heartbeat: Heartbeat = success(response).await?.json().await?;
        if heartbeat.state != "online"
            || OffsetDateTime::parse(&heartbeat.server_time, &Rfc3339).is_err()
            || heartbeat.next_heartbeat_seconds == 0
            || heartbeat.offline_after_seconds <= heartbeat.next_heartbeat_seconds
        {
            return Err(Error::Contract(
                "Invalid heartbeat contract; no observation was sent",
            ));
        }
        Ok(())
    }

    /// Deliver the exact persisted document and validate its matching receipt.
    /// # Errors
    /// Retain the document after every error, including a rate-limited response.
    pub async fn deliver(&self, workspace: &str, event: &Value) -> Result<Acknowledged, Error> {
        let device = self.device().await?;
        validate_device(&device, workspace, self.source.class())?;
        if event
            .pointer("/provenance/device_id")
            .and_then(Value::as_str)
            != Some(&device.id)
        {
            return Err(Error::Contract(
                "Pending observation belongs to another device; preserve it before retrying",
            ));
        }
        let event_id = event
            .get("event_id")
            .and_then(Value::as_str)
            .ok_or(Error::Contract(
                "Pending observation has no event ID; preserve it for diagnosis",
            ))?;
        let response = client()?
            .post(endpoint(self, "/device/events")?)
            .bearer_auth(&self.token)
            .json(&json!({"events": [event]}))
            .send()
            .await?;
        if !response.status().is_success()
            && response.status() != reqwest::StatusCode::TOO_MANY_REQUESTS
        {
            return Err(failure(response).await);
        }
        let receipts: Receipts = response.json().await?;
        checked_receipt(receipts.receipts, event_id)
    }
}

fn client() -> Result<Client, Error> {
    Ok(Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()?)
}

fn endpoint(state: &Session, suffix: &str) -> Result<Url, Error> {
    origin(&state.origin)?
        .join(&format!("{API}{suffix}"))
        .map_err(|_| Error::Contract("Invalid hardware API path"))
}

fn origin(value: &str) -> Result<Url, Error> {
    let url = Url::parse(value).map_err(|_| {
        Error::Contract("Use an HTTPS Agora origin without credentials or an API path")
    })?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(Error::Contract(
            "Use an HTTPS Agora origin without credentials, query, fragment or an API path",
        ));
    }
    Ok(url)
}

fn validate_pairing(issuer: &Url, pairing: &Pairing) -> Result<(), Error> {
    let uri = Url::parse(&pairing.verification_uri)
        .map_err(|_| Error::Contract("Invalid pairing approval URL"))?;
    if uri.origin() != issuer.origin()
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.query().is_some()
        || uri.path() != "/bridge-pair"
        || uri.fragment() != Some(pairing.code.as_str())
        || pairing.code.is_empty()
        || pairing.expires_in == 0
        || pairing.interval == 0
    {
        return Err(Error::Contract(
            "Pairing approval must belong to the explicitly configured approval origin; check both origins before retrying",
        ));
    }
    Ok(())
}

fn validate_device(
    device: &Device,
    expected_workspace: &str,
    expected_class: &str,
) -> Result<(), Error> {
    if device.status != "active" {
        return Err(Error::Contract(
            "Approve this hardware device in Agora before sending observations; revoked devices must be paired again",
        ));
    }
    if device.class != expected_class || device.id.is_empty() {
        return Err(Error::Contract(
            "The credential does not match this hardware adapter; use its separate hardware pairing",
        ));
    }
    if expected_workspace.is_empty() || device.tenant_slug.as_deref() != Some(expected_workspace) {
        return Err(Error::Contract(
            "Approved workspace differs from the requested workspace; verify the pairing in Agora",
        ));
    }
    Ok(())
}

async fn success(response: Response) -> Result<Response, Error> {
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(failure(response).await)
    }
}

async fn failure(response: Response) -> Error {
    let status = response.status().as_u16();
    if status == 429 {
        return response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .map_or_else(|| Error::Contract("Rate-limited response lacks Retry-After; retain the event and inspect the server response"), Error::RateLimited);
    }
    let refusal = response
        .bytes()
        .await
        .ok()
        .and_then(|body| Refusal::parse(&body));
    Error::Http { status, refusal }
}

fn checked_receipt(mut receipts: Vec<Receipt>, event_id: &str) -> Result<Acknowledged, Error> {
    if receipts.len() != 1 {
        return Err(Error::Contract(
            "Expected one receipt; pending observation retained",
        ));
    }
    let receipt = receipts
        .pop()
        .ok_or(Error::Contract("Missing event receipt"))?;
    if receipt.index != 0 || receipt.event_id.as_deref() != Some(event_id) {
        return Err(Error::Contract(
            "Receipt does not match the pending event; pending observation retained",
        ));
    }
    match receipt.status.as_str() {
        "stored" | "duplicate"
            if receipt.journal_id.is_some_and(|id| id > 0)
                && receipt
                    .received_at
                    .as_ref()
                    .is_some_and(|time| OffsetDateTime::parse(time, &Rfc3339).is_ok()) =>
        {
            Ok(Acknowledged {
                event_id: event_id.into(),
                status: receipt.status,
                journal_id: receipt
                    .journal_id
                    .ok_or(Error::Contract("Receipt has no journal ID"))?,
                received_at: receipt
                    .received_at
                    .ok_or(Error::Contract("Receipt has no timestamp"))?,
            })
        }
        "rate_limited" => Err(Error::RateLimited(receipt.retry_after_seconds.ok_or(
            Error::Contract("Rate limit receipt lacks retry delay; pending observation retained"),
        )?)),
        "conflict" => Err(Error::Contract(
            "Event ID conflicts with a stored document; pending observation retained for diagnosis",
        )),
        "invalid" => Err(Error::Contract(
            "Server rejected the observation; pending observation retained for diagnosis",
        )),
        _ => Err(Error::Contract(
            "Unsupported event receipt; pending observation retained",
        )),
    }
}

pub(super) fn timestamp() -> Result<String, Error> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| Error::Contract("Host clock cannot form an RFC3339 timestamp"))
}

pub(super) fn random_id() -> Result<String, Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| Error::Contract("Operating-system randomness is unavailable"))?;
    hex(&bytes)
}

fn hex(bytes: &[u8]) -> Result<String, Error> {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut text, "{byte:02x}")
            .map_err(|_| Error::Contract("Could not encode hardware identity"))?;
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_cannot_redirect_the_operator_to_another_origin() -> Result<(), Error> {
        let issuer = origin("https://agora.example.com")?;
        for value in [
            "http://agora.example.com",
            "https://user@agora.example.com",
            "https://agora.example.com/api",
            "https://agora.example.com/?key=secret",
            "https://agora.example.com/#fragment",
        ] {
            assert!(origin(value).is_err());
        }
        let mut pairing = Pairing {
            verification_uri: "https://agora.example.com/bridge-pair#code".into(),
            code: "code".into(),
            expires_in: 600,
            interval: 5,
        };
        validate_pairing(&issuer, &pairing)?;
        assert!(validate_pairing(&origin("https://api.example.com")?, &pairing).is_err());
        for uri in [
            "https://other.example.com/bridge-pair#code",
            "https://agora.example.com/bridge-pair#wrong",
            "https://agora.example.com/other#code",
            "https://user@agora.example.com/bridge-pair#code",
        ] {
            pairing.verification_uri = uri.into();
            assert!(validate_pairing(&issuer, &pairing).is_err());
        }
        Ok(())
    }

    #[test]
    fn inference_credentials_and_wrong_workspaces_cannot_send_board_observations()
    -> Result<(), Error> {
        let mut device = Device {
            id: "device".into(),
            status: "active".into(),
            class: "llm_node".into(),
            tenant_slug: Some("test".into()),
        };
        assert!(validate_device(&device, "test", "lab_board").is_err());
        device.class = "lab_board".into();
        assert!(validate_device(&device, "different", "lab_board").is_err());
        assert!(validate_device(&device, "test", "robot").is_err());
        validate_device(&device, "test", "lab_board")?;
        device.class = "robot".into();
        validate_device(&device, "test", "robot")?;
        device.status = "revoked".into();
        assert!(validate_device(&device, "test", "robot").is_err());
        Ok(())
    }

    #[test]
    fn only_matching_persisted_receipts_acknowledge_an_event() -> Result<(), Error> {
        for status in ["stored", "duplicate"] {
            let receipt: Receipt = serde_json::from_value(
                json!({"index":0,"event_id":"event", "status":status,"journal_id":1,"received_at":"2026-01-01T00:00:00Z"}),
            )?;
            assert_eq!(checked_receipt(vec![receipt], "event")?.event_id, "event");
        }
        for value in [
            json!({"index":1,"event_id":"event","status":"stored","journal_id":1,"received_at":"time"}),
            json!({"index":0,"event_id":"other","status":"stored","journal_id":1,"received_at":"time"}),
            json!({"index":0,"event_id":"event","status":"stored"}),
            json!({"index":0,"event_id":"event","status":"stored","journal_id":1,"received_at":"not-a-timestamp"}),
            json!({"index":0,"event_id":"event","status":"stored","journal_id":0,"received_at":"2026-01-01T00:00:00Z"}),
            json!({"index":0,"event_id":"event","status":"conflict"}),
            json!({"index":0,"event_id":"event","status":"invalid"}),
            json!({"index":0,"event_id":"event","status":"future"}),
        ] {
            let receipt: Receipt = serde_json::from_value(value)?;
            assert!(checked_receipt(vec![receipt], "event").is_err());
        }
        let receipt: Receipt = serde_json::from_value(
            json!({"index":0,"event_id":"event","status":"rate_limited","retry_after_seconds":12}),
        )?;
        assert!(matches!(
            checked_receipt(vec![receipt], "event"),
            Err(Error::RateLimited(12))
        ));
        assert!(checked_receipt(Vec::new(), "event").is_err());
        Ok(())
    }
}
