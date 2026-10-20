//! Wiki: docs/wiki/architecture.md
#![forbid(unsafe_code)]
#![deny(warnings, rust_2018_idioms, rust_2021_compatibility)]
#![warn(missing_docs, missing_debug_implementations)]

mod app;
mod auth;
mod config;
mod node;
mod observability;
mod proxy;
mod status;

use std::io::Write as _;

use anyhow::Context;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args()
        .nth(1)
        .is_some_and(|flag| flag == "--version" || flag == "-V")
    {
        writeln!(
            std::io::stdout(),
            "{}",
            nexa_tunnel_protocol::build_info!().describe("nexa-llm-bridge")
        )?;
        return Ok(());
    }
    // reqwest is built with rustls-no-provider — install ring before any client exists.
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("could not install the bridge TLS provider"))?;

    let cfg = config::Config::load().context("load config (env BRIDGE_* / config.toml)")?;
    observability::init(&cfg);

    let state = app::AppState::new(&cfg).context("build app state")?;
    {
        let hist = state.node_history.clone();
        tokio::spawn(async move {
            let mut tick =
                tokio::time::interval(std::time::Duration::from_secs(node::HISTORY_EVERY_SECS));
            loop {
                tick.tick().await;
                hist.sample().await;
            }
        });
    }
    let listener = tokio::net::TcpListener::bind(&cfg.listen_addr)
        .await
        .with_context(|| format!("bind {}", cfg.listen_addr))?;
    tracing::info!(
        listen = %cfg.listen_addr,
        upstream = %cfg.upstream_url,
        auth = state.auth.enabled(),
        "nexa-llm-bridge listening",
    );

    status::spawn_poller(state.clone());
    axum::serve(listener, app::router(state))
        .with_graceful_shutdown(shutdown_signal(cfg.supervisor_pipe))
        .await
        .context("server error")?;
    tracing::info!("nexa-llm-bridge stopped");
    Ok(())
}

/// Resolve on SIGTERM, Ctrl-C or, under a supervisor, the end of stdin, so in-flight requests drain.
async fn shutdown_signal(supervisor_pipe: bool) {
    let ctrl_c = tokio::signal::ctrl_c();
    let pipe = async {
        if supervisor_pipe {
            pipe_closed().await;
        } else {
            std::future::pending::<()>().await;
        }
        Ok::<(), std::io::Error>(())
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?
            .recv()
            .await;
        Ok::<(), std::io::Error>(())
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<Result<(), std::io::Error>>();

    let outcome = tokio::select! {
        result = ctrl_c => result,
        result = terminate => result,
        result = pipe => result,
    };
    if let Err(error) = outcome {
        tracing::error!(%error, "shutdown signal handler failed; draining the server");
        return;
    }
    tracing::info!("shutdown signal received; draining in-flight requests");
}

/// Read stdin until it ends: a supervisor that stops, crashes or is killed closes the pipe.
async fn pipe_closed() {
    use tokio::io::AsyncReadExt;

    let mut stdin = tokio::io::stdin();
    let mut sink = [0u8; 256];
    loop {
        match stdin.read(&mut sink).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}
