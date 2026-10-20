//! `nexa-bridge pool`: more model servers behind this bridge for the selected model. Wiki: docs/wiki/server-setup.md#pool

use std::fmt::Write as _;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use nexa_tunnel_protocol::UpstreamStatus;

use super::store::{self, Paths, Server};

/// Another model server serving the selected model, beside this server's own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_inflight: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_body_bytes: Option<usize>,
}

/// The members added to this server's pool; none when it serves alone.
pub fn load(paths: &Paths) -> Result<Vec<Member>> {
    store::read_private(&paths.pool())?.map_or_else(
        || Ok(Vec::new()),
        |json| {
            serde_json::from_str(&json)
                .context("pool.json is invalid; repair it or remove the members again")
        },
    )
}

fn save(paths: &Paths, members: &[Member]) -> Result<()> {
    store::write_private(&paths.pool(), &serde_json::to_string_pretty(members)?)
}

/// A model server's base address: http or https, no path, no `/v1`, no trailing slash.
pub fn base_url(raw: &str) -> Result<String> {
    let url = raw.trim().trim_end_matches('/');
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .context("give the model server as http://host:port or https://host:port")?;
    anyhow::ensure!(
        !rest.is_empty() && !rest.contains('/') && !url.chars().any(char::is_control),
        "give the model server's base address only, without a path: http://host:port"
    );
    Ok(url.to_string())
}

fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The bridge's `config.toml` for a pool: this server's model server and every member, one route each.
pub fn render(server: &Server, members: &[Member]) -> Option<String> {
    if members.is_empty() {
        return None;
    }
    let model = toml_string(&server.selected_model);
    let mut text = format!(
        "# Written by `nexa-bridge pool`: the model servers that serve {model}. Change it with the pool commands.\n"
    );
    let own = Member {
        url: server.upstream_url.clone(),
        ..Member::default()
    };
    for member in std::iter::once(&own).chain(members) {
        let _ = write!(
            text,
            "\n[[upstreams]]\nurl = {}\nmodels = [{model}]\n",
            toml_string(&member.url)
        );
        if let Some(key) = &member.api_key {
            let _ = writeln!(text, "upstream_api_key = {}", toml_string(key));
        }
        if let Some(n) = member.max_inflight {
            let _ = writeln!(text, "max_inflight = {n}");
        }
        if let Some(n) = member.max_body_bytes {
            let _ = writeln!(text, "max_body_bytes = {n}");
        }
    }
    Some(text)
}

/// Write the bridge's pool file, or remove it when this server serves alone.
pub fn write_config(paths: &Paths, server: &Server, members: &[Member]) -> Result<()> {
    render(server, members).map_or_else(
        || match std::fs::remove_file(paths.bridge_config()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).context("cannot remove the bridge's pool file")
            }
            _ => Ok(()),
        },
        |text| store::write_private(&paths.bridge_config(), &text),
    )
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

/// The model ids a model server lists, or `None` when nothing answers there now.
pub async fn served(url: &str, api_key: Option<&str>) -> Result<Option<Vec<String>>> {
    let mut request = client()?.get(format!("{url}/v1/models"));
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let Ok(response) = request.send().await else {
        return Ok(None);
    };
    anyhow::ensure!(
        response.status() != reqwest::StatusCode::UNAUTHORIZED,
        "{url} refused the key: give the one its server was started with (--key)"
    );
    if !response.status().is_success() {
        return Ok(None);
    }
    let listing: serde_json::Value = response
        .json()
        .await
        .context("the model list is not JSON")?;
    Ok(Some(
        listing["data"]
            .as_array()
            .map(|models| {
                models
                    .iter()
                    .filter_map(|m| m["id"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    ))
}

/// Each model server as the running bridge sees it: answering or not, its load and its circuit breaker.
pub async fn health(server: &Server, local_key: &str) -> Result<Vec<UpstreamStatus>> {
    let status: serde_json::Value = client()?
        .get(format!("http://{}/status", server.listen_addr))
        .bearer_auth(local_key)
        .send()
        .await
        .context("the bridge does not answer: is its service running?")?
        .json()
        .await?;
    Ok(serde_json::from_value(status["upstreams"].clone()).unwrap_or_default())
}

/// One model server's line: whether it answers, the requests in flight against its ceiling, an open breaker.
pub fn describe(row: Option<&UpstreamStatus>) -> String {
    let Some(row) = row else {
        return "not known yet".to_string();
    };
    let mut text = if row.healthy {
        "answering"
    } else {
        "NOT answering"
    }
    .to_string();
    match (row.inflight, row.max_inflight) {
        (Some(now), Some(max)) => {
            let _ = write!(text, " · {now}/{max} in flight");
        }
        (Some(now), None) => {
            let _ = write!(text, " · {now} in flight");
        }
        _ => {}
    }
    if row.circuit_open == Some(true) {
        text.push_str(" · set aside after failures (circuit open)");
    }
    text
}

/// Add a member, or replace the one at the same address; the caller restarts the services.
pub fn add(paths: &Paths, server: &Server, member: Member) -> Result<Vec<Member>> {
    anyhow::ensure!(
        member.url != server.upstream_url,
        "{} is this server's own model server: it is in the pool already",
        member.url
    );
    let mut members = load(paths)?;
    members.retain(|m| m.url != member.url);
    members.push(member);
    save(paths, &members)?;
    write_config(paths, server, &members)?;
    Ok(members)
}

/// Remove the member at an address; the caller restarts the services.
pub fn remove(paths: &Paths, server: &Server, url: &str) -> Result<Vec<Member>> {
    let mut members = load(paths)?;
    let before = members.len();
    members.retain(|m| m.url != url);
    anyhow::ensure!(members.len() < before, "{url} is not in this server's pool");
    save(paths, &members)?;
    write_config(paths, server, &members)?;
    Ok(members)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        Server {
            upstream_url: "http://127.0.0.1:8000".into(),
            selected_model: "gemma-4-31B-it".into(),
            ..Server::default()
        }
    }

    #[test]
    fn a_pool_renders_one_route_per_model_server_for_the_selected_model() -> Result<()> {
        assert_eq!(
            render(&server(), &[]),
            None,
            "alone, the bridge needs no pool file"
        );
        let members = [Member {
            url: "http://192.168.1.21:8000".into(),
            api_key: Some("k\"ey".into()),
            max_inflight: Some(16),
            max_body_bytes: None,
        }];
        let text = render(&server(), &members).context("a pool renders")?;
        assert_eq!(text.matches("[[upstreams]]").count(), 2);
        assert!(text.contains("url = \"http://127.0.0.1:8000\"\nmodels = [\"gemma-4-31B-it\"]"));
        assert!(text.contains("upstream_api_key = \"k\\\"ey\""));
        assert!(text.contains("max_inflight = 16"));
        Ok(())
    }

    #[test]
    fn a_model_server_line_says_its_answer_its_load_and_its_breaker() {
        let row = |healthy, inflight, max_inflight, circuit_open| UpstreamStatus {
            url: "http://192.168.1.21:8000".into(),
            healthy,
            inflight,
            max_inflight,
            circuit_open,
        };
        assert_eq!(describe(None), "not known yet");
        assert_eq!(
            describe(Some(&row(true, Some(3), Some(16), Some(false)))),
            "answering · 3/16 in flight"
        );
        assert_eq!(
            describe(Some(&row(false, Some(0), None, Some(true)))),
            "NOT answering · 0 in flight · set aside after failures (circuit open)"
        );
        assert_eq!(
            describe(Some(&row(true, None, None, None))),
            "answering",
            "an older bridge reports no load"
        );
    }

    #[test]
    fn a_member_is_a_base_address() -> Result<()> {
        assert_eq!(
            base_url("http://192.168.1.21:8000/")?,
            "http://192.168.1.21:8000"
        );
        assert!(base_url("http://192.168.1.21:8000/v1").is_err());
        assert!(base_url("192.168.1.21:8000").is_err());
        Ok(())
    }
}
