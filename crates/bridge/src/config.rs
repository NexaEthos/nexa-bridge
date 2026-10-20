//! Layered config: defaults < `config.toml` < `BRIDGE_*` env. See docs/wiki/configuration.md

use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::Deserialize;

/// One model→upstream route; several sharing a `models` name form a load-balancing pool.
#[derive(Clone, Deserialize)]
pub struct UpstreamRoute {
    /// Upstream base — scheme + host ONLY, no `/v1`.
    pub url: String,
    /// Served-model-names this upstream answers for (EXACT match).
    pub models: Vec<String>,
    /// Bearer the bridge presents to THIS upstream.
    #[serde(default)]
    pub upstream_api_key: Option<String>,
    /// Rewrite the body's `model` before forwarding — one canonical name in, served id out.
    #[serde(default)]
    pub upstream_model: Option<String>,
    /// Coerce non-leading `system` messages to `user`, for strict chat templates.
    #[serde(default)]
    pub system_first: Option<bool>,
    /// This device's context budget: a larger body excludes it for that request.
    #[serde(default)]
    pub max_body_bytes: Option<usize>,
    /// This device's concurrency budget; at/over it, selection demotes it behind idler peers.
    #[serde(default)]
    pub max_inflight: Option<usize>,
}

#[derive(Clone, Deserialize)]
pub struct Config {
    /// Loopback by default; a non-loopback bind requires `api_key` or `allow_unauthenticated`.
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,

    /// Your model server: scheme + host ONLY. NO `/v1` — the full request path is forwarded.
    pub upstream_url: String,

    /// Bearer callers must present. `None` ⇒ auth disabled (see `validate_bind_security`).
    pub api_key: Option<String>,

    /// Escape hatch: permit UNAUTHENTICATED on a non-loopback bind, when the network is the boundary.
    #[serde(default)]
    pub allow_unauthenticated: bool,

    /// Bearer the bridge presents to the upstream; usually omitted (same-host / trusted LAN).
    pub upstream_api_key: Option<String>,

    /// Model→upstream routes; empty ⇒ single-upstream behaviour.
    #[serde(default)]
    pub upstreams: Vec<UpstreamRoute>,

    #[serde(default)]
    pub selected_model: Option<String>,

    /// Reported in the status payload: `server` (default) or `desktop` (set by the tray).
    #[serde(default = "default_flavor")]
    pub flavor: String,

    /// Set by a supervisor: the process drains and stops when its stdin closes.
    #[serde(default)]
    pub supervisor_pipe: bool,

    /// Set by a supervisor; `/healthz` echoes it so the supervisor knows the port is this process.
    #[serde(default)]
    pub instance_id: Option<String>,

    /// Cap on the buffered request body (the prompt). Responses always stream.
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,

    #[serde(default = "default_connect_timeout_secs")]
    pub upstream_connect_timeout_secs: u64,

    /// Per-chunk, NOT total — a whole-request timeout would strangle long generations.
    #[serde(default = "default_read_timeout_secs")]
    pub upstream_read_timeout_secs: u64,

    /// Optional global rate limit (req/s), enforced AFTER auth. `None`/`0` ⇒ no limit.
    #[serde(default)]
    pub rate_limit_per_sec: Option<u32>,

    /// Consecutive failures that trip the breaker. See docs/wiki/load-balancing.md#circuit-breaker
    #[serde(default = "default_breaker_failures")]
    pub breaker_failures: u32,

    /// How long a tripped upstream stays demoted, then half-opens.
    #[serde(default = "default_breaker_cooldown_secs")]
    pub breaker_cooldown_secs: u64,

    /// `"json"` (default) or `"text"`.
    #[serde(default = "default_log_format")]
    pub log_format: String,

    /// WHITELIST of units `/_node/logs` may tail. See docs/wiki/node-observability.md
    #[serde(default = "default_log_units")]
    pub log_units: Vec<String>,
}

fn default_listen_addr() -> String {
    "127.0.0.1:8080".to_string()
}
const fn default_max_body_bytes() -> usize {
    // 16 MiB: large-context prompts are multi-MiB JSON. Must stay below `MAX_WS_MSG_BYTES`.
    16 * 1024 * 1024
}
const fn default_connect_timeout_secs() -> u64 {
    10
}
const fn default_read_timeout_secs() -> u64 {
    900
}
const fn default_breaker_failures() -> u32 {
    2
}
const fn default_breaker_cooldown_secs() -> u64 {
    30
}
fn default_log_format() -> String {
    "json".to_string()
}
fn default_log_units() -> Vec<String> {
    vec![
        "vllm".to_string(),
        "nexa-llm-bridge".to_string(),
        "nexa-llm-connector".to_string(),
    ]
}

// Hand-written: a derived Debug would leak keys into logs and error output.
impl std::fmt::Debug for UpstreamRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamRoute")
            .field("url", &"<configured>")
            .field("models", &self.models)
            .field(
                "upstream_api_key",
                &self.upstream_api_key.as_ref().map(|_| "<redacted>"),
            )
            .field("upstream_model", &self.upstream_model)
            .field("system_first", &self.system_first)
            .field("max_body_bytes", &self.max_body_bytes)
            .field("max_inflight", &self.max_inflight)
            .finish()
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("listen_addr", &self.listen_addr)
            .field("upstream_url", &"<configured>")
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("allow_unauthenticated", &self.allow_unauthenticated)
            .field(
                "upstream_api_key",
                &self.upstream_api_key.as_ref().map(|_| "<redacted>"),
            )
            .field("upstreams", &self.upstreams)
            .field("selected_model", &self.selected_model)
            .field("flavor", &self.flavor)
            .field("supervisor_pipe", &self.supervisor_pipe)
            .field(
                "instance_id",
                &self.instance_id.as_ref().map(|_| "<configured>"),
            )
            .field("max_body_bytes", &self.max_body_bytes)
            .field(
                "upstream_connect_timeout_secs",
                &self.upstream_connect_timeout_secs,
            )
            .field(
                "upstream_read_timeout_secs",
                &self.upstream_read_timeout_secs,
            )
            .field("rate_limit_per_sec", &self.rate_limit_per_sec)
            .field("breaker_failures", &self.breaker_failures)
            .field("breaker_cooldown_secs", &self.breaker_cooldown_secs)
            .field("log_format", &self.log_format)
            .field("log_units", &self.log_units)
            .finish()
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let cfg: Self = Figment::new()
            .merge(Toml::file("config.toml"))
            .merge(Env::prefixed("BRIDGE_"))
            .extract()?;
        cfg.validate_bind_security()?;
        cfg.validate_upstreams()?;
        cfg.validate_model_selection()?;
        cfg.validate_instance_id()?;
        Ok(cfg)
    }

    fn validate_instance_id(&self) -> anyhow::Result<()> {
        if let Some(id) = &self.instance_id {
            anyhow::ensure!(
                !id.is_empty() && id.len() <= 128 && id.bytes().all(|byte| byte.is_ascii_graphic()),
                "instance_id must be visible ASCII without whitespace, at most 128 bytes"
            );
        }
        Ok(())
    }

    fn validate_model_selection(&self) -> anyhow::Result<()> {
        if let Some(model) = &self.selected_model {
            anyhow::ensure!(
                !model.trim().is_empty()
                    && model.trim() == model
                    && !model.chars().any(char::is_control),
                "selected_model must be a nonempty exact model id without control characters"
            );
            anyhow::ensure!(
                self.upstreams.iter().all(|route| route.models.len() == 1
                    && route.models[0] == *model
                    && route.upstream_model.is_none()),
                "with selected_model, every upstream route serves exactly that model, never rewritten: a pool of it"
            );
        }
        Ok(())
    }

    /// An address that doesn't parse is treated as NON-loopback — fail safe.
    pub fn listen_is_loopback(&self) -> bool {
        self.listen_addr
            .parse::<std::net::SocketAddr>()
            .is_ok_and(|a| a.ip().is_loopback())
    }

    /// Fail-closed: exposed + keyless would proxy the local model to anyone who can route to it.
    pub fn validate_bind_security(&self) -> anyhow::Result<()> {
        validate_key(self.api_key.as_deref(), "api_key")?;
        if !self.listen_is_loopback() && self.api_key.is_none() && !self.allow_unauthenticated {
            anyhow::bail!(
                "refusing to start: listen_addr `{}` is not loopback and no api_key is set — this \
                 would expose the upstream LLM UNAUTHENTICATED. Fix one of: set BRIDGE_API_KEY \
                 (e.g. `openssl rand -hex 32`); bind 127.0.0.1; or — ONLY on an already-trusted \
                 network (Tailscale/VPN) — set BRIDGE_ALLOW_UNAUTHENTICATED=true.",
                self.listen_addr
            );
        }
        Ok(())
    }

    fn validate_upstreams(&self) -> anyhow::Result<()> {
        validate_upstream(&self.upstream_url)?;
        validate_key(self.upstream_api_key.as_deref(), "upstream_api_key")?;
        for route in &self.upstreams {
            validate_upstream(&route.url)?;
            validate_key(route.upstream_api_key.as_deref(), "route upstream_api_key")?;
        }
        anyhow::ensure!(
            self.upstream_connect_timeout_secs > 0 && self.upstream_read_timeout_secs > 0,
            "upstream timeouts must be positive"
        );
        Ok(())
    }
}

fn validate_key(key: Option<&str>, field: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        key.is_none_or(|key| !key.is_empty() && key.bytes().all(|byte| byte.is_ascii_graphic())),
        "{field} must contain only visible ASCII without whitespace; omit it to disable authentication"
    );
    Ok(())
}

fn validate_upstream(value: &str) -> anyhow::Result<()> {
    let url = reqwest::Url::parse(value)?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/",
        "upstream URLs must be HTTP(S) origins without credentials, paths, queries or fragments"
    );
    Ok(())
}

fn default_flavor() -> String {
    "server".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(listen: &str, key: Option<&str>, allow: bool) -> Config {
        Config {
            listen_addr: listen.to_string(),
            upstream_url: "http://127.0.0.1:8000".to_string(),
            api_key: key.map(str::to_string),
            allow_unauthenticated: allow,
            upstream_api_key: None,
            upstreams: Vec::new(),
            selected_model: None,
            flavor: default_flavor(),
            supervisor_pipe: false,
            instance_id: None,
            max_body_bytes: default_max_body_bytes(),
            upstream_connect_timeout_secs: 10,
            upstream_read_timeout_secs: default_read_timeout_secs(),
            rate_limit_per_sec: None,
            breaker_failures: default_breaker_failures(),
            breaker_cooldown_secs: default_breaker_cooldown_secs(),
            log_format: "json".to_string(),
            log_units: default_log_units(),
        }
    }

    fn route(url: &str, models: &[&str]) -> UpstreamRoute {
        UpstreamRoute {
            url: url.into(),
            models: models.iter().map(|m| (*m).to_string()).collect(),
            upstream_api_key: None,
            upstream_model: None,
            system_first: None,
            max_body_bytes: None,
            max_inflight: None,
        }
    }

    #[test]
    fn exclusive_selection_rejects_blank_ids_and_routes_to_another_model() {
        let mut config = cfg("127.0.0.1:8080", Some("test-key"), false);
        assert!(config.validate_model_selection().is_ok());
        for model in ["", " ", "selected\n", " selected"] {
            config.selected_model = Some(model.into());
            assert!(config.validate_model_selection().is_err());
        }
        config.selected_model = Some("selected".into());
        assert!(config.validate_model_selection().is_ok());
        config.upstreams = vec![
            route("http://127.0.0.1:8000", &["selected"]),
            route("http://10.0.0.2:8000", &["selected"]),
        ];
        assert!(
            config.validate_model_selection().is_ok(),
            "a pool of the selected model"
        );
        config
            .upstreams
            .push(route("http://10.0.0.3:8000", &["other"]));
        assert!(config.validate_model_selection().is_err());
        config.upstreams.pop();
        config
            .upstreams
            .push(route("http://10.0.0.3:8000", &["selected", "other"]));
        assert!(config.validate_model_selection().is_err());
        config.upstreams.pop();
        let mut rewritten = route("http://10.0.0.3:8000", &["selected"]);
        rewritten.upstream_model = Some("other".into());
        config.upstreams.push(rewritten);
        assert!(
            config.validate_model_selection().is_err(),
            "a rewrite would serve another model under the id"
        );
    }

    #[test]
    fn loopback_keyless_is_allowed() {
        assert!(
            cfg("127.0.0.1:8080", None, false)
                .validate_bind_security()
                .is_ok()
        );
        assert!(
            cfg("[::1]:8080", None, false)
                .validate_bind_security()
                .is_ok()
        );
    }

    #[test]
    fn exposed_keyless_is_refused() {
        assert!(
            cfg("0.0.0.0:8080", None, false)
                .validate_bind_security()
                .is_err()
        );
        assert!(
            cfg("192.0.2.10:8080", None, false)
                .validate_bind_security()
                .is_err()
        );
    }

    #[test]
    fn exposed_with_key_or_optin_is_allowed() {
        assert!(
            cfg("0.0.0.0:8080", Some("k"), false)
                .validate_bind_security()
                .is_ok()
        );
        assert!(
            cfg("0.0.0.0:8080", None, true)
                .validate_bind_security()
                .is_ok()
        );
    }

    #[test]
    fn configured_empty_or_invalid_keys_are_never_keyless_optins() {
        for listen in ["127.0.0.1:8080", "0.0.0.0:8080"] {
            for key in ["", " ", "\t", "key\n", " key", "key value", "clé"] {
                for allow in [false, true] {
                    assert!(
                        cfg(listen, Some(key), allow)
                            .validate_bind_security()
                            .is_err()
                    );
                }
            }
        }
    }

    #[test]
    fn upstreams_require_credential_free_origins_and_valid_optional_keys() {
        for value in ["http://127.0.0.1:1234", "https://model.example.com/"] {
            assert!(validate_upstream(value).is_ok());
        }
        for value in [
            "file:///tmp/model",
            "http://user:key@model.example.com",
            "https://model.example.com/v1",
            "http://model.example.com?key=example",
            "http://model.example.com#fragment",
        ] {
            assert!(validate_upstream(value).is_err());
        }
        let mut config = cfg("127.0.0.1:8080", None, false);
        assert!(config.validate_upstreams().is_ok());
        config.upstream_api_key = Some(String::new());
        assert!(config.validate_upstreams().is_err());
        config.upstream_api_key = None;
        config.upstream_read_timeout_secs = 0;
        assert!(config.validate_upstreams().is_err());
    }

    /// A hostname:port doesn't parse as `SocketAddr` → fail-safe to "exposed" → needs a key.
    #[test]
    fn unparseable_addr_treated_as_exposed() {
        assert!(
            cfg("myhost:8080", None, false)
                .validate_bind_security()
                .is_err()
        );
        assert!(!cfg("myhost:8080", None, false).listen_is_loopback());
    }
}
