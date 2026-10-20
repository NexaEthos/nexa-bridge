# Managed desktop linking

The managed journey is implemented in source and under coordinated staging validation. It is
**Connect to Agora → approve the computer in the browser → select a loaded model → confirm the
workspace and connect**. No user-managed API key is needed. The local bridge remains authenticated;
LM Studio itself may remain keyless on loopback. The desktop has no other path; a server without a
desktop links the same way from its shell ([server setup](server-setup.md)). The command-line programs keep their
manual mode for a relay run by hand, never a fallback from failed managed authentication.

## Trusted deployment and credential boundaries

Official installers supply the HTTPS issuer origin at build time through `NEXA_AGORA_URL`.
For example, `NEXA_AGORA_URL=https://agora.example.com cargo build -p nexa-desktop`.
Custom installers leave it empty and require the operator's trusted HTTPS origin in Advanced.
Advanced also accepts a separate trusted browser approval origin. Empty selects the existing
same-origin deployment behavior. Pairing requests and device proofs use the API origin; approval
URLs must match the browser origin and the exact returned code. Neither origin is learned from a
server response. The API origin stays bound to the approved identity until a new browser pairing.
Both new origins remain attached to the pending request until approval completes. Cancelling or
expiring that request preserves the previous identity and its origins. Existing stored requests
without a separate approval origin retain their original same-origin contract.
No deployment-specific hostname or secret belongs in the public repository. Redirects are refused;
URLs with userinfo, queries, fragments or non-root issuer paths are invalid.

Native Rust generates a random 256-bit device proof, saves it in the OS credential store and sends
only its SHA-256 digest to `/api/v1/bridge/managed/pairing`. A ten-minute human approval code is not
an attachment credential. The browser owner/admin sees computer and tenant before approving once.
The desktop polls using its private proof, then displays the actual approved workspace. Autostart
and tray actions cannot confirm a new workspace; the first panel Connect explicitly confirms it.

The server owns a separate opaque serving key per managed connection, encrypted in the ordinary
tenant-owned model preset. The desktop never receives it. The relay receives its SHA-256 in an
issuer-bound device lease: 180 seconds, renewed every 60 seconds. A device proof cannot authorize
inference and a serving key cannot attach. Expiry closes admission and the socket, including
in-flight streams, on a 250 ms sweep. A late renewal cannot revive expired authority.

After positive mutual `x-nexa-managed-auth: managed-v1` WebSocket negotiation, the connector replaces
cloud Authorization with its locally generated bridge key. It strips caller internal-auth headers.
Only exact model-inventory and inference routes are eligible: GET `/v1/models`; POST
`/v1/chat/completions`, `/v1/completions`, `/v1/embeddings`, `/v1/responses`.
Native runtime administration/loading paths are not exposed. The bridge independently gates the
exact selected model and strips its own caller key before reaching LM Studio.

`CONNECTOR_MANAGED_AUTH=true` requires WSS, a root-path explicit HTTP loopback upstream, no custom
status URL, and a non-empty `CONNECTOR_STATUS_API_KEY`. The desktop supplies these from native
state, not from an untrusted webview. Manual connectors preserve caller Authorization instead.
Unsupported/missing negotiation fails with an actionable error; it never injects local credentials.

## Persistence and recovery

Non-secret settings include issuer, pending approval, selected model and confirmed device identity.
Settings are committed by temporary-file rename. Device proofs use per-device keychain accounts,
so a failed settings save cannot overwrite the previous device's key. Pending and loopback keys
use separate accounts. The webview receives presence flags and lifecycle metadata, never secrets.

Pending pairing can be resumed after restart, or canceled locally. A second begin cannot replace
a pending proof silently. If browser launch fails, reload native state so the approval code and
Open approval action remain accessible. Expired pairing explains cancel/retry. Local cancellation
does not revoke a concurrently approved server record: the owner must revoke that unused device
in Agora. Revocation/relink is user recovery; the desktop does not schedule automatic proof rotation.

Connect saves dirty model settings, rechecks actual model availability and verifies the saved
issuer/device/tenant/connection before publishing that model to the owning tenant. The preset is
uncertified and residency-unknown; ordinary residency policy still applies. A healthy tunnel plus
selected-model readiness is Connected, not proof that live cloud inference has been certified.

## Verification and delivery boundary

Pure Rust tests cover runtime payload validation, exact selection, credential injection and HTTPS
origin policy. Run `cargo test --all-targets --all-features --locked`, `cargo fmt --all -- --check`
and the full AGENTS lint profile before publication. Passing the narrower pedantic profile is not
a claim that nursery/cargo/unwrap/expect checks pass across the entire workspace.

Backend/relay enforcement, browser approval and desktop must be coordinated. The backend flag
`BRIDGE_MANAGED_AUTH_ENABLED` defaults off. Enable it only after the compatible browser approval
surface is available. Live database/transport acceptance runs on a real staging deployment,
including replay, cross-tenant authorization, expired/revoked leases and actual SSE content/usage.
Do not claim end-to-end success from pure tests or a handshake. Signed release packaging remains
a separate distribution gate; do not migrate production settings automatically.

## Native lifecycle and transport implementation notes

### crates/desktop/src/main.rs

The panel is the UI; a console window on Windows would be a second, empty one.
Config directory, resolved once at startup.
Everything the panel renders in one payload — one round trip, no partial states.
Absent = leave the stored secret untouched; empty string = clear it.
Scan the default ports of the runtimes a desktop user is likely to already have running.
Probe one address the user typed, so a non-default port is still one click to verify.
Tray "Connect"/"Disconnect".
reqwest is built with rustls-no-provider — install ring before any client exists.
Second launch (or a deep link) raises the running instance instead of starting a rival.
Auto-connect when the config is already complete, so a login start needs no click.
Closing the panel hides it: this is a tray app, the window is not the app.

### crates/desktop/src/settings.rs

The local model server — scheme + host only, no `/v1`.
Where the bridge listens. Loopback: only the co-located connector reaches it.
The relay WebSocket the connector dials out to.
Start the app on login.
Ollama's default port — the runtime a desktop user is most likely to already have.
Everything needed to run is present. The panel greys out Start until this holds.
`Ok(None)` means "no such entry", which is a normal first-run state, not an error.
Deleting a missing entry is not a failure — the desired end state is "absent".
32 bytes of OS entropy, hex encoded — same shape as `openssl rand -hex 32`.

### crates/connector/src/main.rs

RFC 7230 §6.1 — must not be forwarded in either direction.
reqwest is built with rustls-no-provider — install ring before any client exists.
Config-governed: a non-streaming completion yields no bytes until generation ends,
so this must outlast the slowest healthy generation (see Config docs).
A clean close is an ingress recycling the WS — routine, not an alarm.
Unbounded by choice: queue depth is the honest backpressure signal.
Writer: the only task touching the sink.
Keepalive: ping so an ingress idle timeout never trips a healthy tunnel.
Spawned, so a slow generation never blocks the next request off the wire.
Authorization IS forwarded — the bridge is what authenticates it.
Generic on the wire: transport errors can embed internal URLs and addresses.

### crates/connector/src/config.rs

Relay WebSocket to dial, e.g. `wss://relay.example.com/_tunnel`.
Bearer presented on the attach request — decides who may attach.
The local bridge, e.g. `http://127.0.0.1:8088`.
Per-read upstream timeout. Generous by default: a loaded model's non-streaming
completion produces no bytes until generation finishes — size this for the SLOWEST
healthy generation, never for the median request.
Where to read the bridge's live status (default: `<upstream_url>/status`).
Empty string disables status reporting entirely.
Bearer for the status poll when the bridge auth is enabled; absent = unauthenticated poll.
Hand-written: a derived Debug would leak the token into logs and error output.

## Changelog

- 2026-09-05: Implement separated credential boundaries, positive managed negotiation, explicit
  workspace confirmation, exact-model publication and recoverable native pairing state.
