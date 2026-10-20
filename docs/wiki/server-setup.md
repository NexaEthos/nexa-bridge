# Server setup — one command over SSH

A server with a model on it (a GPU box running vLLM, SGLang, Ollama, LM Studio or llama.cpp) links to a Nexa Agora
workspace with one command in its shell. No browser runs on the server: a workspace administrator approves it from
any phone or computer.

```sh
curl -fsSL https://raw.githubusercontent.com/NexaEthos/nexa-bridge/main/install.sh | sh
```

Options after `sh -s --` reach the setup, for example `| sh -s -- --model google/gemma-4-31b-qat --yes`.

## What the installer does

[`install.sh`](../../install.sh) is POSIX `sh` and needs no root:

1. picks the build for the machine — Linux x86_64 or aarch64 (static musl), macOS arm64 or x86_64;
2. downloads `nexa-llm-bridge`, `nexa-llm-connector` and the release's `SHA256SUMS` over HTTPS, and installs nothing
   unless both programs match their checksums;
3. installs them in `~/.local/bin` (`NEXA_BRIDGE_BIN_DIR` changes it), with `nexa-bridge`, a two-line script that runs
   the connector's commands;
4. starts `nexa-bridge setup` on the terminal (`/dev/tty`), since the script's own input is the download.

`NEXA_BRIDGE_VERSION=X.Y.Z` installs that release instead of the latest; `NEXA_BRIDGE_FROM=<directory>` installs from
files already on the machine, named and checked as in a release, for a build that is not published.

**Updating** is running the installer again. On a server already linked, setup rewrites the services' units for the
programs just installed and restarts them, then says the version they run; without the services (`--no-service`) it
says to start the two programs again. The link, the model and the secrets stay as they were. Before 1.0.0-3 the
installer replaced the programs but setup refused a linked server: the old version kept running, and the update
ended with an error.

## What the setup does

`nexa-bridge setup` runs in this order; each step says what it found:

1. **Finds the model server** on the usual local ports (11434, 1234, 8080, 8000, 30000) or at `--upstream`. When none
   answers yet, the setup goes on: the server can start afterwards.
2. **Links the server.** It generates the device proof on the server, sends Agora only its SHA-256, and prints the
   approval link, its code and a QR code. A workspace administrator approves; the code expires in ten minutes and
   works once. The setup then names the approved workspace and asks to confirm it (`--yes` answers).
3. **Chooses the model.** The choice lists the ids the model server serves and Nexa's presets that carry a local
   guide (`GET /device/catalog`), each marked served now or not yet. The id chosen is the exact id requests will name.
   For one of Nexa's presets the setup prints its guide for this runtime: the window to load, the steps, the flags —
   for vLLM, `--served-model-name <id> --max-model-len <window>` and the rest Nexa wrote.
4. **Tells the workspace** which id this server serves (`PUT /device/model`), as the desktop does at connect.
5. **Starts the services** (Linux): `nexa-bridge.service` and `nexa-bridge-connector.service` as systemd user units,
   enabled at boot, and asks systemd to keep them running after logout (`loginctl enable-linger`). When that needs an
   administrator, the setup says so and how.

The model server may start before or after the setup. Started after, with the guide's flags, it is served as soon as
it answers: the bridge lists the selected id only while the model server reports it, and nothing needs running again.

Linking the server does not choose its models for the workspace: an administrator picks Nexa's presets on this
bridge in Agora's model settings (the bridge is named by its connection), as for a desktop.

## Commands

| Command | What it does |
| --- | --- |
| `nexa-bridge setup [options]` | the steps above; on a server already linked, the update: the services restart on the programs installed |
| `nexa-bridge status` | the workspace and bridge, the device's standing at Agora, the model and whether the model server serves it, the services, and the workspace's presets on this bridge marked served or not |
| `nexa-bridge model <id>` | serve another exact id: rewrite the bridge's settings, tell the workspace, restart the services, print the guide when Nexa has one |
| `nexa-bridge pool` | the model servers this bridge balances the selected model across, and whether each answers as the running bridge sees it ([#pool](#pool)) |
| `nexa-bridge pool add <url> [--key <api key>] [--max-inflight <n>] [--max-body-bytes <n>]` | balance the model across another model server serving the same exact id; the services restart on the new pool |
| `nexa-bridge pool remove <url>` | take a model server out of the pool; the services restart |
| `nexa-bridge unlink` | stop and remove the services and this server's files; the server stays approved until revoked in Agora, which the command reminds |
| `nexa-bridge uninstall` | unlink, then remove the three programs; also `curl -fsSL …/install.sh \| sh -s -- --uninstall` |

Setup options: `--agora`, `--approval` (when the approval page has another origin), `--name` (default: the hostname),
`--upstream`, `--runtime` (`vllm`, `sglang`, `ollama`, `lm-studio`, `llama-cpp`: the server to start later),
`--model`, `--listen` (default `127.0.0.1:8088`), `--no-service`, `--yes`. Without a terminal no question is asked: a
choice the flags do not settle stops with the flag that settles it.

## Pool {#pool}

One linked bridge balances its selected model across several model servers serving the **same exact id**: this
server's own (`upstream_url`) and every member added with `nexa-bridge pool add`, each reachable from this server.
Before adding a member the command reads its `/v1/models` with the key given: a server listing another id is refused
with the id to serve (vLLM: `--served-model-name`), a refused key is refused, a server that does not answer now joins
and gets work once it answers. `--max-inflight` gives the member's concurrency (e.g. vLLM `--max-num-seqs`) and
`--max-body-bytes` its context budget, both read by the selection ([load-balancing.md](load-balancing.md)).

The members live in `pool.json` (owner-only, with their keys). From it the command writes the bridge's `config.toml`
in the same directory, one `[[upstreams]]` route per model server with the selected id as its only model, and the
bridge unit runs there (`WorkingDirectory`), so the bridge reads the routes at start; the environment keeps every
other setting. `nexa-bridge model <id>` rewrites the routes for the new id, and every member must then serve it too.
A member with a key reaches the bridge's status probe and its proxy with that key; nothing else leaves the server.
`nexa-bridge status` counts the members, and `nexa-bridge pool` shows each one's health from the bridge's own
`/status`.

## Files and secrets

Everything lives in `$XDG_CONFIG_HOME/nexa-bridge` (default `~/.config/nexa-bridge`, created 0700), written
owner-only (0600) through a temporary file renamed into place:

- `server.json` — the link without a secret: Agora, workspace, connection, device id, relay, model, runtime;
- `bridge.env` — the bridge's settings with the local key between connector and bridge;
- `connector.env` — the relay, the device proof as the tunnel bearer, managed authentication on, the local key;
- `pending-proof` — only while an approval is pending;
- `pool.json` and `config.toml` — only with a pool: its members with their keys, and the bridge's routes written from
  them ([#pool](#pool)).

The proof never appears on a command line, in the shell's history or in its environment; systemd reads it from the
owner-only file. The units run with `NoNewPrivileges=true`.

## Elsewhere than Linux

The services are systemd units. On another system `--no-service` links and configures the server and prints how to
start the two programs with their files; the desktop app is the way on a personal computer.

## Changelog

- 2026-10-19: `nexa-bridge pool` balances the selected model across several model servers, and
  `nexa-bridge uninstall` (also `install.sh --uninstall`) removes the programs after unlinking.

- 2026-10-19: Running the installer again updates a linked server: the services restart on the new programs and
  setup succeeds (found testing 1.0.0-2 on the lab server).
- 2026-10-19: First version: the installer with checksums, `nexa-bridge setup|status|model|unlink` in the
  connector, the device catalog of Nexa's guided presets, systemd user services. Replaces the hand-written token
  setup for Agora.
