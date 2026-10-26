//! # Wiki: docs/wiki/managed-auth.md

use anyhow::Context as _;
use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::Deserialize;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{EnvFilter, fmt};

#[derive(Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub managed_auth: bool,
    #[serde(default)]
    pub agora_url: Option<String>,
    pub relay_url: String,
    pub tunnel_token: String,
    pub upstream_url: String,
    #[serde(default = "default_reconnect_secs")]
    pub reconnect_secs: u64,
    #[serde(default = "default_reconnect_max_secs")]
    pub reconnect_max_secs: u64,
    #[serde(default)]
    pub supervisor_pipe: bool,
    #[serde(default = "default_keepalive_secs")]
    pub keepalive_secs: u64,
    #[serde(default = "default_read_timeout_secs")]
    pub upstream_read_timeout_secs: u64,
    #[serde(default = "default_log_format")]
    pub log_format: String,
    #[serde(default)]
    pub status_url: Option<String>,
    pub status_api_key: Option<String>,
    #[serde(default = "default_status_interval_secs")]
    pub status_interval_secs: u64,
}

const fn default_status_interval_secs() -> u64 {
    30
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("managed_auth", &self.managed_auth)
            .field("agora_url", &self.agora_url)
            .field("relay_url", &"<configured>")
            .field("tunnel_token", &"<redacted>")
            .field("upstream_url", &"<configured>")
            .field("reconnect_secs", &self.reconnect_secs)
            .field("reconnect_max_secs", &self.reconnect_max_secs)
            .field("supervisor_pipe", &self.supervisor_pipe)
            .field("keepalive_secs", &self.keepalive_secs)
            .field(
                "upstream_read_timeout_secs",
                &self.upstream_read_timeout_secs,
            )
            .field("log_format", &self.log_format)
            .field(
                "status_url",
                &self.status_url.as_ref().map(|_| "<configured>"),
            )
            .field(
                "status_api_key",
                &self.status_api_key.as_ref().map(|_| "<redacted>"),
            )
            .field("status_interval_secs", &self.status_interval_secs)
            .finish()
    }
}

const fn default_reconnect_secs() -> u64 {
    3
}
const fn default_reconnect_max_secs() -> u64 {
    60
}
const fn default_keepalive_secs() -> u64 {
    25
}
const fn default_read_timeout_secs() -> u64 {
    900
}
fn default_log_format() -> String {
    "json".to_string()
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let cfg: Self = Figment::new()
            .merge(Toml::file("connector.toml"))
            .merge(Env::prefixed("CONNECTOR_"))
            .extract()?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> anyhow::Result<()> {
        let relay = reqwest::Url::parse(&self.relay_url)?;
        let upstream = reqwest::Url::parse(&self.upstream_url)?;
        anyhow::ensure!(
            relay.scheme() == "wss" && relay.host_str().is_some(),
            "relay must use WSS"
        );
        anyhow::ensure!(
            matches!(upstream.scheme(), "http" | "https") && upstream.host_str().is_some(),
            "upstream must use HTTP(S)"
        );
        for url in [&relay, &upstream] {
            anyhow::ensure!(
                url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "connector URLs cannot contain credentials, queries or fragments"
            );
        }
        anyhow::ensure!(
            upstream.path() == "/",
            "upstream must be an origin without a path"
        );
        for key in std::iter::once(self.tunnel_token.as_str()).chain(self.status_api_key.as_deref())
        {
            anyhow::ensure!(
                !key.is_empty() && key.bytes().all(|byte| byte.is_ascii_graphic()),
                "connector credentials must contain visible ASCII without whitespace; omit optional keys instead of leaving them empty"
            );
        }
        anyhow::ensure!(
            self.reconnect_secs > 0
                && self.keepalive_secs > 0
                && self.upstream_read_timeout_secs > 0
                && self.status_interval_secs > 0,
            "connector intervals and timeouts must be positive"
        );
        anyhow::ensure!(
            self.reconnect_max_secs >= self.reconnect_secs,
            "reconnect_max_secs must be at least reconnect_secs"
        );
        if let Some(value) = self.status_url.as_deref().filter(|value| !value.is_empty()) {
            let url = reqwest::Url::parse(value)?;
            anyhow::ensure!(
                matches!(url.scheme(), "http" | "https")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "status URL must use HTTP(S) without credentials, queries or fragments"
            );
        }
        if !self.managed_auth {
            anyhow::ensure!(
                self.agora_url.is_none(),
                "agora_url belongs to managed authentication: a manual tunnel token never goes to Agora"
            );
            return Ok(());
        }
        let agora = self.agora_url.as_deref().context(
            "managed authentication requires agora_url, the Agora that approved this computer",
        )?;
        anyhow::ensure!(
            nexa_link::agora::origin(agora)? == agora,
            "agora_url must be a bare HTTPS origin such as https://agora.example.com"
        );
        anyhow::ensure!(
            upstream.scheme() == "http"
                && upstream.host_str().is_some_and(|host| {
                    host.trim_start_matches('[')
                        .trim_end_matches(']')
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
                }),
            "managed connector must target a loopback IP over HTTP"
        );
        anyhow::ensure!(
            self.status_url.is_none(),
            "managed status must use the local bridge endpoint"
        );
        anyhow::ensure!(
            self.status_api_key
                .as_ref()
                .is_some_and(|key| !key.trim().is_empty()),
            "managed authentication requires a locally generated bridge credential"
        );
        Ok(())
    }
}

pub fn init_tracing(cfg: &Config) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,nexa_llm_connector=info"));
    let registry = tracing_subscriber::registry().with(filter);
    if cfg.log_format == "json" {
        registry.with(fmt::layer().json()).init();
    } else {
        registry.with(fmt::layer()).init();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            managed_auth: false,
            agora_url: None,
            relay_url: "wss://relay.example.com/_tunnel".into(),
            tunnel_token: "test-token".into(),
            upstream_url: "http://127.0.0.1:8088".into(),
            reconnect_secs: 3,
            reconnect_max_secs: 60,
            supervisor_pipe: false,
            keepalive_secs: 25,
            upstream_read_timeout_secs: 900,
            log_format: "json".into(),
            status_url: None,
            status_api_key: None,
            status_interval_secs: 30,
        }
    }

    #[test]
    fn manual_mode_rejects_cleartext_relays_embedded_secrets_and_invalid_intervals() {
        assert!(config().validate().is_ok());
        for relay in [
            "ws://relay.example.com/_tunnel",
            "wss://user:key@relay.example.com/_tunnel",
            "wss://relay.example.com/_tunnel?key=example",
            "wss://relay.example.com/#fragment",
        ] {
            assert!(
                Config {
                    relay_url: relay.into(),
                    ..config()
                }
                .validate()
                .is_err()
            );
        }
        for upstream in [
            "file:///tmp/bridge",
            "http://127.0.0.1:8088/v1",
            "http://user:key@127.0.0.1:8088",
        ] {
            assert!(
                Config {
                    upstream_url: upstream.into(),
                    ..config()
                }
                .validate()
                .is_err()
            );
        }
        for token in ["", " ", "token\n", "tökén"] {
            assert!(
                Config {
                    tunnel_token: token.into(),
                    ..config()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            Config {
                keepalive_secs: 0,
                ..config()
            }
            .validate()
            .is_err()
        );
        assert!(
            Config {
                reconnect_max_secs: 2,
                ..config()
            }
            .validate()
            .is_err()
        );
        assert!(
            Config {
                status_api_key: Some(String::new()),
                ..config()
            }
            .validate()
            .is_err()
        );
        assert!(
            Config {
                status_url: Some("https://user:key@status.example.com/status".into()),
                ..config()
            }
            .validate()
            .is_err()
        );
        assert!(
            Config {
                status_url: Some(String::new()),
                ..config()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn managed_mode_requires_a_local_key_and_explicit_loopback() {
        let base = Config {
            managed_auth: true,
            agora_url: Some("https://agora.example.com".into()),
            status_api_key: Some("local-key".into()),
            ..config()
        };
        for upstream in ["http://127.0.0.1:8088", "http://[::1]:8088"] {
            assert!(
                Config {
                    upstream_url: upstream.into(),
                    ..base.clone()
                }
                .validate()
                .is_ok()
            );
        }
        for upstream in [
            "http://localhost:8088",
            "http://192.0.2.10:8088",
            "https://127.0.0.1:8088",
        ] {
            assert!(
                Config {
                    upstream_url: upstream.into(),
                    ..base.clone()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            Config {
                status_api_key: None,
                ..base.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            Config {
                status_url: Some(String::new()),
                ..base.clone()
            }
            .validate()
            .is_err()
        );
        for agora in [
            None,
            Some("https://agora.example.com/"),
            Some("http://agora.example.com"),
            Some("https://agora.example.com/app"),
        ] {
            assert!(
                Config {
                    agora_url: agora.map(Into::into),
                    ..base.clone()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            Config {
                managed_auth: false,
                status_api_key: None,
                ..base
            }
            .validate()
            .is_err(),
            "a manual tunnel token never goes to Agora"
        );
    }
}
