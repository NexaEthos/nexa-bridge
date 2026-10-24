//! Wiki: docs/wiki/server-setup.md

mod pairing;
mod pool;
mod qr;
mod service;
mod setup;
mod store;
mod term;

use anyhow::{Context, Result};

use term::Term;

const HELP: &str = "nexa-bridge — link this server's model to your Nexa Agora workspace

Usage:
  nexa-bridge setup [options]   find the model server, link this server, choose the model, start the services
  nexa-bridge status            the link, the services, the model and the workspace's presets here
  nexa-bridge model <id>        serve another exact model id and restart the services
  nexa-bridge pool              the model servers this bridge balances, and whether each answers
  nexa-bridge pool add <url> [--key <api key>] [--max-inflight <n>] [--max-body-bytes <n>]
                                balance the model across another server serving the same exact id
  nexa-bridge pool remove <url> take a model server out of the pool
  nexa-bridge unlink            stop the services and remove this server's credentials
  nexa-bridge uninstall         unlink, then remove the three programs
  nexa-bridge help              this text

Setup options:
  --agora <https://…>       your workspace's Agora address; required to recover an unbound pending proof
  --approval <https://…>    the browser approval address, when it differs from the Agora address
  --name <name>             how this server appears in Agora (default: its hostname)
  --upstream <address>      the model server's address, when it is not on a usual local port
  --runtime <name>          model server to use; otherwise detected or chosen before linking
                            vllm, sglang, ollama, lm-studio, llama-cpp
  --model <exact id>        the model to serve, without asking
  --listen <host:port>      the bridge's local address (default 127.0.0.1:8088)
  --no-service              link and configure only; print how to start the two programs
  --yes                     confirm the workspace without asking
";

#[derive(Debug, Default)]
pub struct Args {
    pub command: String,
    pub value: Option<String>,
    pub agora: Option<String>,
    pub approval: Option<String>,
    pub name: Option<String>,
    pub upstream: Option<String>,
    pub runtime: Option<String>,
    pub model: Option<String>,
    pub listen: Option<String>,
    pub key: Option<String>,
    pub max_inflight: Option<usize>,
    pub max_body_bytes: Option<usize>,
    pub rest: Vec<String>,
    pub no_service: bool,
    pub yes: bool,
}

const COMMANDS: [&str; 7] = [
    "setup",
    "status",
    "model",
    "pool",
    "unlink",
    "uninstall",
    "help",
];

pub fn is_command(first: Option<&str>) -> bool {
    first.is_some_and(|word| COMMANDS.contains(&word) || word == "--help" || word == "-h")
}

pub fn parse(words: &[String]) -> Result<Args> {
    let mut args = Args {
        command: words.first().cloned().unwrap_or_default(),
        ..Args::default()
    };
    if args.command == "--help" || args.command == "-h" {
        args.command = "help".into();
    }
    let mut rest = words.iter().skip(1);
    while let Some(word) = rest.next() {
        let (flag, inline) = word
            .split_once('=')
            .map_or((word.as_str(), None), |(f, v)| (f, Some(v.to_string())));
        let mut value = || -> Result<String> {
            inline
                .clone()
                .or_else(|| rest.next().cloned())
                .with_context(|| format!("{flag} needs a value"))
        };
        match flag {
            "--agora" => args.agora = Some(value()?),
            "--approval" => args.approval = Some(value()?),
            "--name" => args.name = Some(value()?),
            "--upstream" => args.upstream = Some(value()?),
            "--runtime" => args.runtime = Some(value()?),
            "--model" => args.model = Some(value()?),
            "--listen" => args.listen = Some(value()?),
            "--key" => args.key = Some(value()?),
            "--max-inflight" => {
                args.max_inflight =
                    Some(value()?.parse().context("--max-inflight needs a number")?);
            }
            "--max-body-bytes" => {
                args.max_body_bytes = Some(
                    value()?
                        .parse()
                        .context("--max-body-bytes needs a number")?,
                );
            }
            "--no-service" => args.no_service = true,
            "--yes" | "-y" => args.yes = true,
            other if !other.starts_with('-') && args.value.is_none() => {
                args.value = Some(other.to_string());
            }
            other if !other.starts_with('-') => args.rest.push(other.to_string()),
            other => anyhow::bail!("unknown option {other:?}; see `nexa-bridge help`"),
        }
    }
    Ok(args)
}

pub async fn run(words: &[String]) -> Result<()> {
    let args = parse(words)?;
    let term = Term::new();
    match args.command.as_str() {
        "setup" => setup::setup(&term, &args).await,
        "status" => setup::status(&term).await,
        "model" => {
            let id = args
                .value
                .as_deref()
                .context("give the exact model id: nexa-bridge model <id>")?;
            setup::model(&term, id).await
        }
        "pool" => match (args.value.as_deref(), args.rest.first()) {
            (None, _) => setup::pool_list(&term).await,
            (Some("add"), Some(url)) => setup::pool_add(&term, url, &args).await,
            (Some("remove"), Some(url)) => setup::pool_remove(&term, url),
            _ => anyhow::bail!("use `nexa-bridge pool`, `pool add <url>` or `pool remove <url>`"),
        },
        "unlink" => setup::unlink(&term),
        "uninstall" => setup::uninstall(&term),
        _ => term.line(HELP),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn options_read_in_both_forms_and_an_unknown_one_is_refused() -> Result<()> {
        let args = parse(&words(
            "setup --agora=https://agora.example.com --model google/gemma-4-31b-qat --yes",
        ))?;
        assert_eq!(args.command, "setup");
        assert_eq!(args.agora.as_deref(), Some("https://agora.example.com"));
        assert_eq!(args.model.as_deref(), Some("google/gemma-4-31b-qat"));
        assert!(args.yes && !args.no_service);
        assert_eq!(
            parse(&words("model qwen/qwen3.8-27b"))?.value.as_deref(),
            Some("qwen/qwen3.8-27b")
        );
        let pool = parse(&words(
            "pool add http://10.0.0.2:8000 --key k --max-inflight 16",
        ))?;
        assert_eq!(
            (pool.value.as_deref(), pool.rest.first().map(String::as_str)),
            (Some("add"), Some("http://10.0.0.2:8000"))
        );
        assert_eq!(
            (pool.key.as_deref(), pool.max_inflight),
            (Some("k"), Some(16))
        );
        assert!(parse(&words("pool add http://10.0.0.2:8000 --max-inflight many")).is_err());
        assert!(parse(&words("setup --colour")).is_err());
        assert!(parse(&words("setup --model")).is_err());
        Ok(())
    }

    #[test]
    fn the_connector_runs_as_before_without_a_command() {
        assert!(is_command(Some("setup")));
        assert!(is_command(Some("--help")));
        assert!(!is_command(None));
        assert!(!is_command(Some("--version")));
    }
}
