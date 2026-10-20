//! `/_node/*` — box observability served through the tunnel. See docs/wiki/node-observability.md

use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header::CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio::process::Command;

use crate::app::AppState;

const CMD_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_LOG_LINES: u32 = 1000;
/// 240 points at 30s ≈ 2h of trend.
const HISTORY_RING: usize = 240;
pub const HISTORY_EVERY_SECS: u64 = 30;

struct NodePoint {
    at: Instant,
    gpu_util_avg: Option<f64>,
    gpu_mem_used_mib: Option<f64>,
    load_1m: Option<f64>,
}

/// Shared trend ring; cloned into `AppState` and the background sampler.
#[derive(Clone, Default)]
pub struct NodeHistory(Arc<Mutex<VecDeque<NodePoint>>>);

impl NodeHistory {
    /// One sampler tick: GPUs + load appended to the ring.
    pub async fn sample(&self) {
        let gpus = run(
            "nvidia-smi",
            &[
                "--query-gpu=index,name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw",
                "--format=csv,noheader,nounits",
            ],
        )
        .await
        .map(|csv| parse_gpus(&csv));
        let (util, mem) = match &gpus {
            Some(g) if !g.is_empty() => {
                let utils: Vec<f64> = g.iter().filter_map(|x| x["util_pct"].as_f64()).collect();
                let mems: Vec<f64> = g
                    .iter()
                    .filter_map(|x| x["mem_used_mib"].as_f64())
                    .collect();
                (
                    (!utils.is_empty()).then(|| {
                        let (sum, count) = utils
                            .iter()
                            .fold((0.0, 0.0), |(sum, count), value| (sum + value, count + 1.0));
                        sum / count
                    }),
                    (!mems.is_empty()).then(|| mems.iter().sum::<f64>()),
                )
            }
            _ => (None, None),
        };
        let point = NodePoint {
            at: Instant::now(),
            gpu_util_avg: util,
            gpu_mem_used_mib: mem,
            load_1m: read_loadavg().map(|l| l.0),
        };
        if let Ok(mut ring) = self.0.lock() {
            ring.push_back(point);
            if ring.len() > HISTORY_RING {
                ring.pop_front();
            }
        }
    }

    /// Relative `ago_secs`, so a consumer needs no clock agreement with the box.
    fn serialize(&self) -> Vec<serde_json::Value> {
        let Ok(ring) = self.0.lock() else {
            return Vec::new();
        };
        let now = Instant::now();
        ring.iter()
            .map(|p| {
                serde_json::json!({
                    "ago_secs": now.duration_since(p.at).as_secs(),
                    "gpu_util_avg": p.gpu_util_avg,
                    "gpu_mem_used_mib": p.gpu_mem_used_mib,
                    "load_1m": p.load_1m,
                })
            })
            .collect()
    }
}

/// Argv-style with a hard timeout — no shell, so nothing from a request reaches a command line.
async fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(CMD_TIMEOUT, child).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

/// Malformed rows are skipped, never fatal.
fn parse_gpus(csv: &str) -> Vec<serde_json::Value> {
    csv.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split(',').map(str::trim).collect();
            if f.len() < 7 {
                return None;
            }
            Some(serde_json::json!({
                "index": f[0].parse::<u32>().ok()?,
                "name": f[1],
                "util_pct": f[2].parse::<f64>().ok(),
                "mem_used_mib": f[3].parse::<f64>().ok(),
                "mem_total_mib": f[4].parse::<f64>().ok(),
                "temp_c": f[5].parse::<f64>().ok(),
                "power_w": f[6].parse::<f64>().ok(),
            }))
        })
        .collect()
}

/// The node's own name, so a console can label what it shows without hard-coding it.
fn read_hostname() -> Option<String> {
    hostname_from_str(&std::fs::read_to_string("/etc/hostname").ok()?)
}

fn hostname_from_str(raw: &str) -> Option<String> {
    let name = raw.trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn read_loadavg() -> Option<(f64, f64, f64)> {
    let s = std::fs::read_to_string("/proc/loadavg").ok()?;
    let mut it = s.split_whitespace();
    Some((
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
    ))
}

fn read_meminfo_kib(key: &str) -> Option<u64> {
    let s = std::fs::read_to_string("/proc/meminfo").ok()?;
    s.lines()
        .find(|l| l.starts_with(key))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// `GET /_node/stats` — GPUs, load, memory. Missing sources degrade to null.
pub async fn stats(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(deny) = st.auth.check(&headers) {
        return deny;
    }
    let gpu_csv = run(
        "nvidia-smi",
        &[
            "--query-gpu=index,name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw",
            "--format=csv,noheader,nounits",
        ],
    )
    .await;
    let load = read_loadavg();
    let body = serde_json::json!({
        "gpus": gpu_csv.as_deref().map(parse_gpus),
        "load_1m": load.map(|l| l.0),
        "load_5m": load.map(|l| l.1),
        "load_15m": load.map(|l| l.2),
        "cpu_count": std::thread::available_parallelism().map(usize::from).ok(),
        "mem_total_mib": read_meminfo_kib("MemTotal").map(|k| k / 1024),
        "mem_available_mib": read_meminfo_kib("MemAvailable").map(|k| k / 1024),
        "hostname": read_hostname(),
        "log_units": st.log_units,
        "history": st.node_history.serialize(),
    });
    ([(CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}

#[derive(Deserialize)]
pub struct LogsQuery {
    /// Must be one of the configured `log_units` — a whitelist, not a free parameter.
    pub unit: Option<String>,
    pub lines: Option<u32>,
}

/// `GET /_node/logs?unit=&lines=` — journald tail; user scope first, then system.
pub async fn logs(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<LogsQuery>,
) -> Response {
    if let Some(deny) = st.auth.check(&headers) {
        return deny;
    }
    let Some(unit) = q
        .unit
        .or_else(|| st.log_units.first().cloned())
        .filter(|u| st.log_units.contains(u))
    else {
        return (
            StatusCode::BAD_REQUEST,
            format!(
                "unknown unit — configure it in log_units (available: {})",
                st.log_units.join(", ")
            ),
        )
            .into_response();
    };
    let lines = q.lines.unwrap_or(200).clamp(10, MAX_LOG_LINES).to_string();
    let base = ["-q", "--no-pager", "-o", "short-iso", "-n", lines.as_str()];
    let user = run(
        "journalctl",
        &[&base[..], &["--user", "-u", unit.as_str()]].concat(),
    )
    .await
    .filter(|s| !s.trim().is_empty());
    let text = match user {
        Some(t) => t,
        None => run("journalctl", &[&base[..], &["-u", unit.as_str()]].concat())
            .await
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("no journal entries for unit `{unit}`")),
    };
    ([(CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostname_is_trimmed_and_never_empty() {
        assert_eq!(hostname_from_str("g292-1\n"), Some("g292-1".to_string()));
        assert_eq!(hostname_from_str("  \n"), None);
    }

    #[test]
    fn gpu_csv_parses_and_skips_malformed() {
        let csv = "0, Example GPU 12GB, 37, 10240, 12282, 61, 68.5\nbad line\n";
        let gpus = parse_gpus(csv);
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0]["index"], 0);
        assert_eq!(gpus[0]["util_pct"], 37.0);
        assert_eq!(gpus[0]["mem_total_mib"], 12282.0);
    }

    // /proc exists only on Linux; elsewhere these sources degrade to null by design.
    #[test]
    #[cfg(target_os = "linux")]
    fn loadavg_and_meminfo_read_on_linux() {
        assert!(read_loadavg().is_some());
        assert!(read_meminfo_kib("MemTotal").is_some());
    }
}
