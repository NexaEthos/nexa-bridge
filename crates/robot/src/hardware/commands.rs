//! A declared device's commands through the workspace's doors. Wiki: docs/wiki/robot.md#commands

use std::time::Duration;

use reqwest::Response;
use serde_json::Value;

use super::client::{endpoint, failure, https_client};
use super::{Error, Session};

/// The longest the workspace holds a long poll, in seconds.
pub const WAIT_MAX_SECONDS: u64 = 30;
/// How long past its wait the poll's answer may take to arrive.
const ANSWER_MARGIN: Duration = Duration::from_secs(15);
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
    use super::{command_suffix, object};
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
    fn an_acknowledgement_is_one_object() {
        assert!(object(&json!({"status": "accepted"})).is_ok());
        assert!(object(&json!(["accepted"])).is_err());
        assert!(object(&json!("accepted")).is_err());
    }
}
