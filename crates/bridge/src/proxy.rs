//! Wiki: docs/wiki/proxy.md

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, CONTENT_LENGTH, HOST};
use axum::http::{HeaderMap, HeaderName, StatusCode, request::Parts};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;

use crate::app::{AppState, Target};

mod selection;

/// Rides the streamed body so the in-flight count tracks the generation, not just the connect.
struct InflightGuard(Arc<AtomicUsize>);

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// RFC 7230 §6.1 — must not be forwarded in either direction.
const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

fn is_hop_by_hop(name: &HeaderName) -> bool {
    let n = name.as_str();
    HOP_BY_HOP.iter().any(|h| n.eq_ignore_ascii_case(h))
}

/// Per-route body rewrites; `None` ⇒ forward verbatim. See docs/wiki/proxy.md#body-transforms
fn transform_request_body(
    body: &[u8],
    model_rewrite: Option<&str>,
    system_first: bool,
) -> Option<Vec<u8>> {
    if model_rewrite.is_none() && !system_first {
        return None;
    }
    let mut v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let obj = v.as_object_mut()?;
    let mut changed = if let Some(name) = model_rewrite
        && obj.contains_key("model")
    {
        obj.insert(
            "model".to_string(),
            serde_json::Value::String(name.to_string()),
        );
        true
    } else {
        false
    };
    if system_first && let Some(messages) = obj.get_mut("messages").and_then(|m| m.as_array_mut()) {
        // Strict templates accept exactly one `system`, at index 0; re-role every other one.
        for (i, msg) in messages.iter_mut().enumerate() {
            if i > 0
                && msg.get("role").and_then(|r| r.as_str()) == Some("system")
                && let Some(m) = msg.as_object_mut()
            {
                m.insert(
                    "role".to_string(),
                    serde_json::Value::String("user".to_string()),
                );
                changed = true;
            }
        }
    }
    if changed {
        serde_json::to_vec(&v).ok()
    } else {
        None
    }
}

/// Catch-all: authenticate, then forward `<upstream_url><incoming path+query>` and stream back.
pub async fn proxy(State(st): State<AppState>, req: Request) -> Response {
    if let Some(reject) = st.auth.check(req.headers()) {
        return reject;
    }
    // Post-auth, so an unauthenticated flood never consumes the budget.
    if let Some(limiter) = &st.limiter
        && limiter.check().is_err()
    {
        return (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
    }
    let (parts, body) = req.into_parts();
    let path_q = parts
        .uri
        .path_and_query()
        .map_or("/", axum::http::uri::PathAndQuery::as_str);

    // Buffered to enforce the cap, to route by `model`, and so failover can re-send it.
    let Ok(request_body) = axum::body::to_bytes(body, st.max_body_bytes).await else {
        return (StatusCode::PAYLOAD_TOO_LARGE, "request body too large").into_response();
    };

    if let Some(response) = selection::admit(&st, &parts, &request_body).await {
        return response;
    }

    // Ordered candidates; a connect error or 5xx falls over to the next. See docs/wiki/load-balancing.md
    let affinity = parts
        .headers
        .get("x-affinity-key")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty());
    let candidates = st.route_candidates(&request_body, affinity);
    let n = candidates.len();
    for (i, target) in candidates.iter().enumerate() {
        let last = i + 1 == n;
        let url = format!("{}{}", target.base.trim_end_matches('/'), path_q);
        let logged = log_target(&url);
        let counter = st.inflight.get(&target.base).cloned();
        if let Some(c) = &counter {
            c.fetch_add(1, Ordering::Relaxed);
        }

        let rb = upstream_request(&st, target, &parts, request_body.clone(), &url);

        let upstream = match rb.send().await {
            Ok(r) if r.status().is_server_error() && !last => {
                if let Some(c) = &counter {
                    c.fetch_sub(1, Ordering::Relaxed);
                }
                if mark_upstream(&st, &target.base, false) {
                    tracing::warn!(status = %r.status(), url = %logged, "upstream DOWN (5xx) — failing over; repeats at debug until recovery");
                } else {
                    tracing::debug!(status = %r.status(), url = %logged, "upstream still down (5xx) — failing over");
                }
                continue;
            }
            Ok(r) => {
                if mark_upstream(&st, &target.base, true) {
                    tracing::info!(url = %logged, "upstream recovered");
                }
                r
            }
            Err(e) if !last => {
                if let Some(c) = &counter {
                    c.fetch_sub(1, Ordering::Relaxed);
                }
                if mark_upstream(&st, &target.base, false) {
                    tracing::warn!(error = %e.without_url(), url = %logged, "upstream DOWN — failing over; repeats at debug until recovery");
                } else {
                    tracing::debug!(error = %e.without_url(), url = %logged, "upstream still down — failing over");
                }
                continue;
            }
            Err(e) => {
                if let Some(c) = &counter {
                    c.fetch_sub(1, Ordering::Relaxed);
                }
                if mark_upstream(&st, &target.base, false) {
                    tracing::warn!(error = %e.without_url(), url = %logged, "upstream DOWN (last candidate)");
                } else {
                    tracing::debug!(error = %e.without_url(), url = %logged, "upstream still down (last candidate)");
                }
                return (StatusCode::BAD_GATEWAY, "upstream unavailable").into_response();
            }
        };

        let status = upstream.status();
        let mut headers = HeaderMap::new();
        for (name, value) in upstream.headers() {
            // Content-Type passes verbatim (text/event-stream); a streamed body has no length.
            if is_hop_by_hop(name) || name == CONTENT_LENGTH {
                continue;
            }
            headers.insert(name.clone(), value.clone());
        }
        let guard = counter.map(InflightGuard);
        let body = Body::from_stream(upstream.bytes_stream().map(move |chunk| {
            let _hold = &guard;
            chunk
        }));
        return (status, headers, body).into_response();
    }
    // route_candidates always yields ≥1 candidate — unreachable.
    (StatusCode::BAD_GATEWAY, "no upstream candidate").into_response()
}

fn upstream_request(
    st: &AppState,
    target: &Target,
    parts: &Parts,
    body: Bytes,
    url: &str,
) -> reqwest::RequestBuilder {
    let mut request = st.client.request(parts.method.clone(), url);
    request =
        match transform_request_body(&body, target.model_rewrite.as_deref(), target.system_first) {
            Some(transformed) => request.body(transformed),
            None => request.body(body),
        };
    for (name, value) in &parts.headers {
        if is_hop_by_hop(name)
            || name == HOST
            || name == AUTHORIZATION
            || name == CONTENT_LENGTH
            || name.as_str() == "x-affinity-key"
        {
            continue;
        }
        request = request.header(name.clone(), value.clone());
    }
    if let Some(key) = target.api_key.as_deref() {
        request = request.bearer_auth(key);
    }
    request
}

/// Record an outcome; true when the health state CHANGED, so repeats stay quiet.
fn mark_upstream(st: &AppState, base: &str, ok: bool) -> bool {
    let Ok(mut map) = st.upstream_health.lock() else {
        return true; // poisoned lock: never suppress a log over bookkeeping
    };
    crate::app::record_observation(
        &mut map,
        base,
        ok,
        st.breaker_failures,
        st.breaker_cooldown,
        std::time::Instant::now(),
    )
}

/// Readiness: probe the upstream `/v1/models`. Liveness (`/healthz`) is a static 200.
pub async fn readyz(State(st): State<AppState>) -> Response {
    let url = format!("{}/v1/models", st.upstream_url.trim_end_matches('/'));
    let mut rb = st.client.get(&url);
    if let Some(key) = st.upstream_api_key.as_deref() {
        rb = rb.bearer_auth(key);
    }
    match rb.send().await {
        Ok(r) if r.status().is_success() => (StatusCode::OK, "ready").into_response(),
        Ok(r) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("upstream status {}", r.status()),
        )
            .into_response(),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "upstream unreachable").into_response(),
    }
}

/// The upstream base and path for the log line — never the caller's query string.
fn log_target(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

#[cfg(test)]
mod health_tests {
    use crate::app::{is_available, record_observation};
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    const COOLDOWN: Duration = Duration::from_secs(30);

    #[test]
    fn transitions_log_once_and_recover() {
        let mut m = HashMap::new();
        let now = Instant::now();
        // First-ever failure is a transition (unknown starts healthy); repeats are quiet.
        assert!(record_observation(
            &mut m, "http://a", false, 2, COOLDOWN, now
        ));
        assert!(!record_observation(
            &mut m, "http://a", false, 2, COOLDOWN, now
        ));
        assert!(!record_observation(
            &mut m, "http://a", false, 2, COOLDOWN, now
        ));
        assert!(record_observation(
            &mut m, "http://a", true, 2, COOLDOWN, now
        ));
        assert!(!record_observation(
            &mut m, "http://a", true, 2, COOLDOWN, now
        ));
        // A first-ever success is NOT a transition.
        assert!(!record_observation(
            &mut m, "http://b", true, 2, COOLDOWN, now
        ));
    }

    #[test]
    fn breaker_trips_at_threshold_and_half_opens_after_cooldown() {
        let mut m = HashMap::new();
        let now = Instant::now();
        record_observation(&mut m, "http://a", false, 2, COOLDOWN, now);
        assert!(is_available(m.get("http://a"), now));
        record_observation(&mut m, "http://a", false, 2, COOLDOWN, now);
        assert!(!is_available(m.get("http://a"), now));
        assert!(!is_available(m.get("http://a"), now + COOLDOWN / 2));
        // Cooldown expiry half-opens: available again for a natural probe.
        assert!(is_available(m.get("http://a"), now + COOLDOWN));
        let later = now + COOLDOWN + Duration::from_secs(1);
        record_observation(&mut m, "http://a", false, 2, COOLDOWN, later);
        assert!(!is_available(m.get("http://a"), later));
        let later2 = later + COOLDOWN;
        record_observation(&mut m, "http://a", true, 2, COOLDOWN, later2);
        assert!(is_available(m.get("http://a"), later2));
        assert!(is_available(m.get("http://never-seen"), now));
    }
}

#[cfg(test)]
mod log_tests {
    use super::log_target;

    #[test]
    fn the_log_line_never_carries_the_query_string() {
        assert_eq!(
            log_target("http://127.0.0.1:8000/v1/chat/completions?key=secret"),
            "http://127.0.0.1:8000/v1/chat/completions"
        );
        assert_eq!(
            log_target("http://127.0.0.1:8000/v1/models"),
            "http://127.0.0.1:8000/v1/models"
        );
    }
}
