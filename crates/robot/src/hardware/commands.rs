//! A declared device's commands through the workspace's doors. Wiki: docs/wiki/robot.md#commands

use std::time::Duration;

use reqwest::Response;
use reqwest::header::CONTENT_TYPE;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::client::{endpoint, failure, hex, https_client};
use super::{Error, Session};

/// The longest the workspace holds a long poll, in seconds.
pub const WAIT_MAX_SECONDS: u64 = 30;
/// How long past its wait the poll's answer may take to arrive.
const ANSWER_MARGIN: Duration = Duration::from_secs(15);
/// How long the workspace may take to answer a photo it is sent.
const PHOTO_WAIT: Duration = Duration::from_mins(2);
const COMMAND_ID_LEN: usize = 36;

fn command_suffix(command_id: &str, door: &str) -> Result<String, Error> {
    let valid = command_id.len() == COMMAND_ID_LEN
        && command_id
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '-');
    if !valid {
        return Err(Error::Contract(
            "A command id is the 36-character id the workspace gave the command",
        ));
    }
    Ok(format!("/device/commands/{command_id}{door}"))
}

fn object(body: &Value) -> Result<&Value, Error> {
    if body.is_object() {
        Ok(body)
    } else {
        Err(Error::Contract(
            "An acknowledgement or an outcome is one JSON object",
        ))
    }
}

/// The media type the bytes themselves say they are: a photo is a JPEG or a PNG.
fn photo_type(photo: &[u8]) -> Result<&'static str, Error> {
    if photo.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Ok("image/jpeg")
    } else if photo.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok("image/png")
    } else {
        Err(Error::Contract("A photo is a JPEG or a PNG file"))
    }
}

fn view_name(view: &str) -> Result<&str, Error> {
    let valid = !view.is_empty()
        && view
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if valid {
        Ok(view)
    } else {
        Err(Error::Contract(
            "A view is the name of a declared place: letters, digits, dots, underscores and hyphens",
        ))
    }
}

async fn answer(response: Response) -> Result<Value, Error> {
    if !response.status().is_success() {
        return Err(failure(response).await);
    }
    Ok(response.json().await?)
}

impl Session {
    /// Wait up to `wait` seconds for the approved commands and the stops of the accepted ones.
    /// # Errors
    /// Refuses a wait beyond the workspace's; reports transport failures and refusals.
    pub async fn poll_commands(&self, wait: u64) -> Result<Value, Error> {
        if wait > WAIT_MAX_SECONDS {
            return Err(Error::Contract(
                "The workspace holds a long poll for at most 30 seconds",
            ));
        }
        let mut url = endpoint(self, "/device/commands")?;
        url.set_query(Some(&format!("wait={wait}")));
        let response = https_client(Duration::from_secs(wait) + ANSWER_MARGIN)?
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await?;
        answer(response).await
    }

    /// Read one of the device's commands, after a reconnection, instead of replaying anything.
    /// # Errors
    /// Rejects a malformed id; reports transport failures and refusals.
    pub async fn read_command(&self, command_id: &str) -> Result<Value, Error> {
        let url = endpoint(self, &command_suffix(command_id, "")?)?;
        let response = https_client(ANSWER_MARGIN)?
            .get(url)
            .bearer_auth(&self.token)
            .send()
            .await?;
        answer(response).await
    }

    /// Send the executor's acknowledgement of a command, as it wrote it.
    /// # Errors
    /// Rejects a malformed id or body; reports transport failures and refusals.
    pub async fn acknowledge_command(
        &self,
        command_id: &str,
        body: &Value,
    ) -> Result<Value, Error> {
        self.post_command(command_id, "/ack", body).await
    }

    /// Send the executor's outcome of a command, as it wrote it.
    /// # Errors
    /// Rejects a malformed id or body; reports transport failures and refusals.
    pub async fn report_command(&self, command_id: &str, body: &Value) -> Result<Value, Error> {
        self.post_command(command_id, "/result", body).await
    }

    /// Send one photo of a running command, as the device kept it at a declared view: the bytes, their SHA-256
    /// and the view's name. The Bridge reads the photo and changes nothing in it.
    /// # Errors
    /// Rejects a malformed id or view and bytes that are neither a JPEG nor a PNG; reports transport failures
    /// and refusals.
    pub async fn publish_photo(
        &self,
        command_id: &str,
        view: &str,
        photo: Vec<u8>,
    ) -> Result<Value, Error> {
        let media = photo_type(&photo)?;
        let mut url = endpoint(self, &command_suffix(command_id, "/photos")?)?;
        url.query_pairs_mut()
            .append_pair("sha256", &hex(&Sha256::digest(&photo))?)
            .append_pair("view", view_name(view)?);
        let response = https_client(PHOTO_WAIT)?
            .post(url)
            .bearer_auth(&self.token)
            .header(CONTENT_TYPE, media)
            .body(photo)
            .send()
            .await?;
        answer(response).await
    }

    async fn post_command(
        &self,
        command_id: &str,
        door: &str,
        body: &Value,
    ) -> Result<Value, Error> {
        let url = endpoint(self, &command_suffix(command_id, door)?)?;
        let response = https_client(ANSWER_MARGIN)?
            .post(url)
            .bearer_auth(&self.token)
            .json(object(body)?)
            .send()
            .await?;
        answer(response).await
    }
}

#[cfg(test)]
mod tests {
    use super::{command_suffix, object, photo_type, view_name};
    use serde_json::json;

    #[test]
    fn a_command_id_is_the_workspaces_uuid_and_nothing_else() {
        let id = "83533b67-3d1c-4e26-b4db-cd3d26a107ac";
        assert_eq!(
            command_suffix(id, "/ack").ok().as_deref(),
            Some("/device/commands/83533b67-3d1c-4e26-b4db-cd3d26a107ac/ack")
        );
        for bad in [
            "",
            "../device",
            "83533b67-3d1c-4e26-b4db-cd3d26a107a/",
            "83533b67?wait=1",
        ] {
            assert!(command_suffix(bad, "").is_err(), "{bad}");
        }
    }

    #[test]
    fn a_photo_is_a_jpeg_or_a_png_by_its_own_bytes_and_a_view_is_a_plain_name() {
        assert_eq!(
            photo_type(&[0xFF, 0xD8, 0xFF, 0xE0]).ok(),
            Some("image/jpeg")
        );
        assert_eq!(photo_type(b"\x89PNG\r\n\x1a\n....").ok(), Some("image/png"));
        assert!(photo_type(b"GIF89a").is_err());
        assert!(photo_type(&[]).is_err());
        assert_eq!(view_name("panel-detail").ok(), Some("panel-detail"));
        for bad in ["", "panel detail", "../home", "view&x=1"] {
            assert!(view_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_acknowledgement_is_one_object() {
        assert!(object(&json!({"status": "accepted"})).is_ok());
        assert!(object(&json!(["accepted"])).is_err());
        assert!(object(&json!("accepted")).is_err());
    }
}
