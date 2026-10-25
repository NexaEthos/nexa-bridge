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

1. **Finds or chooses the model server before linking.** It probes the usual local ports (11434, 1234, 8080, 8000,
   30000), or the address named by `--upstream` or `--runtime`. An explicit runtime uses its own usual address,
   not another runtime that happens to answer. When none answers, the terminal asks which runtime you will start.
   Without a terminal, supply `--runtime`; an unanswered choice stops before creating a pairing. A custom upstream
   may be offline, but must be a credential-free HTTP(S) origin. Pasted `/v1` suffixes are removed.
2. **Links or resumes the server.** It generates the device proof on the server, sends Agora only its SHA-256, and prints the
   approval link, its code and a QR code. A workspace administrator approves; the code expires in ten minutes and
   works once. The setup then names the approved workspace and asks to confirm it (`--yes` answers).
3. **Chooses the model.** The choice lists the ids the model server serves now; the id chosen is the exact id requests
   will name. The setup then names Nexa's operating presets (`GET /device/catalog`); a workspace administrator
   chooses one for this server in Agora, and `nexa-bridge model <id>` prints the guides of the presets chosen for it:
   the window they declare, the steps, the flags Nexa wrote.
4. **Tells the workspace** which id this server is configured to serve (`PUT /device/model`), as the desktop does at connect.
5. **Starts the services** (Linux): `nexa-bridge.service` and `nexa-bridge-connector.service` as systemd user units,
   enabled at boot, and asks systemd to keep them running after logout (`loginctl enable-linger`). When that needs an
   administrator, the setup says so and how.

The model server may start before or after the setup. Started after, with the guide's flags, it is served as soon as
it answers: the bridge lists the selected id only while the model server reports it, and nothing needs running again.
Setup says **configured**, not **serving**: use `nexa-bridge status` to check the model and the services. It does not
install or start vLLM, SGLang, Ollama, LM Studio or llama.cpp for you. SGLang is recorded as OpenAI-compatible and
uses the SGLang or generic OpenAI-compatible guide provided by the selected preset.

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

## Resuming an interrupted setup

Setup saves its proof together with the Agora and approval origins before requesting enrollment. After Agora
returns the approval link, it saves that link too. Running setup again keeps the same device and checks its status:

- An approved device resumes even after the original ten-minute approval window. That expiry applies only to an
  unapproved request; it is not a credential lifetime.
- A pending request shows the original approval page again. If an interrupted request never returned a link,
  complete the original browser page if available, or wait for that request to expire before retrying.
- A network or service error retains the proof. A definitive authorization/expiry refusal clears the unusable
  pending state and tells you to run setup again. No failed request silently creates a replacement device.
- A different Agora or approval origin cannot reuse the saved proof. Resume with the original addresses; to
  deliberately change workspaces, revoke the old device in Agora, then use `nexa-bridge unlink` and set up again.
- The pending record remains until model publication and local configuration finish. If either publication or
  service installation fails after `server.json` is written, setup retries completion with the same device and
  credential, rather than treating the incomplete setup as a successful software update.

### Recovering a proof left by version 1.0.0

Version 1.0.0 could stop after browser approval when no model server answered and no runtime was specified. Its
`pending-proof` file has no origin metadata. With a build containing the recovery fix, explicitly supply the **same
Agora origin where you approved the device**:

```sh
nexa-bridge setup --agora https://agora.nexaethos.com --runtime vllm
```

For another deployment use its original HTTPS origin, and its original `--approval` when different. The recovery
reads the existing proof locally and queries that origin; only an `active` device is adopted. An unapproved proof
is retained so you can finish the original approval. A refusal also retains the unbound proof: the supplied origin
may be wrong. Check the original address first; if it is correct, use `nexa-bridge unlink` and set up again.
Never paste the proof into a command, browser, support ticket or chat. Do not repeat the old
installer to recover it: the old setup replaces the proof with a new pairing.

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
- `pending-pairing.json` — the proof, its Agora/approval origins and the approval link when received; retained until
  setup completes, including after browser approval;
- `pending-proof` — the unbound proof left by an interrupted 1.0.0 setup; imported only through explicit-origin recovery;
- `pool.json` and `config.toml` — only with a pool: its members with their keys, and the bridge's routes written from
  them ([#pool](#pool)).

The proof never appears on a command line, in the shell's history or in its environment; systemd reads it from the
owner-only file. The units run with `NoNewPrivileges=true`.
Environment files quote and escape every value for systemd's `EnvironmentFile` format. Local commands decode the
same format when reusing the device proof and local key. A missing file means absent state; unreadable or malformed
state is an error. Writes flush to a temporary file and rename it into place. Unlink removes the local state and
services but does not revoke the cloud device; use Agora to revoke it.

The command parser accepts both `--flag value` and `--flag=value`; positional arguments after a pool action select
its member. Without a CLI command, the executable keeps its connector behavior. Installed service paths are taken
from the running executable's directory. Pool mutations reinstall existing units and restart the services.

## Elsewhere than Linux

The services are systemd units. On another system `--no-service` links and configures the server and prints how to
start the two programs with their files; the desktop app is the way on a personal computer.

## Changelog

- 2026-10-25 — the model choice lists the served ids alone; the setup names Nexa's operating presets, and the guides follow the presets the workspace chose, never a match on the model id.
- 2026-10-24: Choose the runtime before enrollment, resume origin-bound pairing and incomplete setup, recover
  approved 1.0.0 proofs with an explicit origin, and distinguish configuration from live model availability.

- 2026-10-19: `nexa-bridge pool` balances the selected model across several model servers, and
  `nexa-bridge uninstall` (also `install.sh --uninstall`) removes the programs after unlinking.

- 2026-10-19: Running the installer again updates a linked server: the services restart on the new programs and
  setup succeeds (found testing 1.0.0-2 on the lab server).
- 2026-10-19: First version: the installer with checksums, `nexa-bridge setup|status|model|unlink` in the
  connector, the device catalog of Nexa's guided presets, systemd user services. Replaces the hand-written token
  setup for Agora.
