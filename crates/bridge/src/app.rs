//! Shared state + router wiring. See docs/wiki/load-balancing.md

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::IntoResponse;
use axum::routing::get;
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};

use crate::auth::Auth;
use crate::config::{Config, UpstreamRoute};
use crate::proxy;

#[derive(Clone)]
pub struct AppState {
    /// One shared client (Arc internally) so the upstream connection pool is reused.
    pub client: reqwest::Client,
    /// Fallback upstream, used when no `upstreams` route matches the request's `model`.
    pub upstream_url: String,
    pub upstream_api_key: Option<String>,
    pub upstreams: Vec<UpstreamRoute>,
    pub selected_model: Option<String>,
    pub auth: Auth,
    pub max_body_bytes: usize,
    /// Optional global GCRA limiter; `Arc` so every per-request clone shares one bucket.
    pub limiter: Option<Arc<DefaultDirectRateLimiter>>,
    /// Round-robin cursor shared across requests — the least-connections tie-break.
    pub rr: Arc<AtomicUsize>,
    /// Per-upstream in-flight counters (by base URL) driving least-connections + saturation.
    pub inflight: Arc<HashMap<String, Arc<AtomicUsize>>>,
    /// Per-upstream health, keyed by base URL: state-change logging + the circuit breaker.
    pub upstream_health: Arc<std::sync::Mutex<HashMap<String, UpstreamHealth>>>,
    pub breaker_failures: u32,
    pub breaker_cooldown: Duration,
    /// Units `/_node/logs` may tail — a whitelist. See docs/wiki/node-observability.md
    pub log_units: Vec<String>,
    /// ~2h ring of sampled GPU/CPU trend points for `/_node/stats`.
    pub node_history: crate::node::NodeHistory,
    /// Live serving snapshot the poller refreshes and `/status` serves.
    pub status_cache: crate::status::StatusCache,
    /// `server` | `desktop` — echoed into the status payload.
    pub flavor: String,
    /// A supervisor's token for this process, echoed by `/healthz`. See docs/wiki/desktop.md#sidecar-lifecycle
    pub instance_id: Option<String>,
}

impl AppState {
    pub fn new(cfg: &Config) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(cfg.upstream_connect_timeout_secs))
            // Per-chunk, NOT whole-request: long generations stream for minutes.
            .read_timeout(Duration::from_secs(cfg.upstream_read_timeout_secs))
            .tcp_nodelay(true)
            .build()?;
        if cfg.api_key.is_none() {
            // `Config::load` already refused the dangerous combo, so this is loopback or an opt-in.
            if cfg.listen_is_loopback() {
                tracing::warn!(
                    "api_key not set — bridge is UNAUTHENTICATED but bound to loopback \
                     (dev / relay-colocated); only local processes can reach it"
                );
            } else {
                tracing::warn!(
                    "api_key not set and allow_unauthenticated=true — bridge is UNAUTHENTICATED on \
                     a NON-loopback bind; the network (tailnet/VPN) MUST be the boundary"
                );
            }
        }
        let limiter = cfg
            .rate_limit_per_sec
            .and_then(std::num::NonZeroU32::new)
            .map(|n| Arc::new(RateLimiter::direct(Quota::per_second(n))));
        if let Some(n) = cfg.rate_limit_per_sec.filter(|n| *n > 0) {
            tracing::info!(rate_limit_per_sec = n, "global rate limit enabled");
        }
        let mut inflight: HashMap<String, Arc<AtomicUsize>> = HashMap::new();
        for route in &cfg.upstreams {
            inflight
                .entry(route.url.clone())
                .or_insert_with(|| Arc::new(AtomicUsize::new(0)));
        }
        inflight
            .entry(cfg.upstream_url.clone())
            .or_insert_with(|| Arc::new(AtomicUsize::new(0)));
        Ok(Self {
            client,
            upstream_url: cfg.upstream_url.clone(),
            upstream_api_key: cfg.upstream_api_key.clone(),
            upstreams: cfg.upstreams.clone(),
            selected_model: cfg.selected_model.clone(),
            auth: Auth::new(cfg.api_key.as_deref()),
            max_body_bytes: cfg.max_body_bytes,
            limiter,
            rr: Arc::new(AtomicUsize::new(0)),
            inflight: Arc::new(inflight),
            upstream_health: Arc::new(std::sync::Mutex::new(HashMap::new())),
            breaker_failures: cfg.breaker_failures,
            breaker_cooldown: Duration::from_secs(cfg.breaker_cooldown_secs),
            log_units: cfg.log_units.clone(),
            node_history: crate::node::NodeHistory::default(),
            status_cache: std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::status::empty_status(&cfg.flavor),
            )),
            flavor: cfg.flavor.clone(),
            instance_id: cfg.instance_id.clone(),
        })
    }

    /// Ordered upstream candidates for this request. See docs/wiki/load-balancing.md
    pub fn route_candidates(&self, body: &[u8], affinity: Option<&str>) -> Vec<Target> {
        // A poisoned lock fails OPEN: routing everywhere beats routing nowhere.
        let now = Instant::now();
        let unavailable: HashSet<String> = self.upstream_health.lock().map_or_else(
            |_| HashSet::new(),
            |map| {
                self.upstreams
                    .iter()
                    .map(|r| &r.url)
                    .filter(|url| !is_available(map.get(url.as_str()), now))
                    .cloned()
                    .collect()
            },
        );
        select_candidates(
            &self.upstreams,
            &self.rr,
            &self.inflight,
            &unavailable,
            (&self.upstream_url, self.upstream_api_key.as_deref()),
            body,
            affinity,
        )
    }
}

/// Per-upstream breaker state — cross-request failure memory feeding candidate ordering.
#[derive(Default)]
pub struct UpstreamHealth {
    pub consecutive_failures: u32,
    /// While `now < unhealthy_until`, selection demotes this upstream.
    pub unhealthy_until: Option<Instant>,
}

/// Record one outcome; returns true when the log state changed. See docs/wiki/load-balancing.md#circuit-breaker
pub fn record_observation(
    map: &mut HashMap<String, UpstreamHealth>,
    base: &str,
    ok: bool,
    threshold: u32,
    cooldown: Duration,
    now: Instant,
) -> bool {
    let h = map.entry(base.to_string()).or_default();
    let was_failing = h.consecutive_failures > 0;
    if ok {
        h.consecutive_failures = 0;
        h.unhealthy_until = None;
        was_failing
    } else {
        h.consecutive_failures = h.consecutive_failures.saturating_add(1);
        if h.consecutive_failures >= threshold {
            h.unhealthy_until = Some(now + cooldown);
        }
        !was_failing
    }
}

/// False only INSIDE a breaker cooldown; an unobserved upstream is assumed healthy.
pub fn is_available(state: Option<&UpstreamHealth>, now: Instant) -> bool {
    state
        .and_then(|s| s.unhealthy_until)
        .is_none_or(|until| now >= until)
}

/// Inline so the affinity mapping is stable forever — std's hasher makes no such guarantee.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Rendezvous (highest-random-weight) score for one (key, upstream) pair.
fn rendezvous_score(key: &str, url: &str) -> u64 {
    let mut buf = Vec::with_capacity(key.len() + 1 + url.len());
    buf.extend_from_slice(key.as_bytes());
    buf.push(0);
    buf.extend_from_slice(url.as_bytes());
    fnv1a64(&buf)
}

/// Pure pool selection; see docs/wiki/load-balancing.md.
fn select_candidates(
    upstreams: &[UpstreamRoute],
    rr: &AtomicUsize,
    inflight: &HashMap<String, Arc<AtomicUsize>>,
    unavailable: &HashSet<String>,
    (default_url, default_api_key): (&str, Option<&str>),
    body: &[u8],
    affinity: Option<&str>,
) -> Vec<Target> {
    if !upstreams.is_empty() {
        #[derive(serde::Deserialize)]
        struct ModelField {
            model: Option<String>,
        }
        if let Ok(ModelField { model: Some(model) }) = serde_json::from_slice::<ModelField>(body) {
            let matching: Vec<&UpstreamRoute> = upstreams
                .iter()
                .filter(|r| r.models.iter().any(|m| m == &model))
                .collect();
            if !matching.is_empty() {
                // Context fit is the only hard exclusion; if nothing fits, try everything.
                let fitting: Vec<&UpstreamRoute> = matching
                    .iter()
                    .copied()
                    .filter(|r| r.max_body_bytes.is_none_or(|cap| body.len() <= cap))
                    .collect();
                let pool = if fitting.is_empty() {
                    &matching
                } else {
                    &fitting
                };
                let mut rotated: Vec<&UpstreamRoute> = affinity.map_or_else(
                    || {
                        // Round-robin base order, then a STABLE sort by load → least-connections.
                        let n = pool.len();
                        let start = rr.fetch_add(1, Ordering::Relaxed);
                        let mut r: Vec<&UpstreamRoute> =
                            (0..n).map(|i| pool[start.wrapping_add(i) % n]).collect();
                        r.sort_by_key(|route| {
                            inflight
                                .get(&route.url)
                                .map_or(0, |c| c.load(Ordering::Relaxed))
                        });
                        r
                    },
                    |key| {
                        let mut scored: Vec<&UpstreamRoute> = pool.clone();
                        scored.sort_by_key(|r| std::cmp::Reverse(rendezvous_score(key, &r.url)));
                        scored
                    },
                );
                // Stable band sort: [under-cap] → [saturated] → [breaker]. Demote, never exclude.
                rotated.sort_by_key(|r| {
                    if unavailable.contains(&r.url) {
                        return 2_u8;
                    }
                    let saturated = r.max_inflight.is_some_and(|cap| {
                        inflight
                            .get(&r.url)
                            .map_or(0, |c| c.load(Ordering::Relaxed))
                            >= cap
                    });
                    u8::from(saturated)
                });
                return rotated
                    .into_iter()
                    .map(|r| Target {
                        base: r.url.clone(),
                        api_key: r.upstream_api_key.clone(),
                        model_rewrite: r.upstream_model.clone(),
                        system_first: r.system_first.unwrap_or(false),
                    })
                    .collect();
            }
        }
    }
    vec![Target {
        base: default_url.to_string(),
        api_key: default_api_key.map(str::to_string),
        model_rewrite: None,
        system_first: false,
    }]
}

/// One resolved upstream for a request.
pub struct Target {
    pub base: String,
    pub api_key: Option<String>,
    pub model_rewrite: Option<String>,
    pub system_first: bool,
}

/// Liveness, plus the instance id a supervisor gave this process — so it can tell the port is ours.
async fn healthz(State(st): State<AppState>) -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    if let Some(id) = st
        .instance_id
        .as_deref()
        .and_then(|id| HeaderValue::from_str(id).ok())
    {
        headers.insert("x-nexa-instance", id);
    }
    (headers, "ok")
}

pub fn router(state: AppState) -> Router {
    use tower_http::trace::TraceLayer;

    Router::new()
        // Liveness + readiness are unauthenticated probes, handled before the catch-all.
        .route("/healthz", get(healthz))
        .route("/readyz", get(proxy::readyz))
        .route("/status", get(crate::status::status))
        .route("/_node/stats", get(crate::node::stats))
        .route("/_node/logs", get(crate::node::logs))
        .fallback(proxy::proxy)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Quota::per_second(n)` admits a burst of `n`, then trips — what `AppState::new` relies on.
    #[test]
    fn limiter_admits_burst_then_trips() {
        let n = std::num::NonZeroU32::MIN.saturating_add(1);
        let limiter = RateLimiter::direct(Quota::per_second(n));
        assert!(limiter.check().is_ok());
        assert!(limiter.check().is_ok());
        assert!(limiter.check().is_err());
    }

    fn route(url: &str, model: &str, cap: Option<usize>) -> UpstreamRoute {
        UpstreamRoute {
            url: url.into(),
            models: vec![model.into()],
            upstream_api_key: None,
            upstream_model: None,
            system_first: None,
            max_body_bytes: cap,
            max_inflight: None,
        }
    }

    fn route_budget(url: &str, model: &str, max_inflight: usize) -> UpstreamRoute {
        UpstreamRoute {
            max_inflight: Some(max_inflight),
            ..route(url, model, None)
        }
    }

    /// Two upstreams for one model form a pool: both are offered, and the cursor advances.
    #[test]
    fn route_candidates_round_robins_across_matching_pool() {
        let ups = vec![
            route("http://a:1", "m", None),
            route("http://b:2", "m", None),
        ];
        let rr = AtomicUsize::new(0);
        let body = br#"{"model":"m"}"#;
        let inflight = HashMap::new();
        let c1 = select_candidates(
            &ups,
            &rr,
            &inflight,
            &HashSet::new(),
            ("http://default:9", None),
            body,
            None,
        );
        let c2 = select_candidates(
            &ups,
            &rr,
            &inflight,
            &HashSet::new(),
            ("http://default:9", None),
            body,
            None,
        );
        assert_eq!(c1.len(), 2);
        assert_eq!(c2.len(), 2);
        assert_eq!(c1[0].base, "http://a:1");
        assert_eq!(c2[0].base, "http://b:2");
        assert_ne!(c1[0].base, c2[0].base);
    }

    /// Context fit: a device that cannot hold the body is dropped for that request.
    #[test]
    fn route_candidates_skips_upstream_that_cannot_fit_body() {
        let ups = vec![
            route("http://a:1", "m", Some(4)),
            route("http://b:2", "m", None),
        ];
        let rr = AtomicUsize::new(0);
        let cands = select_candidates(
            &ups,
            &rr,
            &HashMap::new(),
            &HashSet::new(),
            ("http://default:9", None),
            br#"{"model":"m"}"#,
            None,
        );
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].base, "http://b:2");
    }

    /// An unmatched model falls back to the single default upstream, with no rewrite.
    #[test]
    fn route_candidates_falls_back_to_default_for_unmatched_model() {
        let ups = vec![route("http://a:1", "m", None)];
        let rr = AtomicUsize::new(0);
        let cands = select_candidates(
            &ups,
            &rr,
            &HashMap::new(),
            &HashSet::new(),
            ("http://default:9", None),
            br#"{"model":"other"}"#,
            None,
        );
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].base, "http://default:9");
    }

    /// Least-connections: the idler leads regardless of the cursor; the busy peer stays last.
    #[test]
    fn route_candidates_prefers_idler_upstream() {
        let ups = vec![
            route("http://a:1", "m", None),
            route("http://b:2", "m", None),
        ];
        let rr = AtomicUsize::new(0);
        let mut inflight = HashMap::new();
        inflight.insert("http://a:1".to_string(), Arc::new(AtomicUsize::new(5)));
        inflight.insert("http://b:2".to_string(), Arc::new(AtomicUsize::new(0)));
        let body = br#"{"model":"m"}"#;
        let c1 = select_candidates(
            &ups,
            &rr,
            &inflight,
            &HashSet::new(),
            ("http://default:9", None),
            body,
            None,
        );
        let c2 = select_candidates(
            &ups,
            &rr,
            &inflight,
            &HashSet::new(),
            ("http://default:9", None),
            body,
            None,
        );
        assert_eq!(c1[0].base, "http://b:2");
        assert_eq!(c2[0].base, "http://b:2");
        assert_eq!(c1[1].base, "http://a:1");
    }

    /// Affinity: one key always yields the same order, whatever the cursor and load.
    #[test]
    fn affinity_pins_a_stable_primary_regardless_of_cursor_and_load() {
        let ups = vec![
            route("http://a:1", "m", None),
            route("http://b:2", "m", None),
        ];
        let rr = AtomicUsize::new(0);
        let mut inflight = HashMap::new();
        inflight.insert("http://a:1".to_string(), Arc::new(AtomicUsize::new(9)));
        inflight.insert("http://b:2".to_string(), Arc::new(AtomicUsize::new(0)));
        let body = br#"{"model":"m"}"#;
        let first = select_candidates(
            &ups,
            &rr,
            &inflight,
            &HashSet::new(),
            ("http://d:9", None),
            body,
            Some("t1:a7"),
        );
        for _ in 0..5 {
            let again = select_candidates(
                &ups,
                &rr,
                &inflight,
                &HashSet::new(),
                ("http://d:9", None),
                body,
                Some("t1:a7"),
            );
            assert_eq!(again[0].base, first[0].base);
            assert_eq!(again[1].base, first[1].base);
        }
        assert_eq!(first.len(), 2);
    }

    /// Distinct keys spread across the pool — balance without giving up stickiness.
    #[test]
    fn affinity_spreads_distinct_keys_across_devices() {
        let ups = vec![
            route("http://a:1", "m", None),
            route("http://b:2", "m", None),
        ];
        let rr = AtomicUsize::new(0);
        let inflight = HashMap::new();
        let body = br#"{"model":"m"}"#;
        let mut leaders = std::collections::HashSet::new();
        for i in 0..32 {
            let key = format!("t1:a{i}");
            let c = select_candidates(
                &ups,
                &rr,
                &inflight,
                &HashSet::new(),
                ("http://d:9", None),
                body,
                Some(&key),
            );
            leaders.insert(c[0].base.clone());
        }
        assert_eq!(
            leaders.len(),
            2,
            "32 keys never landed on one of the two devices"
        );
    }

    /// Context fit dominates affinity: a device that cannot hold this body is still excluded.
    #[test]
    fn affinity_respects_context_fit_exclusion() {
        let ups = vec![
            route("http://a:1", "m", Some(4)),
            route("http://b:2", "m", None),
        ];
        let rr = AtomicUsize::new(0);
        let c = select_candidates(
            &ups,
            &rr,
            &HashMap::new(),
            &HashSet::new(),
            ("http://d:9", None),
            br#"{"model":"m"}"#,
            Some("anykey"),
        );
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].base, "http://b:2");
    }

    /// Saturation escape: a pinned primary at its budget spills, and the pin resumes on drain.
    #[test]
    fn affinity_escapes_saturated_primary_and_returns_after_drain() {
        let ups = vec![
            route_budget("http://a:1", "m", 2),
            route_budget("http://b:2", "m", 2),
        ];
        let rr = AtomicUsize::new(0);
        let body = br#"{"model":"m"}"#;
        let idle = select_candidates(
            &ups,
            &rr,
            &HashMap::new(),
            &HashSet::new(),
            ("http://d:9", None),
            body,
            Some("hotkey"),
        );
        let primary = idle[0].base.clone();
        let secondary = idle[1].base.clone();
        let mut inflight = HashMap::new();
        inflight.insert(primary.clone(), Arc::new(AtomicUsize::new(2)));
        inflight.insert(secondary.clone(), Arc::new(AtomicUsize::new(0)));
        let spilled = select_candidates(
            &ups,
            &rr,
            &inflight,
            &HashSet::new(),
            ("http://d:9", None),
            body,
            Some("hotkey"),
        );
        assert_eq!(spilled[0].base, secondary);
        assert_eq!(spilled[1].base, primary);
        inflight[&primary].store(0, Ordering::Relaxed);
        let back = select_candidates(
            &ups,
            &rr,
            &inflight,
            &HashSet::new(),
            ("http://d:9", None),
            body,
            Some("hotkey"),
        );
        assert_eq!(back[0].base, primary);
    }

    /// All saturated ⇒ the band sort is uniform and the deterministic order stands.
    #[test]
    fn affinity_keeps_hrw_order_when_all_saturated() {
        let ups = vec![
            route_budget("http://a:1", "m", 1),
            route_budget("http://b:2", "m", 1),
        ];
        let rr = AtomicUsize::new(0);
        let body = br#"{"model":"m"}"#;
        let idle = select_candidates(
            &ups,
            &rr,
            &HashMap::new(),
            &HashSet::new(),
            ("http://d:9", None),
            body,
            Some("k"),
        );
        let mut inflight = HashMap::new();
        inflight.insert("http://a:1".to_string(), Arc::new(AtomicUsize::new(9)));
        inflight.insert("http://b:2".to_string(), Arc::new(AtomicUsize::new(9)));
        let sat = select_candidates(
            &ups,
            &rr,
            &inflight,
            &HashSet::new(),
            ("http://d:9", None),
            body,
            Some("k"),
        );
        assert_eq!(sat[0].base, idle[0].base);
        assert_eq!(sat[1].base, idle[1].base);
    }

    /// A tripped upstream is demoted in BOTH ordering modes — never dropped.
    #[test]
    fn breaker_tripped_upstream_is_demoted_never_dropped() {
        let ups = vec![
            route("http://a:1", "m", None),
            route("http://b:2", "m", None),
        ];
        let rr = AtomicUsize::new(0);
        let body = br#"{"model":"m"}"#;
        let mut unavailable = HashSet::new();
        unavailable.insert("http://a:1".to_string());
        let mut inflight = HashMap::new();
        inflight.insert("http://a:1".to_string(), Arc::new(AtomicUsize::new(0)));
        inflight.insert("http://b:2".to_string(), Arc::new(AtomicUsize::new(7)));
        let c = select_candidates(
            &ups,
            &rr,
            &inflight,
            &unavailable,
            ("http://d:9", None),
            body,
            None,
        );
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].base, "http://b:2");
        assert_eq!(c[1].base, "http://a:1");
        for i in 0..8 {
            let key = format!("k{i}");
            let ca = select_candidates(
                &ups,
                &rr,
                &inflight,
                &unavailable,
                ("http://d:9", None),
                body,
                Some(&key),
            );
            assert_eq!(ca[0].base, "http://b:2");
        }
    }

    /// The hash is pinned by test, not by dependency — the affinity map must never shift.
    #[test]
    fn fnv1a64_matches_reference_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }
}
