//! nexa-robot — telemetry + teleobservation. No actuation in V1. See docs/wiki/robot.md
#![forbid(unsafe_code)]
#![deny(warnings, rust_2018_idioms, rust_2021_compatibility)]
#![warn(missing_docs, missing_debug_implementations)]

#[cfg(unix)]
use nexa_robot::hardware;

use nexa_robot::config::Config;
use std::io::Write as _;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [command] = args.as_slice()
        && matches!(command.as_str(), "--help" | "-h")
    {
        writeln!(
            std::io::stdout(),
            "nexa-robot: read-only hardware observations\n\ninspect-uno-q <ADB-serial>\ninspect-ros-battery\npair-uno-q <private-state-file> <API-origin> <approval-origin> <ADB-serial> <asset>\npair-ros-battery <private-state-file> <API-origin> <approval-origin> <asset>\nobserve <state-file> <expected-workspace-slug>\n\nPairing and observe are available on Unix only. Pairing requires browser approval.\nObserve sends one observation or retries its pending event.\nROS/Zenoh service mode is not implemented. See docs/wiki/robot.md."
        )?;
        return Ok(());
    }
    #[cfg(unix)]
    if args.first().is_some_and(|command| {
        matches!(
            command.as_str(),
            "pair-uno-q" | "pair-ros-battery" | "observe"
        )
    }) {
        hardware_command(&args).await?;
        return Ok(());
    }
    if let [command] = args.as_slice()
        && command == "inspect-ros-battery"
    {
        let snapshot = nexa_robot::ros::inspect_battery().await?;
        serde_json::to_writer_pretty(std::io::stdout(), &snapshot)?;
        writeln!(std::io::stdout())?;
        return Ok(());
    }
    if let [command, serial] = args.as_slice()
        && command == "inspect-uno-q"
    {
        let snapshot = nexa_robot::board::inspect_uno_q(serial).await?;
        serde_json::to_writer_pretty(std::io::stdout(), &snapshot)?;
        writeln!(std::io::stdout())?;
        return Ok(());
    }
    anyhow::ensure!(
        args.is_empty(),
        "Unknown command or arguments; run nexa-robot --help for supported commands"
    );
    let cfg = Config::load()?;
    init_tracing(&cfg.log_format);

    // Fail-closed: an empty allowlist means the module would serve nothing.
    if cfg.topics.is_empty() {
        anyhow::bail!(
            "nexa-robot: the topic allowlist is EMPTY — this module serves nothing by design \
             (fail-closed). Add `topics = [\"/rover/...\"]` to robot.toml."
        );
    }
    tracing::info!(
        zenoh = %cfg.zenoh_endpoint,
        foxglove = %cfg.foxglove_listen,
        topics = cfg.topics.len(),
        "nexa-robot plan resolved"
    );

    // Exit loudly either way: a misconfigured unit must never look "up" while serving nothing.
    #[cfg(feature = "zenoh-transport")]
    {
        anyhow::bail!("zenoh transport: wiring lands with the live rover");
    }
    #[cfg(not(feature = "zenoh-transport"))]
    {
        anyhow::bail!(
            "nexa-robot was built WITHOUT the `zenoh-transport` feature — this is the scaffold \
             build (config + allowlist + foxglove encoders are real and tested; the transport \
             lands when the rover is reachable). Nothing is being served."
        );
    }
}

#[cfg(unix)]
async fn hardware_command(args: &[String]) -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("Could not initialize hardware TLS"))?;
    let pairing = match args {
        [command, path, api, approval, serial, asset] if command == "pair-uno-q" => {
            hardware::pair(
                std::path::Path::new(path),
                api,
                approval,
                hardware::Source::UnoQ {
                    serial: serial.clone(),
                },
                asset,
            )
            .await?
        }
        [command, path, api, approval, asset] if command == "pair-ros-battery" => {
            hardware::pair(
                std::path::Path::new(path),
                api,
                approval,
                hardware::Source::RosBattery,
                asset,
            )
            .await?
        }
        [command, path, workspace] if command == "observe" => {
            let receipt = hardware::observe(std::path::Path::new(path), workspace).await?;
            writeln!(
                std::io::stdout(),
                "Observation {}: {} (journal {}, received {})",
                receipt.event_id,
                receipt.status,
                receipt.journal_id,
                receipt.received_at
            )?;
            return Ok(());
        }
        _ => anyhow::bail!("Invalid hardware arguments; run nexa-robot --help"),
    };
    writeln!(
        std::io::stdout(),
        "Approve this hardware device in your browser: {}",
        pairing.verification_uri
    )?;
    writeln!(
        std::io::stdout(),
        "Approval expires in {} seconds. After approval, use observe with this state file and the approved workspace slug.",
        pairing.expires_in
    )?;
    Ok(())
}

fn init_tracing(format: &str) {
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{EnvFilter, fmt};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if format == "json" {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().json())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer())
            .init();
    }
}
