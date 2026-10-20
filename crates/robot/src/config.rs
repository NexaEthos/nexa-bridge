//! Layered config: defaults < `robot.toml` < `ROBOT_*` env. See docs/wiki/robot.md

use figment::Figment;
use figment::providers::{Env, Format, Toml};
use serde::Deserialize;

/// Parsed robotics topic-governance and telemetry settings.
#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    /// Zenoh router to connect to, e.g. `tcp/192.0.2.2:7447`. The device dials OUT.
    #[serde(default)]
    pub zenoh_endpoint: String,
    /// Topic allowlist (exact names or `/prefix/*`). EMPTY = serve nothing, by design.
    #[serde(default)]
    pub topics: Vec<String>,
    /// Foxglove ws-protocol listen address for teleobservation.
    #[serde(default = "default_foxglove_listen")]
    pub foxglove_listen: String,
    #[serde(default = "default_log_format")]
    /// Structured JSON or human-readable text logging.
    pub log_format: String,
}

fn default_foxglove_listen() -> String {
    "127.0.0.1:8765".to_string()
}

fn default_log_format() -> String {
    "json".to_string()
}

impl Config {
    #[doc = "# Errors\nSee the [configuration reference](../../../docs/wiki/configuration.md#robot)."]
    pub fn load() -> anyhow::Result<Self> {
        Ok(Figment::new()
            .merge(Toml::file("robot.toml"))
            .merge(Env::prefixed("ROBOT_"))
            .extract()?)
    }
}
