//! Live serving status — the truth the tunnel reports up (design: bridge P1/P4-bis).
//!
//! A background poller interrogates every upstream's `/v1/models` (~30s) and keeps one
//! [`BridgeStatus`] snapshot: models WITH their load-time facts where the runtime reports
//! them (vLLM `max_model_len`; task from speaches-style entries), per-upstream health.
//! `GET /status` serves the snapshot — honoring the bridge auth when configured, so the
//! endpoint never reveals more than `/v1/models` would.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use nexa_tunnel_protocol::{BridgeStatus, ModelStatus, UpstreamStatus};
use tokio::sync::RwLock;

use crate::app::AppState;

pub const POLL_INTERVAL: Duration = Duration::from_secs(30);

pub type StatusCache = Arc<RwLock<BridgeStatus>>;

pub fn empty_status(flavor: &str) -> BridgeStatus {
    BridgeStatus {
        schema: BridgeStatus::SCHEMA,
        bridge_version: env!("CARGO_PKG_VERSION").to_string(),
        flavor: flavor.to_string(),
        models: Vec::new(),
        upstreams: Vec::new(),
        build: Some(nexa_tunnel_protocol::build_info!()),
    }
}

/// Parse one `/v1/models` body into model rows, keeping the optional serving facts.
fn parse_models(body: &serde_json::Value) -> Vec<ModelStatus> {
    let Some(data) = body.get("data").and_then(|d| d.as_array()) else {
        return Vec::new();
    };
    data.iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_string();
            Some(ModelStatus {
                id,
                task: m.get("task").and_then(|v| v.as_str()).map(str::to_string),
                context_length: m
                    .get("max_model_len")
                    .or_else(|| m.get("context_length"))
                    .and_then(serde_json::Value::as_u64),
                quantization: m
                    .get("quantization")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            })
        })
        .collect()
}

/// One model server's row: the probe's answer, the requests in flight to it, its ceiling and its breaker.
fn upstream_status(st: &AppState, url: String, healthy: bool) -> UpstreamStatus {
    let inflight = st
        .inflight
        .get(&url)
        .map(|count| count.load(std::sync::atomic::Ordering::Relaxed));
    let max_inflight = st
        .upstreams
        .iter()
        .find(|route| route.url == url)
        .and_then(|route| route.max_inflight);
    let now = std::time::Instant::now();
    let circuit_open = st.upstream_health.lock().ok().map(|health| {
        health
            .get(&url)
            .and_then(|h| h.unhealthy_until)
            .is_some_and(|until| now < until)
    });
    UpstreamStatus {
        url,
        healthy,
        inflight,
        max_inflight,
        circuit_open,
    }
}

async fn poll_once(st: &AppState, flavor: &str) -> BridgeStatus {
    let mut targets: Vec<(String, Option<String>)> =
        vec![(st.upstream_url.clone(), st.upstream_api_key.clone())];
    for route in &st.upstreams {
        if !targets.iter().any(|(u, _)| u == &route.url) {
            targets.push((route.url.clone(), route.upstream_api_key.clone()));
        }
    }
    let mut status = empty_status(flavor);
    for (url, key) in targets {
        let mut req = st
            .client
            .get(format!("{}/v1/models", url.trim_end_matches('/')));
        if let Some(k) = &key {
            req = req.bearer_auth(k);
        }
        let healthy = match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    for model in parse_models(&body) {
                        if !status.models.iter().any(|m| m.id == model.id) {
                            status.models.push(model);
                        }
                    }
                }
                true
            }
            _ => false,
        };
        status.upstreams.push(upstream_status(st, url, healthy));
    }
    if let Some(selected) = &st.selected_model {
        status.models.retain(|model| &model.id == selected);
    }
    status
}

/// Spawn the poller: refreshes the state-held cache the `/status` route serves.
pub fn spawn_poller(st: AppState) {
    tokio::spawn(async move {
        loop {
            let fresh = poll_once(&st, &st.flavor).await;
            *st.status_cache.write().await = fresh;
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}

/// `GET /status` — the snapshot, behind the same auth as the proxy when configured.
pub async fn status(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(reject) = st.auth.check(&headers) {
        return reject;
    }
    Json(st.status_cache.read().await.clone()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_models_with_optional_serving_facts() {
        let body = serde_json::json!({"data": [
            {"id": "qwen3.8-27b", "max_model_len": 32768},
            {"id": "kokoro", "task": "text-to-speech"},
            {"id": "bare"},
        ]});
        let models = parse_models(&body);
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].context_length, Some(32768));
        assert_eq!(models[1].task.as_deref(), Some("text-to-speech"));
        assert!(models[2].task.is_none() && models[2].context_length.is_none());
    }

    #[test]
    fn empty_status_carries_version_and_flavor() {
        let s = empty_status("desktop");
        assert_eq!(s.schema, BridgeStatus::SCHEMA);
        assert_eq!(s.flavor, "desktop");
        assert_ne!(s.bridge_version, "");
    }
}
