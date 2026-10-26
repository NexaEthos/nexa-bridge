//! Sidecar lifecycle and verified state. See docs/wiki/desktop.md#connection-state

use crate::settings::{self, Settings};
use nexa_link::agora;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_shell::ShellExt;
use tauri_plugin_shell::process::{CommandChild, CommandEvent};

pub const BRIDGE: &str = "nexa-llm-bridge";
pub const CONNECTOR: &str = "nexa-llm-connector";
const BACKOFF: [u64; 5] = [1, 2, 5, 10, 30];
const TUNNEL_FRESHNESS: Duration = Duration::from_mins(1);
// A sidecar that ran this long before dying starts the backoff ladder again from the bottom.
const HEALTHY_RUN: Duration = Duration::from_mins(1);
// The grace between SIGTERM and the kill exists only where SIGTERM does.
#[cfg(unix)]
const STOP_GRACE: Duration = Duration::from_secs(3);
const VERIFY_POLL: Duration = Duration::from_millis(500);
const ENDED: &str = "Agora no longer accepts this computer: it was revoked, its connection was removed or the workspace \
is suspended. Use Link to Agora again, or Connect once the workspace is active.";

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Stopped,
    Starting,
    Running,
    Retrying,
    Failed,
}

#[derive(Clone, Serialize, Debug)]
pub struct Status {
    pub phase: Phase,
    pub bridge_up: bool,
    pub connector_up: bool,
    pub detail: Option<String>,
    pub restarts: u32,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            phase: Phase::Stopped,
            bridge_up: false,
            connector_up: false,
            detail: None,
            restarts: 0,
        }
    }
}

#[derive(Default)]
struct Inner {
    children: Vec<(&'static str, CommandChild)>,
    status: Status,
    generation: u64,
    tunnel_seen: Option<Instant>,
    bridge_instance: Option<BridgeInstance>,
}

struct BridgeInstance {
    id: String,
    verified: bool,
}

impl Inner {
    fn bridge_verified(&self) -> bool {
        self.bridge_instance
            .as_ref()
            .is_some_and(|instance| instance.verified)
    }

    fn verify_bridge(&mut self, generation: u64, instance_id: &str) -> bool {
        if self.generation != generation {
            return false;
        }
        if let Some(instance) = self.bridge_instance.as_mut()
            && instance.id == instance_id
        {
            instance.verified = true;
            return true;
        }
        false
    }
}

struct RunConfig {
    settings: Settings,
    token: String,
    api_key: String,
    generation: u64,
    data_dir: PathBuf,
}

enum ChildStart {
    WaitingForBridge,
    Running {
        events: tauri::async_runtime::Receiver<CommandEvent>,
        pid: u32,
    },
}

#[derive(Clone, Default)]
pub struct Supervisor(Arc<Mutex<Inner>>);

impl Supervisor {
    pub fn status(&self) -> Status {
        match self.0.lock() {
            Ok(inner) => inner.status.clone(),
            Err(e) => {
                tracing::error!(error = %e, "supervisor state unavailable");
                Status {
                    phase: Phase::Failed,
                    detail: Some("Supervisor state unavailable; restart the app.".into()),
                    ..Status::default()
                }
            }
        }
    }

    pub fn is_running(&self) -> bool {
        self.status().phase != Phase::Stopped
    }

    pub fn start(
        &self,
        app: &AppHandle,
        settings: &Settings,
        token: &str,
        api_key: &str,
    ) -> Result<(), String> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(2))
            .build()
            .map_err(|e| e.to_string())?;
        let data_dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&data_dir)
            .map_err(|e| format!("Could not create the data directory: {e}"))?;
        let generation = {
            let mut inner = self.0.lock().map_err(|e| e.to_string())?;
            if inner.status.phase != Phase::Stopped {
                return Err("Disconnect before starting another connection.".into());
            }
            inner.generation += 1;
            inner.tunnel_seen = None;
            inner.bridge_instance = None;
            inner.status = Status {
                phase: Phase::Starting,
                ..Status::default()
            };
            inner.generation
        };
        let cfg = Arc::new(RunConfig {
            settings: settings.clone(),
            token: token.into(),
            api_key: api_key.into(),
            generation,
            data_dir,
        });
        for service in [BRIDGE, CONNECTOR] {
            let this = self.clone();
            let app = app.clone();
            let cfg = cfg.clone();
            tauri::async_runtime::spawn(async move {
                this.run_service(&app, service, &cfg).await;
            });
        }
        let this = self.clone();
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            this.monitor(&handle, &cfg, &client).await;
        });
        self.publish(app);
        Ok(())
    }

    pub fn stop(&self, app: &AppHandle) -> Result<(), String> {
        let (children, mut errors) = {
            let mut inner = self.0.lock().map_err(|e| e.to_string())?;
            let errors = if inner.status.phase == Phase::Failed {
                vec![inner.status.detail.clone().unwrap_or_else(|| {
                    "Previous shutdown failed; check sidecar processes and restart the app.".into()
                })]
            } else {
                inner.status = Status::default();
                Vec::new()
            };
            inner.generation += 1;
            inner.tunnel_seen = None;
            inner.bridge_instance = None;
            inner.status.bridge_up = false;
            inner.status.connector_up = false;
            (std::mem::take(&mut inner.children), errors)
        };
        for (service, child) in children {
            if let Some(error) = terminate(service, child) {
                errors.push(error);
            }
        }
        if !errors.is_empty() {
            let message = errors.join("; ");
            let mut inner = self.0.lock().map_err(|e| e.to_string())?;
            inner.status.phase = Phase::Failed;
            inner.status.detail = Some(message.clone());
            drop(inner);
            self.publish(app);
            return Err(message);
        }
        self.publish(app);
        Ok(())
    }

    fn current(&self, generation: u64) -> bool {
        match self.0.lock() {
            Ok(inner) => inner.generation == generation,
            Err(e) => {
                tracing::error!(error = %e, "supervisor state unavailable");
                false
            }
        }
    }

    fn spawn_child(
        &self,
        app: &AppHandle,
        service: &'static str,
        cfg: &RunConfig,
    ) -> Result<ChildStart, String> {
        let instance_id = if service == BRIDGE {
            Some(settings::instance_id().map_err(|e| e.to_string())?)
        } else {
            None
        };
        let command = app
            .shell()
            .sidecar(service)
            .map_err(|e| format!("{service}: sidecar unavailable ({e})"))?
            // The data directory, never the launch directory: no stray config file is merged in.
            .current_dir(cfg.data_dir.clone());
        let command = if service == BRIDGE {
            command
                .env("BRIDGE_SUPERVISOR_PIPE", "true")
                .env("BRIDGE_UPSTREAM_URL", &cfg.settings.upstream_url)
                .env("BRIDGE_SELECTED_MODEL", &cfg.settings.selected_model)
                .env("BRIDGE_LISTEN_ADDR", &cfg.settings.listen_addr)
                .env("BRIDGE_API_KEY", &cfg.api_key)
                .env("BRIDGE_LOG_FORMAT", "json")
                .env("BRIDGE_FLAVOR", "desktop")
                .env("RUST_LOG", "info")
        } else {
            let command = if cfg.settings.managed.is_some() {
                command.env("CONNECTOR_AGORA_URL", &cfg.settings.agora_url)
            } else {
                command
            };
            command
                .env("CONNECTOR_SUPERVISOR_PIPE", "true")
                .env("CONNECTOR_RELAY_URL", &cfg.settings.relay_url)
                .env("CONNECTOR_TUNNEL_TOKEN", &cfg.token)
                .env(
                    "CONNECTOR_MANAGED_AUTH",
                    cfg.settings.managed.is_some().to_string(),
                )
                .env(
                    "CONNECTOR_UPSTREAM_URL",
                    format!("http://{}", cfg.settings.listen_addr),
                )
                .env("CONNECTOR_STATUS_API_KEY", &cfg.api_key)
                .env("CONNECTOR_LOG_FORMAT", "json")
                .env("RUST_LOG", "info")
        };
        let mut inner = self.0.lock().map_err(|e| e.to_string())?;
        if inner.generation != cfg.generation {
            return Err("Connection was stopped during startup.".into());
        }
        if service == CONNECTOR && !inner.bridge_verified() {
            return Ok(ChildStart::WaitingForBridge);
        }
        let command = if let Some(id) = &instance_id {
            command.env("BRIDGE_INSTANCE_ID", id)
        } else {
            command
        };
        let (events, child) = command
            .spawn()
            .map_err(|e| format!("{service}: could not start ({e})"))?;
        let pid = child.pid();
        if let Some(id) = instance_id {
            inner.bridge_instance = Some(BridgeInstance {
                id,
                verified: false,
            });
        }
        inner.children.push((service, child));
        drop(inner);
        Ok(ChildStart::Running { events, pid })
    }

    async fn run_service(&self, app: &AppHandle, service: &'static str, cfg: &RunConfig) {
        let mut attempt = 0usize;
        while self.current(cfg.generation) {
            if service == CONNECTOR {
                // No bearer leaves this app until `/healthz` proved the port is our bridge.
                while self.current(cfg.generation) && !self.bridge_verified(cfg.generation) {
                    tokio::time::sleep(VERIFY_POLL).await;
                }
                if !self.current(cfg.generation) {
                    return;
                }
            }
            let mut terminated = false;
            let mut ended = false;
            let mut started_pid = None;
            let detail = match self.spawn_child(app, service, cfg) {
                Ok(ChildStart::WaitingForBridge) => {
                    tokio::time::sleep(VERIFY_POLL).await;
                    continue;
                }
                Ok(ChildStart::Running { mut events, pid }) => {
                    started_pid = Some(pid);
                    let started = Instant::now();
                    let mut last_stderr: Option<String> = None;
                    let mut detail = format!("{service}: event stream closed");
                    while let Some(event) = events.recv().await {
                        if !self.current(cfg.generation) {
                            return;
                        }
                        match event {
                            CommandEvent::Stdout(bytes) if service == CONNECTOR => {
                                if let Some(connected) = tunnel_event(&bytes)
                                    && let Ok(mut inner) = self.0.lock()
                                    && inner.generation == cfg.generation
                                    && inner.children.iter().any(|(name, child)| {
                                        *name == CONNECTOR && child.pid() == pid
                                    })
                                    && inner.bridge_verified()
                                {
                                    inner.tunnel_seen = connected.then(Instant::now);
                                    if connected {
                                        attempt = 0;
                                    }
                                }
                            }
                            CommandEvent::Stderr(bytes) => {
                                last_stderr = last_line(&bytes).or(last_stderr);
                            }
                            CommandEvent::Terminated(payload) => {
                                terminated = true;
                                ended = device_ended(service, payload.code);
                                detail = exit_detail(service, payload.code, last_stderr.as_deref());
                                if started.elapsed() >= HEALTHY_RUN {
                                    attempt = 0;
                                }
                                break;
                            }
                            CommandEvent::Error(error) => {
                                tracing::error!(service, error = %error, "sidecar event error");
                                detail = format!("{service}: process event error; retry scheduled");
                                break;
                            }
                            _ => {}
                        }
                    }
                    detail
                }
                Err(error) => error,
            };
            if ended {
                self.end(app, cfg.generation, started_pid);
                return;
            }
            if service == CONNECTOR
                && let Some(pid) = started_pid
                && self.0.lock().is_ok_and(|inner| {
                    inner.generation == cfg.generation
                        && !inner
                            .children
                            .iter()
                            .any(|(name, child)| *name == CONNECTOR && child.pid() == pid)
                })
            {
                attempt = 0;
                continue;
            }
            if !self.retire_service(app, service, cfg.generation, terminated, &detail) {
                return;
            }
            tracing::warn!(service, detail, "sidecar unavailable; retry scheduled");
            self.publish(app);
            tokio::time::sleep(Duration::from_secs(BACKOFF[attempt.min(BACKOFF.len() - 1)])).await;
            attempt = attempt.saturating_add(1);
        }
    }

    /// Agora refused this computer: stop both sidecars and say so, instead of retrying a credential that is gone.
    fn end(&self, app: &AppHandle, generation: u64, pid: Option<u32>) {
        match self.0.lock() {
            Ok(mut inner) if inner.generation == generation => inner
                .children
                .retain(|(name, child)| *name != CONNECTOR || Some(child.pid()) != pid),
            Ok(_) => return,
            Err(e) => tracing::error!(error = %e, "supervisor state unavailable"),
        }
        if let Err(error) = self.stop(app) {
            tracing::error!(
                error,
                "sidecars did not stop after Agora refused this computer"
            );
            return;
        }
        if let Ok(mut inner) = self.0.lock()
            && inner.status.phase == Phase::Stopped
        {
            inner.status.detail = Some(ENDED.into());
        }
        self.publish(app);
    }

    fn retire_service(
        &self,
        app: &AppHandle,
        service: &'static str,
        generation: u64,
        terminated: bool,
        detail: &str,
    ) -> bool {
        if !self.current(generation) {
            return false;
        }
        if let Ok(mut inner) = self.0.lock() {
            if inner.generation != generation {
                return false;
            }
            if service == BRIDGE {
                inner.bridge_instance = None;
                inner.tunnel_seen = None;
                inner.status.bridge_up = false;
                inner.status.connector_up = false;
                if let Some(index) = inner
                    .children
                    .iter()
                    .position(|(name, _)| *name == CONNECTOR)
                {
                    let (_, child) = inner.children.swap_remove(index);
                    if let Err(error) = child.kill() {
                        inner.status.phase = Phase::Failed;
                        inner.status.detail = Some(format!(
                            "Could not stop connector after bridge exit: {error}. Restart the app after checking sidecar processes."
                        ));
                        drop(inner);
                        if let Err(error) = self.stop(app) {
                            tracing::error!(error, "dependent sidecar cleanup failed");
                        }
                        return false;
                    }
                }
            }
            if let Some(index) = inner.children.iter().position(|(name, _)| *name == service) {
                let (_, child) = inner.children.swap_remove(index);
                if !terminated && let Err(e) = child.kill() {
                    inner.generation += 1;
                    inner.status.phase = Phase::Failed;
                    inner.status.detail = Some(format!(
                        "Could not stop {service}: {e}. Inspect sidecar processes and restart the app."
                    ));
                    drop(inner);
                    if let Err(error) = self.stop(app) {
                        tracing::error!(error, "sidecar cleanup failed; retries stopped");
                    }
                    return false;
                }
            }
            if service == CONNECTOR {
                inner.tunnel_seen = None;
            }
            inner.status.phase = Phase::Retrying;
            inner.status.detail = Some(detail.to_owned());
            inner.status.restarts = inner.status.restarts.saturating_add(1);
        }
        true
    }

    fn bridge_verified(&self, generation: u64) -> bool {
        self.0
            .lock()
            .is_ok_and(|inner| inner.generation == generation && inner.bridge_verified())
    }

    fn mark_bridge_verified(&self, generation: u64, instance_id: &str) -> bool {
        self.0
            .lock()
            .is_ok_and(|mut inner| inner.verify_bridge(generation, instance_id))
    }

    async fn monitor(&self, app: &AppHandle, cfg: &RunConfig, client: &reqwest::Client) {
        while self.current(cfg.generation) {
            let address = &cfg.settings.listen_addr;
            let instance = self.0.lock().ok().and_then(|inner| {
                (inner.generation == cfg.generation)
                    .then(|| {
                        inner
                            .bridge_instance
                            .as_ref()
                            .map(|instance| (instance.id.clone(), instance.verified))
                    })
                    .flatten()
            });
            let ready = match instance.as_ref() {
                Some((id, verified))
                    if *verified
                        || healthz_instance(client, address).await.as_ref() == Some(id) =>
                {
                    self.mark_bridge_verified(cfg.generation, id)
                        && bridge_ready(client, address, &cfg.api_key).await
                }
                _ => false,
            };
            if let Ok(mut inner) = self.0.lock() {
                if inner.generation != cfg.generation {
                    return;
                }
                if inner.bridge_instance.as_ref().map(|instance| &instance.id)
                    != instance.as_ref().map(|(id, _)| id)
                {
                    continue;
                }
                let bridge_alive = inner.children.iter().any(|(name, _)| *name == BRIDGE);
                let connector_alive = inner.children.iter().any(|(name, _)| *name == CONNECTOR);
                inner.status.bridge_up = bridge_alive && ready;
                inner.status.connector_up = connector_alive
                    && inner
                        .tunnel_seen
                        .is_some_and(|seen| seen.elapsed() < TUNNEL_FRESHNESS);
                if inner.status.bridge_up && inner.status.connector_up {
                    inner.status.phase = Phase::Running;
                    inner.status.detail = None;
                } else {
                    inner.status.phase = Phase::Retrying;
                    if bridge_alive && connector_alive {
                        inner.status.detail = Some(if inner.status.connector_up {
                            "Tunnel attached; waiting for an authenticated bridge with a healthy model.".into()
                        } else {
                            "Waiting for an authenticated relay connection; check the relay URL and token.".into()
                        });
                    }
                }
            }
            self.publish(app);
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    fn publish(&self, app: &AppHandle) {
        let status = self.status();
        if let Err(e) = app.emit("status", &status) {
            tracing::warn!(error = %e, "could not publish desktop status");
        }
        crate::tray::refresh(app, &status);
    }
}

/// Ask first (SIGTERM, so the bridge drains), kill after the grace period; Windows has no SIGTERM.
fn terminate(service: &'static str, child: CommandChild) -> Option<String> {
    #[cfg(unix)]
    {
        use nix::sys::signal::{Signal, kill};
        use nix::unistd::Pid;

        match i32::try_from(child.pid()) {
            Ok(pid) => {
                if let Err(e) = kill(Pid::from_raw(pid), Signal::SIGTERM) {
                    tracing::warn!(service, error = %e, "SIGTERM was not delivered; killing");
                }
            }
            Err(e) => tracing::warn!(service, error = %e, "pid out of range; killing"),
        }
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(STOP_GRACE).await;
            // A process that already left makes this a no-op; one that stayed is killed now.
            if let Err(e) = child.kill() {
                tracing::error!(service, error = %e, "could not kill sidecar after the grace period");
            }
        });
        None
    }
    #[cfg(not(unix))]
    child.kill().err().map(|e| {
        tracing::error!(service, error = %e, "could not stop sidecar");
        format!("Could not stop {service}: {e}")
    })
}

/// The last non-empty stderr line, trimmed to 200 characters — the bind error a bridge dies with.
fn last_line(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    Some(line.chars().take(200).collect())
}

/// The connector exits with this status only once Agora said its device is gone.
fn device_ended(service: &str, code: Option<i32>) -> bool {
    service == CONNECTOR && code == Some(i32::from(agora::ENDED_EXIT_STATUS))
}

fn exit_detail(service: &str, code: Option<i32>, stderr: Option<&str>) -> String {
    stderr.map_or_else(
        || format!("{service} exited ({code:?}); retry scheduled"),
        |line| format!("{service} exited ({code:?}): {line}; retry scheduled"),
    )
}

/// The instance id `/healthz` echoes — unauthenticated, so nothing secret reaches a stranger on the port.
async fn healthz_instance(client: &reqwest::Client, address: &str) -> Option<String> {
    let response = client
        .get(format!("http://{address}/healthz"))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response
        .headers()
        .get("x-nexa-instance")?
        .to_str()
        .ok()
        .map(str::to_string)
}

fn tunnel_event(bytes: &[u8]) -> Option<bool> {
    let event: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    event
        .get("fields")?
        .get("desktop_tunnel_connected")?
        .as_bool()
}

async fn bridge_ready(client: &reqwest::Client, address: &str, key: &str) -> bool {
    let Ok(response) = client
        .get(format!("http://{address}/status"))
        .bearer_auth(key)
        .send()
        .await
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(status) = response.json::<serde_json::Value>().await else {
        return false;
    };
    serving_ready(&status)
}

fn serving_ready(status: &serde_json::Value) -> bool {
    status
        .get("models")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|models| !models.is_empty())
        && status
            .get("upstreams")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|upstreams| {
                upstreams.iter().any(|upstream| {
                    upstream.get("healthy").and_then(serde_json::Value::as_bool) == Some(true)
                })
            })
}

pub fn shutdown(app: &AppHandle) {
    if let Some(sup) = app.try_state::<Supervisor>()
        && let Err(e) = sup.stop(app)
    {
        tracing::error!(error = %e, "desktop shutdown incomplete");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_current_bridge_instance_can_be_verified() {
        let mut inner = Inner {
            generation: 1,
            bridge_instance: Some(BridgeInstance {
                id: "first".into(),
                verified: false,
            }),
            ..Inner::default()
        };
        assert!(!inner.verify_bridge(0, "first"));
        assert!(!inner.verify_bridge(1, "other"));
        assert!(!inner.bridge_verified());
        assert!(inner.verify_bridge(1, "first"));
        assert!(inner.bridge_verified());

        inner.bridge_instance = None;
        assert!(!inner.verify_bridge(1, "first"));
        assert!(!inner.bridge_verified());
        inner.bridge_instance = Some(BridgeInstance {
            id: "replacement".into(),
            verified: false,
        });
        assert!(!inner.verify_bridge(1, "first"));
        assert!(!inner.bridge_verified());
        assert!(inner.verify_bridge(1, "replacement"));
        inner.generation = 2;
        assert!(!inner.verify_bridge(1, "replacement"));
    }

    #[test]
    fn the_last_stderr_line_names_the_exit() {
        assert_eq!(
            last_line(b"Error: bind 127.0.0.1:8088\n\nCaused by:\n    Address already in use (os error 48)\n"),
            Some("Address already in use (os error 48)".into())
        );
        assert_eq!(last_line(b"\n  \n"), None);
        assert_eq!(
            exit_detail(
                "nexa-llm-bridge",
                Some(1),
                Some("Address already in use (os error 48)")
            ),
            "nexa-llm-bridge exited (Some(1)): Address already in use (os error 48); retry scheduled"
        );
        assert_eq!(
            exit_detail("nexa-llm-connector", None, None),
            "nexa-llm-connector exited (None); retry scheduled"
        );
    }
    use serde_json::json;

    #[test]
    fn only_the_connectors_ended_status_stops_the_connection() {
        assert!(device_ended(CONNECTOR, Some(77)));
        assert!(!device_ended(BRIDGE, Some(77)));
        assert!(!device_ended(CONNECTOR, Some(1)));
        assert!(!device_ended(CONNECTOR, None));
    }

    #[test]
    fn connection_events_are_typed_not_log_messages() {
        assert_eq!(
            tunnel_event(br#"{"fields":{"desktop_tunnel_connected":true}}"#),
            Some(true)
        );
        assert_eq!(
            tunnel_event(br#"{"fields":{"desktop_tunnel_connected":false}}"#),
            Some(false)
        );
        assert_eq!(
            tunnel_event(br#"{"fields":{"message":"attached to relay"}}"#),
            None
        );
        assert_eq!(tunnel_event(b"not json"), None);
    }

    #[test]
    fn running_requires_models_and_a_healthy_upstream() {
        assert!(!serving_ready(
            &json!({"models": [], "upstreams": [{"healthy": true}]})
        ));
        assert!(!serving_ready(
            &json!({"models": [{"id": "model"}], "upstreams": [{"healthy": false}]})
        ));
        assert!(serving_ready(
            &json!({"models": [{"id": "model"}], "upstreams": [{"healthy": true}]})
        ));
        assert!(!serving_ready(&json!({})));
    }
}
