# Release review — the contract and the desktop's operational safety

A targeted read of the code before the first public release: pairing, credentials, sidecar
supervision, stop and restart, updates, logging, and the compatibility of the wire contract with
desktops that will stay installed after the relay moves on. Findings are classified **technical**
(fixed in a pull request without changing what a user sees or a policy) or **product** (a behaviour
or a policy the owner decides first). Line numbers are not given on purpose; the symbols are.

## Current verification status

The original findings below are a review snapshot, not a list of defects still present in the
current implementation. The source references in this table distinguish implemented fixes from
remaining decisions. Source inspection alone does not establish hardware or platform acceptance.

| Original finding | Current evidence | Remaining verification or decision |
| --- | --- | --- |
| 1 — sidecar lifetime | Both sidecars watch supervisor stdin; `supervisor::terminate` sends SIGTERM on Unix before a delayed kill. | Exercise abrupt desktop death and cleanup on each supported OS; Windows has a different termination path. |
| 2 — reconnect | `connector::run`, `next_wait` and `jittered` implement exponential delay and jitter. | Authentication rejection is still handled by the reconnect loop; terminal relink behavior remains a product decision. |
| 3 — loopback ownership | Each bridge spawn gets a fresh `x-nexa-instance`; exit invalidates verification and stops the dependent connector. Stale health responses cannot verify a replacement. | Pure lifecycle checks cover stale replies; packaged restart and port-reuse acceptance remain required. Automatic port selection is not implemented. |
| 6 — relay comparison | `managed::same_relay` compares parsed URLs. | Keep normalization cases covered by the managed-linking tests. |
| 7 — approval origin | `managed::begin` requires the verification URI to share the configured Agora origin. | A separate relay-host policy remains a product decision. |
| 8 — pairing termination | `managed::checked` identifies terminal 401/403/410 responses; `poll` clears terminal or expired pending state. | Rate-limit-aware polling remains separate; an actionable error is not a retry schedule. |
| 9 — process configuration | `supervisor::spawn_child` sets the application data directory explicitly. | Sidecar credentials still travel through environment variables; do not describe them as pipe-delivered. |
| 10 — diagnostics | Build identity and a diagnostics export exist. | Persistent redacted logs are a separate decision; the export is not a complete log facility. |
| 11 — logged request URL | `log_target` strips queries from the explicit URL field; proxy and connector request errors remove reqwest's attached URL before formatting. | Logs still contain transport diagnostics; sanitize reports before sharing. |
| 12 — restart delay | The sidecar supervision loop resets its attempt counter after a healthy lifetime. | Verify repeated failure and recovery with the real packaged sidecars. |
| 14 — settings persistence | `Settings::save` restricts Unix permissions and syncs the file before rename and the directory afterward. | Windows permissions and failure behavior need platform-specific evidence. |
| 15 — distribution | Native build, candidate and publish workflows now exist, with signing inputs and provenance verification. | A workflow definition is not a signed release; signing, installation and acceptance remain release evidence. Automatic updating is wired ([the updater channel](release.md#the-updater-channel)); its first live update is release evidence too. |

Findings 4 (unlink), 5 (protocol negotiation), 13 (failed-cleanup recovery) and the product portions
above remain unresolved by this review. Finding 16 is a documented configuration behavior. The
robot executable supports one-shot USB/ROS observations and direct HTTPS delivery, verified in staging.
Continuous ROS/Zenoh transport and hardware UI acceptance remain open. See [robot.md](robot.md#live-acceptance-and-remaining-coverage).

## Original findings

| # | Severity | Area | What the code does | Why it matters in public | Fix shape | Class |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | medium | supervision | `supervisor::stop` calls `CommandChild::kill` (an immediate kill, never SIGTERM); cleanup runs only on `ExitRequested` and tray Quit; the sidecars have no parent-death watch (`panic = "abort"` skips the handlers). | A crash or force-quit leaves a connector dialling the relay with the device credential and a bridge holding the port, so the next launch cannot bind. The bridge's drain-on-SIGTERM path is never exercised. | The sidecars exit when the pipe from their supervisor closes; stop closes the pipe and waits before killing. | technical |
| 2 | medium | connector | The reconnect loop treats a 401/403 handshake and a `verify_handshake` mismatch like a network blip: a fixed `reconnect_secs` sleep, no backoff, never a terminal state; the desktop stays in `Phase::Retrying`. | A revoked device knocks on the relay every few seconds for ever, and the person never sees "link again". | Exponential backoff with jitter (technical); a distinct exit on authentication rejection that the desktop maps to a relink state (product). | technical + product |
| 3 | medium | supervision | The bridge listens on a fixed loopback port (Advanced setting); if the port is taken the bridge exits and retries for ever, its bind error is dropped (only the connector's stdout is parsed), and the status poll, the connector's status reporter and every forwarded request send the local bearer to whatever owns the port. | Any local process that binds the port first receives the local key and the prompts. | The bridge answers `/healthz` with an instance id the supervisor gave it at spawn; nothing authenticated is sent until the id matches; the bind error is surfaced. An ephemeral port chosen by the supervisor is the product half. | technical + product |
| 4 | medium | credentials | There is no unlink: `save` keeps `managed`; "Link again" writes a new `managed_device:<id>` and leaves the old entry; manual `tunnel_token` / `api_key` and `managed_local` are never deleted; no server-side revoke is called. | Secrets accumulate in the keystore and a person cannot leave managed mode or un-pair a machine. | An Unlink command that deletes the entries, clears `managed` and revokes the device at the server. | product |
| 5 | medium | protocol | The handshake carries `Authorization` and, in managed mode, `x-nexa-managed-auth: managed-v1` — no connector version, no protocol version. The relay learns `bridge_version` only from `BridgeStatus`, after attach, and only while status reporting works. Frames have no version byte; the connector decodes strictly and logs unknown frame types per frame; meta structs tolerate unknown fields but every field is required. | See [Protocol versioning](#protocol-versioning): a relay that adds a frame type, changes the header layout, renames a field or moves to `managed-v2` cannot tell an old desktop what to do, and the error text points at the wrong side. | Version headers at handshake, the relay's answer, and the rule for new fields. | technical, after the contract below is agreed |
| 6 | low | pairing | `managed::prepare` compares the stored relay URL (a normalised `Url::to_string()`) with the server's string byte for byte. | A server value without a trailing slash or with an uppercase host fails every Connect with "The approved connection changed". | Compare parsed `Url`s. | technical |
| 7 | low | pairing | `verification_uri` is checked for https, no userinfo, path `/bridge-pair` and the code in the fragment — not for the Agora origin; `relay_url` is checked for wss and no credentials — any host. | The browser can be sent to a host that is not the Agora the person linked to; the device proof is sent as the tunnel bearer to whatever relay host the server names. | Require the `verification_uri` origin to equal the Agora origin (technical). A relay host allowlist is a policy (product). | technical + product |
| 8 | low | pairing | Polling runs every 5 s until the local expiry whatever the answer: 401, 403, 410, 429 and 5xx are retried; a denied or revoked device keeps `pending_pairing`; an expired pairing waits for Cancel. | Noise at the server, and a person waiting on a pairing that cannot complete. | Stop on 401/403/410 and on denied/revoked, back off on 429, clear an expired pending pairing. | technical |
| 9 | low | credentials | Secrets reach the sidecars as environment variables (`BRIDGE_API_KEY`, `CONNECTOR_TUNNEL_TOKEN`, `CONNECTOR_STATUS_API_KEY`) and no `current_dir` is set, so a `config.toml` / `connector.toml` in the launch directory is merged in. | Same-user processes read the environment; a stray config file can set `allow_unauthenticated`, `upstream_api_key` or `log_units` under the desktop's feet. | Set `current_dir` to the app's data directory now; move secrets to the supervisor's pipe when the pipe exists (finding 1). | technical |
| 10 | low | diagnostics | All sidecar output except the tunnel-connected line is dropped; the desktop's own tracing goes to a GUI process's stdout; there is no Logs view. | Nobody can tell what failed, on their box or in a report. | A rotating, redacted log file and a "Reveal logs" action; the diagnostics export of the [build block](#build-diagnostics). | product |
| 11 | low | logging | `proxy` logs `%url` at warn level including the caller's query string; redaction relies on the hand-written `Debug` impls and on never logging headers. | A key passed as `?key=` by a client lands in the log. | Log the upstream base and the path, never the query. | technical |
| 12 | low | supervision | The restart `attempt` counter resets only when the connector reports a connection; after five bridge failures every later restart waits 30 s for the app's lifetime. | A box that was unhealthy once stays slow to recover. | Reset after a healthy period. | technical |
| 13 | low | supervision | `Failed` is sticky: `stop()` errors, `is_running()` stays true, Save/Link/Connect and the tray toggle are blocked until the app restarts (documented as intentional). | A person has to know to quit and relaunch. | A "retry cleanup" or "restart app" action. | product |
| 14 | low | credentials | `settings.json` is written with default permissions (typically 0644) and without fsync before the rename; it holds no secret, but the device id, the tenant slug and the URLs. | Other users of the machine read them; a power cut can leave an empty file. | 0600 on Unix, fsync the file and its directory. | technical |
| 15 | info | updates | No updater plugin or configuration, no signing or notarization configuration, CI never builds the desktop bundle; the version lives in both `tauri.conf.json` and the workspace. Settings changes need Disconnect → Save → Connect. | A release needs an updater key pair (public key in the config, private key and password as CI secrets), a hosted `latest.json`, the updater permission, Developer ID + notarization, Authenticode. | The release workflow carries this. | product |
| 16 | info | settings | `NEXA_AGORA_URL` is baked at build time and validated only when used; it is persisted in `settings.json` on the first save, so a later build with another URL does not move an existing install. | Expected for a custom build; worth saying in the docs. | Document; the [build block](#build-diagnostics) shows the declared origin. | product |

## Verified invariants

Read in the code and confirmed, so the security page can keep saying them:

- **Pairing proof:** 32 random bytes from the OS (`getrandom`), hex-encoded, kept in the keystore;
  only its SHA-256 leaves the machine (`settings::pairing_proof`, `managed::begin`).
- **Agora origin:** https with a host, no userinfo, root path only, no query or fragment
  (`managed::origin`); the pairing client is `https_only`, follows no redirect, times out at 15 s;
  TLS through the platform verifier; no `danger_accept_invalid_certs` anywhere.
- **Approval URI:** path `/bridge-pair`, the code in the fragment, `0 < expires_in ≤ 600`.
- **Before connecting** `managed::prepare` re-checks device id, tenant, connection and relay.
- **The webview** receives presence flags only and nothing stored; capability `core:default` only; no shell permission; CSP `default-src 'self'`;
  single instance.
- **Bridge bind:** loopback by default; an exposed bind without a key refuses to start unless
  `allow_unauthenticated` is set; a blank or whitespace key is rejected even with the opt-in; an
  unparseable address counts as exposed.
- **Relay must be WSS** in the connector and in the desktop, without userinfo, query or fragment.
- **Upstreams are origins** (scheme + host, no path, userinfo, query or fragment); managed mode
  requires a loopback HTTP upstream.
- **Redirects are disabled** in the bridge, the connector and every desktop client; the connector
  accepts origin-form request targets only.
- **No silent downgrade:** `verify_handshake` refuses a reply the connector did not ask for and the
  absence of the one it did; the local key is injected only in managed mode, after
  `authorization`, `x-nexa-managed-auth` and `x-nexa-internal-*` are stripped, and only on the
  inference allowlist; the local key never crosses the relay.
- **`MAX_WS_MSG_BYTES`** is enforced on encode and on the WebSocket configuration;
  `BridgeStatus` tolerates unknown fields.

## Build diagnostics {#build-diagnostics}

The owner's decision: official builds are signed, custom builds are configurable and must be
distinguishable, and troubleshooting needs to know which one it is looking at — without secrets,
without blocking forks, without telemetry.

A `build` block fixed at compile time and carried by every binary:

| Field | Source | Example |
| --- | --- | --- |
| `version` | `CARGO_PKG_VERSION` | `0.1.0` |
| `commit` | the build's git revision, or `unknown` | `583f8e7` |
| `target` | the target triple | `aarch64-apple-darwin` |
| `profile` | `release` / `debug` | `release` |
| `channel` | **declared** by the build configuration: `official` or `custom` (the default) | `custom` |
| `agora_origin` | the declared default Agora origin (`NEXA_AGORA_URL`), or `none` | `https://agora.example.com` |

Where it shows: the desktop's About panel and its diagnostics export (a text file the person can
attach to a report: the build block, the platform, the connection phase, the last redacted log
lines — never a token, a key or a proof); the headless binaries' `--version`; the bridge's `/status`
as a `build` object beside `bridge_version` (unknown fields are already tolerated there, so this is
not a schema change).

What it is not: `channel` is a claim the client makes about itself. A fork can set `official`. The
proof that a package is Nexa's is its signature and its provenance attestation, compared with the
release manifest and digests; the relay never grants or denies anything on `channel`, and no field
of the block is an authorisation.

**Implemented.** The `build-info` crate is the build-script helper every binary crate calls: it emits
`NEXA_BUILD_COMMIT` (the short revision, `-dirty` when tracked files differ from it, `unknown` without
a repository), `NEXA_BUILD_TARGET`, `NEXA_BUILD_PROFILE` and `NEXA_BUILD_CHANNEL` — `custom` unless
the environment declares `official`; any other word stops the build. `nexa_tunnel_protocol::BuildInfo`
is the block and `build_info!()` reads it in the crate that expands it. Where it shows:
`nexa-llm-bridge --version` and `nexa-llm-connector --version` (one line), the bridge's `/status` as
`build` (absent from older bridges: `#[serde(default)]` on both sides), the desktop's footer line and
its *Save diagnostics* button ([desktop.md](desktop.md#diagnostics)). The native matrix declares
`official` only when it runs from a release tag.

## Protocol versioning {#protocol-versioning}

What exists: frames `[u8 type][u64 id BE][payload]`, unknown types skipped by `decode_lenient`
and logged by the connector's strict decoder, `BridgeStatus::SCHEMA = 1`, the managed handshake
header `x-nexa-managed-auth: managed-v1`, and `MAX_WS_MSG_BYTES` fixed on both sides. There is no
negotiation: the relay cannot tell an old connector what it supports, and an old connector cannot
tell the relay what it is.

What a change would break today: a new relay → connector frame type is silently ignored (a cancel
frame, for instance, would never cancel); a header layout change cannot be detected; a removed or
renamed field in `ReqMeta`, `RespMeta` or `BridgeStatus` breaks the other side; a relay that offers
only `managed-v2` makes every installed desktop fail closed with a message that tells the person to
update the relay; a lower `MAX_WS_MSG_BYTES` on one side drops the whole multiplexed socket on one
oversized prompt.

The proposed contract — both sides move together, installed desktops keep working:

1. **The connector declares itself at the handshake** with two headers: `x-nexa-protocol: 1`
   (the frame layout and the set of frame types it understands) and
   `x-nexa-connector: <version> <target> <channel>` (the build block's words; a claim, not a
   proof). The headers are added before any relay reads them: a relay that ignores them behaves as
   today.
2. **The relay answers** `x-nexa-protocol: <n>` with the version it will speak on this socket — the
   highest it supports that is ≤ the connector's. A relay that does not answer is protocol 1. A
   connector that receives a version it does not know fails closed with a message naming **the
   app** as the side to update.
3. **Within a protocol version** fields are only added, never removed or renamed; every added field
   is optional (`#[serde(default)]`) on both sides; frame types are only added, and a side that does
   not know a type ignores it (the lenient decoder becomes the connector's decoder too). A removed
   field, a renamed field or a changed header layout is a new protocol version.
4. **Limits are declared, not assumed:** the relay's `x-nexa-max-frame: <bytes>` at the handshake;
   the connector refuses a request it cannot frame with a typed error instead of letting the socket
   drop.
5. **`BridgeStatus` keeps `schema` as the shape version of the status object** and gains the
   `build` block; the connector forwards the bridge's bytes unchanged, so the relay accepts any
   schema it knows and ignores fields it does not.

The relay's half of this contract lives outside this repository; nothing here changes until both
sides have agreed the words above.

## Changelog

- 2026-10-20 — The robot line keeps to the device's own acceptance.
- 2026-10-17 — Finding 15: the owner decided for automatic updates; the updater channel is wired in the app and
  the publish workflow.
- 2026-10-17 — Reconciled the original review with current source; separated implemented safeguards,
  outstanding product decisions and acceptance evidence instead of presenting old findings as current facts.
- 2026-10-08 — The build block implemented: the `build-info` helper, `BuildInfo` in the protocol,
  `--version` on both binaries, `build` on `/status`, the desktop footer and the diagnostics export
  (the export half of finding 10; the log file stays a decision).
- 2026-10-08 — First review before the public release: sixteen findings classified, the verified
  invariants, the build diagnostics block and the protocol versioning contract proposed.
