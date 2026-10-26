//! # Wiki: docs/wiki/managed-auth.md
#![forbid(unsafe_code)]
#![deny(warnings, rust_2018_idioms, rust_2021_compatibility)]
#![warn(missing_docs, missing_debug_implementations)]

mod cli;
mod config;
mod managed;

use std::io::{Read, Write as _};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use nexa_link::agora::{self, PairingEnded};
use nexa_tunnel_protocol::{Frame, MAX_WS_MSG_BYTES, ReqMeta, RequestId, RespMeta};
use reqwest::Client;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::connect_async_with_config;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use crate::config::Config;

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

fn is_hop_by_hop(name: &str) -> bool {
    HOP_BY_HOP.iter().any(|h| name.eq_ignore_ascii_case(h))
}

fn send_frame(tx: &mpsc::UnboundedSender<Message>, frame: &Frame) -> anyhow::Result<()> {
    let encoded = frame.encode()?;
    tx.send(Message::Binary(encoded))
        .map_err(|_| anyhow::anyhow!("tunnel writer is unavailable"))
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    if std::env::args()
        .nth(1)
        .is_some_and(|flag| flag == "--version" || flag == "-V")
    {
        writeln!(
            std::io::stdout(),
            "{}",
            nexa_tunnel_protocol::build_info!().describe("nexa-llm-connector")
        )?;
        return Ok(ExitCode::SUCCESS);
    }
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("could not install the connector TLS provider"))?;
    let words: Vec<String> = std::env::args().skip(1).collect();
    if cli::is_command(words.first().map(String::as_str)) {
        return cli::run(&words).await.map(|()| ExitCode::SUCCESS);
    }

    let cfg = Config::load()?;
    config::init_tracing(&cfg);
    let mut builder = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(cfg.upstream_read_timeout_secs))
        .tcp_nodelay(true)
        .redirect(reqwest::redirect::Policy::none());
    if cfg.managed_auth {
        builder = builder.no_proxy();
    }
    let client = builder.build()?;

    tracing::info!(relay = %cfg.relay_url, upstream = %cfg.upstream_url, "nexa-llm-connector starting");
    if cfg.supervisor_pipe {
        tokio::select! {
            () = run(&cfg, &client) => {}
            () = pipe_closed() => {
                tracing::info!("supervisor pipe closed; stopping");
                return Ok(ExitCode::SUCCESS);
            }
        }
    } else {
        run(&cfg, &client).await;
    }
    Ok(ExitCode::from(agora::ENDED_EXIT_STATUS))
}

/// Dial, serve, dial again, until Agora says the device is gone: the wait doubles up to `reconnect_max_secs`.
async fn run(cfg: &Config, client: &Client) {
    let mut wait = cfg.reconnect_secs;
    loop {
        let attached = AtomicBool::new(false);
        match serve(cfg, client, &attached).await {
            Ok(()) => tracing::info!("tunnel closed cleanly; reconnecting"),
            Err(e) if refused(&e) && device_ended(cfg).await => return,
            Err(e) => tracing::warn!(error = %e, "tunnel error; reconnecting"),
        }
        tracing::info!(desktop_tunnel_connected = false, "relay detached");
        wait = if attached.load(Ordering::Relaxed) {
            cfg.reconnect_secs
        } else {
            next_wait(wait, cfg.reconnect_max_secs)
        };
        tokio::time::sleep(jittered(wait)).await;
    }
}

/// The relay refused the credential at the handshake: the one case worth asking Agora about.
fn refused(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<WsError>(),
        Some(WsError::Http(response)) if response.status() == StatusCode::UNAUTHORIZED
    )
}

/// Ask Agora about the device: only its final word ends the loop; an unreachable Agora is retried.
async fn device_ended(cfg: &Config) -> bool {
    let Some(origin) = cfg.agora_url.as_deref() else {
        return false;
    };
    match agora::device_status(origin, &cfg.tunnel_token).await {
        Err(error) if error.downcast_ref::<PairingEnded>().is_some() => {
            tracing::error!(
                reason = %error,
                "Agora no longer accepts this computer: it was revoked, its connection was removed or the workspace \
                 is suspended; reconnecting stopped"
            );
            true
        }
        Err(error) => {
            tracing::warn!(error = %error, "the relay refused the credential and Agora did not answer; reconnecting");
            false
        }
        Ok(device) => {
            tracing::warn!(status = %device.status, "the relay refused a credential Agora still knows; reconnecting");
            false
        }
    }
}

/// Double the wait, never past the ceiling.
const fn next_wait(previous: u64, max: u64) -> u64 {
    let doubled = previous.saturating_mul(2);
    if doubled > max { max } else { doubled }
}

/// Spread reconnects by ±25 % from the clock's sub-second noise — no PRNG for a jitter.
fn jittered(secs: u64) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let per_mille = 750 + u64::from(nanos % 501);
    Duration::from_millis(secs.saturating_mul(per_mille))
}

/// Wait until stdin ends: a supervisor that stops, crashes or is killed closes the pipe.
async fn pipe_closed() {
    // A runtime-blocking read of stdin can never be cancelled, so an ended loop would wait on it to exit.
    let _ = closed_when_ends(std::io::stdin()).await;
}

/// Read on a thread of its own until the reader ends, then say so; the runtime never waits for that thread.
fn closed_when_ends<R: Read + Send + 'static>(mut reader: R) -> oneshot::Receiver<()> {
    let (ended, closed) = oneshot::channel();
    std::thread::spawn(move || {
        let mut sink = [0u8; 256];
        while matches!(reader.read(&mut sink), Ok(n) if n > 0) {}
        let _ = ended.send(());
    });
    closed
}

async fn serve(cfg: &Config, client: &Client, attached: &AtomicBool) -> anyhow::Result<()> {
    let mut request = cfg.relay_url.as_str().into_client_request()?;
    request.headers_mut().insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", cfg.tunnel_token))?,
    );
    if cfg.managed_auth {
        request
            .headers_mut()
            .insert(managed::HEADER, HeaderValue::from_static(managed::VERSION));
    }
    let ws_config = WebSocketConfig::default()
        .max_message_size(Some(MAX_WS_MSG_BYTES))
        .max_frame_size(Some(MAX_WS_MSG_BYTES));
    let (ws, response) = connect_async_with_config(request, Some(ws_config), true).await?;
    managed::verify_handshake(cfg.managed_auth, response.headers().get(managed::HEADER))?;
    attached.store(true, Ordering::Relaxed);
    tracing::info!(desktop_tunnel_connected = true, "attached to relay");
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();

    let writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Err(error) = sink.send(msg).await {
                tracing::warn!(%error, "tunnel writer stopped");
                break;
            }
        }
    });

    let status_task = spawn_status_reporter(cfg, client, &tx);

    let ka_tx = tx.clone();
    let keepalive_secs = cfg.keepalive_secs;
    let keepalive = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(keepalive_secs)).await;
            if ka_tx.send(Message::Ping(Bytes::new())).is_err() {
                break;
            }
        }
    });

    while let Some(message) = stream.next().await {
        let msg = match message {
            Ok(msg) => msg,
            Err(error) => {
                tracing::warn!(%error, "tunnel reader stopped");
                break;
            }
        };
        match msg {
            Message::Binary(bytes) => match Frame::decode(bytes) {
                Ok(Frame::Req { id, mut meta, body }) => {
                    let tx = tx.clone();
                    let client = client.clone();
                    let upstream = cfg.upstream_url.clone();
                    let local_key = if cfg.managed_auth {
                        cfg.status_api_key.clone()
                    } else {
                        None
                    };
                    tokio::spawn(async move {
                        let result = async {
                            if let Some(key) = local_key {
                                managed::authorize_local(&mut meta, &key)?;
                            }
                            handle_request(&client, &upstream, id, meta, body, &tx).await
                        }
                        .await;
                        if let Err(error) = result {
                            tracing::warn!(id, error = %error, "request forwarding failed");
                            if let Err(error) = send_frame(
                                &tx,
                                &Frame::RespError {
                                    id,
                                    message: "request forwarding failed".into(),
                                },
                            ) {
                                tracing::warn!(id, error = %error, "could not send terminal request error");
                            }
                        }
                    });
                }
                Ok(_) => tracing::warn!("relay sent an unexpected frame"),
                Err(e) => tracing::warn!(error = %e, "bad frame from relay"),
            },
            Message::Ping(p) => {
                if tx.send(Message::Pong(p)).is_err() {
                    tracing::warn!("tunnel writer stopped before pong");
                    break;
                }
            }
            Message::Close(_) => break,
            Message::Pong(_) => tracing::info!(desktop_tunnel_connected = true, "relay heartbeat"),
            _ => {}
        }
    }

    keepalive.abort();
    status_task.abort();
    writer.abort();
    Ok(())
}

fn spawn_status_reporter(
    cfg: &Config,
    client: &Client,
    tx: &mpsc::UnboundedSender<Message>,
) -> tokio::task::JoinHandle<()> {
    let status_tx = tx.clone();
    let status_client = client.clone();
    let status_url = match cfg.status_url.as_deref() {
        Some("") => None,
        Some(explicit) => Some(explicit.to_string()),
        None => Some(format!("{}/status", cfg.upstream_url.trim_end_matches('/'))),
    };
    let status_key = cfg.status_api_key.clone();
    let status_interval = cfg.status_interval_secs;
    tokio::spawn(async move {
        let Some(url) = status_url else { return };
        let mut last_ok: Option<bool> = None;
        loop {
            let req = status_key.as_ref().map_or_else(
                || status_client.get(&url),
                |key| status_client.get(&url).bearer_auth(key),
            );
            let ok = match req.send().await {
                Ok(resp) if resp.status().is_success() => resp.bytes().await.is_ok_and(|body| {
                    let frame = Frame::Status { payload: body };
                    send_frame(&status_tx, &frame).is_ok()
                }),
                _ => false,
            };
            if last_ok != Some(ok) {
                if ok {
                    tracing::info!("status reporting active ({url})");
                } else {
                    tracing::warn!(
                        "status endpoint unreachable ({url}) — relay will show 'state not reported'"
                    );
                }
                last_ok = Some(ok);
            }
            tokio::time::sleep(Duration::from_secs(status_interval)).await;
        }
    })
}

async fn handle_request(
    client: &Client,
    upstream: &str,
    id: RequestId,
    meta: ReqMeta,
    body: Bytes,
    tx: &mpsc::UnboundedSender<Message>,
) -> anyhow::Result<()> {
    let url = request_url(upstream, &meta.path)?;
    let method = reqwest::Method::from_bytes(meta.method.as_bytes())?;
    let mut rb = client.request(method, &url).body(body);
    for (name, value) in &meta.headers {
        if is_hop_by_hop(name) || name.eq_ignore_ascii_case("host") {
            continue;
        }
        rb = rb.header(name, value);
    }

    let resp = match rb.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(id, error = %e.without_url(), "upstream request failed");
            send_frame(
                tx,
                &Frame::RespError {
                    id,
                    message: "upstream request failed".to_string(),
                },
            )?;
            return Ok(());
        }
    };

    let status = resp.status().as_u16();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .filter(|(n, _)| !is_hop_by_hop(n.as_str()) && *n != reqwest::header::CONTENT_LENGTH)
        .filter_map(|(n, v)| {
            v.to_str()
                .ok()
                .map(|v| (n.as_str().to_string(), v.to_string()))
        })
        .collect();
    send_frame(
        tx,
        &Frame::RespStart {
            id,
            meta: RespMeta { status, headers },
        },
    )?;

    let mut body_stream = resp.bytes_stream();
    while let Some(chunk) = body_stream.next().await {
        match chunk {
            Ok(data) => {
                send_frame(tx, &Frame::RespChunk { id, data })?;
            }
            Err(e) => {
                tracing::warn!(id, error = %e.without_url(), "upstream body stream failed");
                send_frame(
                    tx,
                    &Frame::RespError {
                        id,
                        message: "upstream stream error".to_string(),
                    },
                )?;
                return Ok(());
            }
        }
    }
    send_frame(tx, &Frame::RespEnd { id })
}

fn request_url(upstream: &str, path: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        path.starts_with('/')
            && !path.starts_with("//")
            && !path.contains('#')
            && !path.chars().any(char::is_control),
        "tunnel request target must be an origin-form path with an optional query"
    );
    Ok(format!("{}{}", upstream.trim_end_matches('/'), path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_wait_doubles_to_the_ceiling_and_jitters_within_a_quarter() {
        assert_eq!(next_wait(3, 60), 6);
        assert_eq!(next_wait(48, 60), 60);
        assert_eq!(next_wait(60, 60), 60);
        assert_eq!(next_wait(u64::MAX, 60), 60);
        for _ in 0..64 {
            let wait = jittered(8);
            assert!(wait >= Duration::from_secs(6) && wait <= Duration::from_secs(10));
        }
    }

    /// A reader that blocks for ever, as stdin does while the supervisor's pipe stays open.
    struct OpenPipe(std::sync::mpsc::Receiver<()>);

    impl Read for OpenPipe {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            let _ = self.0.recv();
            Ok(0)
        }
    }

    #[test]
    fn an_ended_loop_exits_while_the_supervisors_pipe_stays_open() -> anyhow::Result<()> {
        let (_keep_open, never) = std::sync::mpsc::channel();
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().map(|rt| {
                rt.block_on(async {
                    tokio::select! {
                        () = async {} => {}
                        _ = closed_when_ends(OpenPipe(never)) => {}
                    }
                });
                drop(rt);
            });
            let _ = done.send(runtime.is_ok());
        });
        assert!(
            finished.recv_timeout(Duration::from_secs(5))?,
            "the runtime stopped without waiting on the pipe"
        );
        Ok(())
    }

    #[test]
    fn a_closed_pipe_is_seen() -> anyhow::Result<()> {
        let runtime = tokio::runtime::Runtime::new()?;
        runtime.block_on(closed_when_ends(std::io::Cursor::new(
            b"still data".to_vec(),
        )))?;
        Ok(())
    }

    #[test]
    fn only_a_handshake_401_asks_agora_about_the_device() -> anyhow::Result<()> {
        let refusal = |status: u16| -> anyhow::Result<anyhow::Error> {
            let response = tokio_tungstenite::tungstenite::http::Response::builder()
                .status(status)
                .body(None)?;
            Ok(WsError::Http(Box::new(response)).into())
        };
        assert!(refused(&refusal(401)?));
        assert!(!refused(&refusal(403)?));
        assert!(!refused(&refusal(503)?));
        assert!(!refused(&WsError::ConnectionClosed.into()));
        assert!(!refused(&anyhow::anyhow!(
            "relay does not support managed authentication"
        )));
        Ok(())
    }

    #[test]
    fn tunnel_request_targets_cannot_replace_the_configured_origin() -> anyhow::Result<()> {
        let base = "http://127.0.0.1:8088";
        assert_eq!(
            request_url(base, "/v1/models?scope=local")?,
            format!("{base}/v1/models?scope=local")
        );
        for path in [
            "@other.example.com/v1/models",
            "https://other.example.com",
            "//other.example.com",
            "v1/models",
            "",
            "/v1/models#ignored",
            "/v1/models\n",
        ] {
            assert!(request_url(base, path).is_err());
        }
        Ok(())
    }
}
