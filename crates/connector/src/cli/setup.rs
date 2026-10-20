//! Link a server to a workspace and keep its model and services in step. Wiki: docs/wiki/server-setup.md

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use nexa_link::agora::{self, PairingEnded};
use nexa_link::guides::WorkspacePreset;
use nexa_link::runtimes::{self, Flavor, Runtime};
use nexa_link::secret;

use super::pool;
use super::service;
use super::store::{self, Paths, Server};
use super::term::Term;
use super::{Args, qr};

const POLL: Duration = Duration::from_secs(5);
const LISTEN: &str = "127.0.0.1:8088";

/// The guide's runtime keys for a detected flavor; `sglang` is reported as another OpenAI-compatible server.
const fn guide_keys(flavor: Flavor) -> &'static [&'static str] {
    match flavor {
        Flavor::LmStudio => &["lm-studio"],
        Flavor::Ollama => &["ollama"],
        Flavor::LlamaCpp => &["llama-cpp"],
        Flavor::Vllm => &["vllm"],
        Flavor::OpenAiCompatible => &["sglang", "open-ai-compatible"],
    }
}

/// A runtime named on the command line: its flavor and its usual loopback address.
fn named_runtime(name: &str) -> Result<(Flavor, &'static str)> {
    Ok(match name {
        "vllm" => (Flavor::Vllm, "http://127.0.0.1:8000"),
        "sglang" => (Flavor::OpenAiCompatible, "http://127.0.0.1:30000"),
        "ollama" => (Flavor::Ollama, "http://127.0.0.1:11434"),
        "lm-studio" => (Flavor::LmStudio, "http://127.0.0.1:1234"),
        "llama-cpp" => (Flavor::LlamaCpp, "http://127.0.0.1:8080"),
        other => anyhow::bail!(
            "unknown runtime {other:?}: one of vllm, sglang, ollama, lm-studio, llama-cpp (or give --upstream)"
        ),
    })
}

fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn hostname() -> Result<String> {
    let out = std::process::Command::new("hostname")
        .output()
        .context("cannot read this server's name: give one with --name")?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    anyhow::ensure!(
        !name.is_empty(),
        "this server has no name: give one with --name"
    );
    Ok(name.chars().take(80).collect())
}

async fn find_runtime(term: &Term, args: &Args) -> Result<Option<Runtime>> {
    if let Some(upstream) = &args.upstream {
        return runtimes::probe_one(upstream)
            .await
            .map(Some)
            .map_err(|e| anyhow::anyhow!("{e}"));
    }
    let found = runtimes::detect().await;
    match found.len() {
        0 => Ok(None),
        1 => Ok(found.into_iter().next()),
        _ => {
            term.line("More than one model server answers on this machine:")?;
            let labels: Vec<String> = found
                .iter()
                .map(|rt| format!("{} at {}", rt.label, rt.url))
                .collect();
            let index = term.choose(&labels, "--upstream <address>")?;
            Ok(found.into_iter().nth(index))
        }
    }
}

async fn pair(
    term: &Term,
    paths: &Paths,
    origin: &str,
    approval: &str,
    label: &str,
) -> Result<(String, agora::DeviceStatus)> {
    let proof = secret::generate()?;
    store::write_private(&paths.pending(), &proof)?;
    let pairing = agora::start_pairing(origin, approval, label, &proof).await?;
    term.blank()?;
    term.line(
        "Link this server to your workspace. A workspace administrator opens, from any device:",
    )?;
    term.line(&format!("  {}", pairing.verification_uri))?;
    term.line(&qr::render(&pairing.verification_uri)?)?;
    term.line(&format!(
        "  code {} · expires in {} minutes",
        pairing.code,
        pairing.expires_at.saturating_sub(now()?) / 60
    ))?;
    term.progress("Waiting for approval")?;
    loop {
        anyhow::ensure!(
            now()? < pairing.expires_at,
            "The approval expired. Run the setup again."
        );
        tokio::time::sleep(POLL).await;
        match agora::device_status(origin, &proof).await {
            Ok(device) if device.status == "pending" => term.tick()?,
            Ok(device) if device.status == "active" => {
                term.done("approved")?;
                return Ok((proof, device));
            }
            Ok(_) => anyhow::bail!("This server was not approved. Run the setup again."),
            Err(error) if error.downcast_ref::<PairingEnded>().is_some() => return Err(error),
            Err(error) => term.note(&format!("Agora did not answer ({error}); still waiting"))?,
        }
    }
}

/// The model this server will serve: a flag, else the person's pick among served ids and Nexa's guided presets.
fn choose_model(
    term: &Term,
    args: &Args,
    runtime: Option<&Runtime>,
    catalog: &[WorkspacePreset],
) -> Result<String> {
    if let Some(model) = &args.model {
        return Ok(model.clone());
    }
    let served: Vec<String> = runtime
        .map(|rt| {
            rt.loaded_models
                .clone()
                .unwrap_or_else(|| rt.models.clone())
        })
        .unwrap_or_default();
    let mut ids: Vec<String> = served.clone();
    ids.extend(
        catalog
            .iter()
            .map(|p| p.model.clone())
            .filter(|id| !served.contains(id)),
    );
    anyhow::ensure!(
        !ids.is_empty(),
        "No model to serve: start your model server, or give the exact id with --model"
    );
    let labels: Vec<String> = ids
        .iter()
        .map(|id| {
            let preset = catalog.iter().find(|p| &p.model == id);
            let status = if served.contains(id) {
                "served now"
            } else {
                "not served yet"
            };
            preset.map_or_else(
                || format!("{id} · {status}"),
                |p| {
                    format!(
                        "{id} · {status} · Nexa preset «{}»{}",
                        p.name,
                        if p.certified { ", certified" } else { "" }
                    )
                },
            )
        })
        .collect();
    term.line("Which model will this server serve? Requests name this exact id.")?;
    let index = term.choose(&labels, "--model <exact id>")?;
    Ok(ids[index].clone())
}

/// Print Nexa's guide for the chosen model and runtime: the window to load, the steps and the flags.
fn show_guide(term: &Term, preset: &WorkspacePreset, flavor: Option<Flavor>) -> Result<()> {
    term.blank()?;
    term.line(&format!(
        "Serve exactly «{}» ({})",
        preset.model, preset.name
    ))?;
    if let Some(window) = preset.context_window {
        term.line(&format!("  context window to load: {window} tokens"))?;
    }
    let Some(guide) = &preset.guide else {
        return Ok(());
    };
    if let Some(notes) = &guide.notes {
        term.line(&format!("  {notes}"))?;
    }
    let keys: Vec<&str> = flavor.map_or_else(
        || guide.runtimes.keys().map(String::as_str).collect(),
        |f| guide_keys(f).to_vec(),
    );
    for key in keys {
        let Some(runtime) = guide.runtimes.get(key) else {
            continue;
        };
        term.line(&format!("  {key}:"))?;
        for (n, step) in runtime.steps.iter().enumerate() {
            term.line(&format!("    {}. {step}", n + 1))?;
        }
        if !runtime.args.is_empty() {
            term.line(&format!("    flags: {}", runtime.args.join(" ")))?;
        }
    }
    Ok(())
}

fn write_envs(paths: &Paths, server: &Server, proof: &str, local_key: &str) -> Result<()> {
    let mut bridge = BTreeMap::new();
    bridge.insert("BRIDGE_UPSTREAM_URL", server.upstream_url.clone());
    bridge.insert("BRIDGE_SELECTED_MODEL", server.selected_model.clone());
    bridge.insert("BRIDGE_LISTEN_ADDR", server.listen_addr.clone());
    bridge.insert("BRIDGE_API_KEY", local_key.to_string());
    bridge.insert("BRIDGE_LOG_FORMAT", "json".to_string());
    store::write_private(&paths.bridge_env(), &store::encode_env(&bridge))?;
    let mut connector = BTreeMap::new();
    connector.insert("CONNECTOR_RELAY_URL", server.relay_url.clone());
    connector.insert("CONNECTOR_TUNNEL_TOKEN", proof.to_string());
    connector.insert("CONNECTOR_MANAGED_AUTH", "true".to_string());
    connector.insert(
        "CONNECTOR_UPSTREAM_URL",
        format!("http://{}", server.listen_addr),
    );
    connector.insert("CONNECTOR_STATUS_API_KEY", local_key.to_string());
    connector.insert("CONNECTOR_LOG_FORMAT", "json".to_string());
    store::write_private(&paths.connector_env(), &store::encode_env(&connector))
}

/// The device proof and the local key, read back from the owner-only env files.
fn secrets(paths: &Paths) -> Result<(String, String)> {
    let text = store::read_private(&paths.connector_env())?
        .context("connector.env is missing; unlink and set this server up again")?;
    let vars = store::decode_env(&text);
    let proof = vars
        .get("CONNECTOR_TUNNEL_TOKEN")
        .cloned()
        .context("the device proof is missing")?;
    let key = vars
        .get("CONNECTOR_STATUS_API_KEY")
        .cloned()
        .context("the local key is missing")?;
    Ok((proof, key))
}

fn flavor_of(server: &Server) -> Result<Flavor> {
    if server.runtime == "open-ai-compatible" {
        return Ok(Flavor::OpenAiCompatible);
    }
    Ok(named_runtime(&server.runtime)?.0)
}

const fn runtime_name(flavor: Flavor) -> &'static str {
    match flavor {
        Flavor::Vllm => "vllm",
        Flavor::Ollama => "ollama",
        Flavor::LmStudio => "lm-studio",
        Flavor::LlamaCpp => "llama-cpp",
        Flavor::OpenAiCompatible => "open-ai-compatible",
    }
}

async fn publish(server: &Server, proof: &str) -> Result<()> {
    let flavor = flavor_of(server)?;
    let window = if flavor == Flavor::LmStudio {
        runtimes::lmstudio_loaded_context(&server.upstream_url, &server.selected_model)
            .await
            .ok()
    } else {
        None
    };
    agora::publish_model(
        &server.agora_url,
        proof,
        &server.selected_model,
        flavor,
        window,
    )
    .await
}

/// The directory the running program was installed in, where the services' units point.
pub(super) fn bin_dir() -> Result<std::path::PathBuf> {
    Ok(std::env::current_exe()?
        .parent()
        .context("cannot locate the installed programs")?
        .to_path_buf())
}

/// A linked server running setup again is an update: the services start again on the programs just installed.
fn update(term: &Term, paths: &Paths, server: &Server) -> Result<()> {
    let version = env!("CARGO_PKG_VERSION");
    if service::installed() {
        service::install(paths, &bin_dir()?)?;
        service::restart()?;
        term.done(&format!(
            "updated: the services run {version} for workspace «{}»",
            server.tenant_slug
        ))?;
    } else {
        term.done(&format!(
            "updated to {version}, linked to workspace «{}»: start the two programs again to run it",
            server.tenant_slug
        ))?;
    }
    term.note("`nexa-bridge status` shows the services; `nexa-bridge model <id>` changes the model")
}

/// `nexa-bridge setup`: find the model server, link the server, choose the model, start the services.
pub async fn setup(term: &Term, args: &Args) -> Result<()> {
    let paths = Paths::resolve()?;
    if let Some(server) = Server::load(&paths)? {
        return update(term, &paths, &server);
    }
    let origin = agora::origin(
        args.agora
            .as_deref()
            .or(option_env!("NEXA_AGORA_URL"))
            .context(
                "This build names no Agora: give your workspace's address with --agora https://…",
            )?,
    )?;
    let approval = agora::origin(args.approval.as_deref().unwrap_or(&origin))?;
    let runtime = find_runtime(term, args).await?;
    match &runtime {
        Some(rt) => term.done(&format!("found {} at {}", rt.label, rt.url))?,
        None => term.line("No model server answers yet: you can start it after this setup.")?,
    }
    let label = args.name.clone().map_or_else(hostname, Ok)?;
    let (proof, device) = pair(term, &paths, &origin, &approval, &label).await?;
    let tenant = device
        .tenant_slug
        .context("the approved server has no workspace")?;
    if !term.confirm(
        &format!("Link this server to workspace «{tenant}»?"),
        args.yes,
    )? {
        anyhow::bail!(
            "Not linked. Revoke the pending server in Agora if it was approved by mistake."
        );
    }
    let catalog = agora::catalog(&origin, &proof)
        .await
        .unwrap_or_else(|error| {
            let _ = term.note(&format!(
                "Nexa's catalog is unavailable ({error}); choose among the served models"
            ));
            Vec::new()
        });
    let model = choose_model(term, args, runtime.as_ref(), &catalog)?;
    let (flavor, upstream) = match (&runtime, &args.runtime) {
        (Some(rt), _) => (rt.flavor, rt.url.clone()),
        (None, Some(name)) => {
            let (flavor, url) = named_runtime(name)?;
            (
                flavor,
                args.upstream.clone().unwrap_or_else(|| url.to_string()),
            )
        }
        (None, None) => anyhow::bail!(
            "Name the model server you will start with --runtime vllm|sglang|ollama|lm-studio|llama-cpp"
        ),
    };
    if let Some(preset) = catalog.iter().find(|p| p.model == model) {
        show_guide(term, preset, Some(flavor))?;
    }
    let server = Server {
        agora_url: origin,
        approval_url: approval,
        device_id: device.id,
        tenant_slug: tenant,
        connection_slug: device
            .connection_slug
            .context("the approved server has no connection")?,
        relay_url: device
            .relay_url
            .context("the approved server has no relay")?,
        upstream_url: upstream,
        runtime: runtime_name(flavor).to_string(),
        selected_model: model,
        listen_addr: args.listen.clone().unwrap_or_else(|| LISTEN.to_string()),
    };
    let local_key = secret::generate()?;
    write_envs(&paths, &server, &proof, &local_key)?;
    server.save(&paths)?;
    std::fs::remove_file(paths.pending()).context("cannot remove the pending proof")?;
    publish(&server, &proof).await?;
    term.done(&format!(
        "serving «{}» for workspace «{}»",
        server.selected_model, server.tenant_slug
    ))?;
    finish(term, args, &paths, &server)
}

fn finish(term: &Term, args: &Args, paths: &Paths, server: &Server) -> Result<()> {
    if args.no_service {
        term.line("Start the two programs with their env files:")?;
        term.line(&format!(
            "  env $(cat {}) nexa-llm-bridge",
            paths.bridge_env().display()
        ))?;
        term.line(&format!(
            "  env $(cat {}) nexa-llm-connector",
            paths.connector_env().display()
        ))?;
    } else {
        service::install(paths, &bin_dir()?)?;
        term.done("started as user services (nexa-bridge, nexa-bridge-connector)")?;
        if !service::lingers() {
            match service::enable_linger() {
                Ok(()) => term.done("kept running after logout and at boot")?,
                Err(error) => term.note(&format!(
                    "{error}: ask an administrator to run `sudo loginctl enable-linger $USER`, or the services stop when you log out"
                ))?,
            }
        }
    }
    term.blank()?;
    term.line(&format!(
        "Next: a workspace administrator chooses the Nexa preset this server serves in Agora › Settings › Nexa Bridge («{}»).",
        server.connection_slug
    ))?;
    term.line("Check it any time with `nexa-bridge status`.")
}

/// `nexa-bridge status`: the link at Agora, the services, the model, and the workspace's presets on this server.
pub async fn status(term: &Term) -> Result<()> {
    let paths = Paths::resolve()?;
    let server =
        Server::load(&paths)?.context("This server is not linked yet: run `nexa-bridge setup`.")?;
    let (proof, _) = secrets(&paths)?;
    let device = agora::device_status(&server.agora_url, &proof).await?;
    term.line(&format!(
        "workspace   {} · bridge {}",
        server.tenant_slug, server.connection_slug
    ))?;
    term.line(&format!(
        "device      {} ({})",
        server.device_id, device.status
    ))?;
    term.line(&format!(
        "model       {} on {} ({})",
        server.selected_model, server.upstream_url, server.runtime
    ))?;
    let answering = runtimes::probe_one(&server.upstream_url).await.ok();
    let live = answering.as_ref().is_some_and(|rt| {
        rt.loaded_models
            .as_ref()
            .unwrap_or(&rt.models)
            .contains(&server.selected_model)
    });
    term.line(&format!(
        "model live  {}",
        if live {
            "yes"
        } else {
            "no — start the model server with the exact id"
        }
    ))?;
    let members = pool::load(&paths)?;
    if !members.is_empty() {
        term.line(&format!(
            "pool        {} more model server(s) serve it: `nexa-bridge pool` shows each",
            members.len()
        ))?;
    }
    if cfg!(target_os = "linux") {
        term.line(&format!(
            "services    bridge {} · connector {}",
            service::state(service::BRIDGE_UNIT),
            service::state(service::CONNECTOR_UNIT)
        ))?;
    }
    let presets = agora::guides(&server.agora_url, &proof).await?;
    if presets.is_empty() {
        return term
            .line("presets     none chosen for this server yet (Agora › Settings › Nexa Bridge)");
    }
    for preset in &presets {
        let mark = if preset.model == server.selected_model {
            "served"
        } else {
            "NOT served here"
        };
        term.line(&format!(
            "preset      {} · «{}» · {mark}",
            preset.name, preset.model
        ))?;
    }
    Ok(())
}

/// `nexa-bridge model <id>`: serve another exact id, tell the workspace, restart the services.
pub async fn model(term: &Term, id: &str) -> Result<()> {
    anyhow::ensure!(
        !id.is_empty() && id.trim() == id && !id.chars().any(char::is_control),
        "give the exact model id, without surrounding spaces"
    );
    let paths = Paths::resolve()?;
    let mut server =
        Server::load(&paths)?.context("This server is not linked yet: run `nexa-bridge setup`.")?;
    let (proof, key) = secrets(&paths)?;
    server.selected_model = id.to_string();
    write_envs(&paths, &server, &proof, &key)?;
    let members = pool::load(&paths)?;
    pool::write_config(&paths, &server, &members)?;
    server.save(&paths)?;
    publish(&server, &proof).await?;
    if cfg!(target_os = "linux") {
        service::restart()?;
    }
    term.done(&format!("serving «{id}»"))?;
    if !members.is_empty() {
        term.note(&format!(
            "every model server of the pool must now serve «{id}» too: `nexa-bridge pool` shows which answer"
        ))?;
    }
    if let Some(preset) = agora::catalog(&server.agora_url, &proof)
        .await?
        .iter()
        .find(|p| p.model == id)
    {
        show_guide(term, preset, flavor_of(&server).ok())?;
    }
    Ok(())
}

/// Rewrite the units and restart the services so the bridge reads its pool again.
fn reload_services(paths: &Paths) -> Result<()> {
    if service::installed() {
        service::install(paths, &bin_dir()?)?;
        service::restart()?;
    }
    Ok(())
}

/// `nexa-bridge pool`: this server's model server and every member, with what the bridge sees of each.
pub async fn pool_list(term: &Term) -> Result<()> {
    let paths = Paths::resolve()?;
    let server =
        Server::load(&paths)?.context("This server is not linked yet: run `nexa-bridge setup`.")?;
    let members = pool::load(&paths)?;
    term.line(&format!(
        "pool        «{}» on {} model server(s), balanced by this bridge",
        server.selected_model,
        members.len() + 1
    ))?;
    let (_, key) = secrets(&paths)?;
    let seen = pool::health(&server, &key).await.unwrap_or_default();
    let answer = |url: &str| pool::describe(seen.iter().find(|row| row.url == url));
    term.line(&format!(
        "  {}  this server · {}",
        server.upstream_url,
        answer(&server.upstream_url)
    ))?;
    for member in &members {
        let key = if member.api_key.is_some() {
            "key set"
        } else {
            "no key"
        };
        term.line(&format!(
            "  {}  {key} · {}",
            member.url,
            answer(&member.url)
        ))?;
    }
    if members.is_empty() {
        term.note("alone for now: `nexa-bridge pool add http://<host>:<port>` balances it across another server")?;
    }
    Ok(())
}

/// `nexa-bridge pool add <url>`: balance the selected model across another model server serving the same id.
pub async fn pool_add(term: &Term, raw: &str, args: &Args) -> Result<()> {
    let paths = Paths::resolve()?;
    let server =
        Server::load(&paths)?.context("This server is not linked yet: run `nexa-bridge setup`.")?;
    let url = pool::base_url(raw)?;
    match pool::served(&url, args.key.as_deref()).await? {
        Some(models) => anyhow::ensure!(
            models.contains(&server.selected_model),
            "{url} serves {models:?}, not «{}»: start it with that exact id (vLLM: --served-model-name)",
            server.selected_model
        ),
        None => term.note(&format!(
            "{url} does not answer now: it joins the pool, and the bridge sends it work once it answers"
        ))?,
    }
    let member = pool::Member {
        url: url.clone(),
        api_key: args.key.clone(),
        max_inflight: args.max_inflight,
        max_body_bytes: args.max_body_bytes,
    };
    let members = pool::add(&paths, &server, member)?;
    reload_services(&paths)?;
    term.done(&format!(
        "{url} joined the pool: «{}» is balanced across {} model servers",
        server.selected_model,
        members.len() + 1
    ))
}

/// `nexa-bridge pool remove <url>`: take a model server out of the pool.
pub fn pool_remove(term: &Term, raw: &str) -> Result<()> {
    let paths = Paths::resolve()?;
    let server =
        Server::load(&paths)?.context("This server is not linked yet: run `nexa-bridge setup`.")?;
    let url = pool::base_url(raw)?;
    let members = pool::remove(&paths, &server, &url)?;
    reload_services(&paths)?;
    term.done(&format!(
        "{url} left the pool: {} model server(s) serve «{}»",
        members.len() + 1,
        server.selected_model
    ))
}

/// `nexa-bridge uninstall`: unlink, then remove the three programs from where they were installed.
pub fn uninstall(term: &Term) -> Result<()> {
    unlink(term)?;
    let dir = bin_dir()?;
    for program in ["nexa-bridge", "nexa-llm-bridge", "nexa-llm-connector"] {
        match std::fs::remove_file(dir.join(program)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(e)
                    .with_context(|| format!("cannot remove {}", dir.join(program).display()));
            }
            _ => {}
        }
    }
    term.done(&format!("programs removed from {}", dir.display()))
}

/// `nexa-bridge unlink`: stop the services and forget the link here; the workspace revokes the server in Agora.
pub fn unlink(term: &Term) -> Result<()> {
    let paths = Paths::resolve()?;
    let server = Server::load(&paths)?;
    service::remove()?;
    if paths.dir.exists() {
        std::fs::remove_dir_all(&paths.dir)
            .with_context(|| format!("cannot remove {}", paths.dir.display()))?;
    }
    term.done("services stopped and local credentials removed")?;
    if let Some(server) = server {
        term.line(&format!(
            "Revoke this server in Agora › Settings › Bridge (workspace «{}», device {}).",
            server.tenant_slug, server.device_id
        ))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_named_runtime_has_its_usual_address_and_guide_keys() -> Result<()> {
        assert_eq!(
            named_runtime("vllm")?,
            (Flavor::Vllm, "http://127.0.0.1:8000")
        );
        assert_eq!(named_runtime("sglang")?.0, Flavor::OpenAiCompatible);
        assert!(named_runtime("tgi").is_err());
        assert_eq!(
            guide_keys(Flavor::OpenAiCompatible),
            &["sglang", "open-ai-compatible"]
        );
        for flavor in [
            Flavor::Vllm,
            Flavor::Ollama,
            Flavor::LmStudio,
            Flavor::LlamaCpp,
        ] {
            assert_eq!(named_runtime(runtime_name(flavor))?.0, flavor);
        }
        Ok(())
    }
}
