# Configuration

Every binary layers its configuration the same way: **built-in defaults < TOML file < environment
variables**. The env prefix differs per binary, and env always wins, so a deployment can ship a file
and override one value at runtime.

| binary | file | env prefix |
| --- | --- | --- |
| `nexa-llm-bridge` | `config.toml` | `BRIDGE_` |
| `nexa-llm-connector` | `connector.toml` | `CONNECTOR_` |
| `nexa-robot` | `robot.toml` | `ROBOT_` |

All three config structs have a **hand-written `Debug`** that redacts keys and tokens. If you add a
secret-bearing field, add it to that impl as `<redacted>` — deriving `Debug` would leak the value
into logs and error messages.

Start from [`crates/bridge/config.example.toml`](../../crates/bridge/config.example.toml).

## Managed desktop mode

The desktop manages credentials automatically after browser approval. Official builds supply the
trusted HTTPS issuer through `NEXA_AGORA_URL`; custom builds expose that origin in Advanced.
`CONNECTOR_MANAGED_AUTH=true` enables positive managed-v1 negotiation and local-only credential
injection, requiring an explicit loopback upstream and local status key. No manual API key is
needed in this mode; no LM Studio key is implied. Contract: [managed-auth.md](managed-auth.md).

## Bridge

### `upstream_url` (required)

Your model server as **scheme + host only** — `http://127.0.0.1:8000`. **Never include `/v1`.** The
bridge forwards the full incoming request path verbatim, so a caller configured with
`base_url = http://<bridge>/v1` reaches `<upstream>/v1/chat/completions`. Adding `/v1` here produces
`/v1/v1/...` and a 404.

### `selected_model` (optional)

Exact model id exclusively exposed by this bridge. The desktop supplies it through
`BRIDGE_SELECTED_MODEL`; unset preserves headless multi-model operation. Blank values are rejected, and so is any
`upstreams` route but a pool of the selected id (exactly that model, no `upstream_model`). Admission rules and supported paths: [model-selection.md](model-selection.md).

### `listen_addr` (default `127.0.0.1:8080`)

Loopback by default because the connector is co-located. Note the deploy artifacts and the example
config use **8088** — the default only applies to a completely unconfigured binary.

Binding a non-loopback address is guarded: see `allow_unauthenticated` below.

### `api_key`

The bearer key callers must present as `Authorization: Bearer <key>`. This is what the calling
platform stores for this endpoint. Generate one with `openssl rand -hex 32`.

Unset means **auth disabled**. That is allowed on a loopback bind (dev, or relay-colocated) and the
bridge logs a warning. On a non-loopback bind it is refused at startup — see below.
An empty string, whitespace or control characters are rejected rather than treated as an omitted key.

### `allow_unauthenticated` (default `false`)

The escape hatch that permits *no `api_key`* on a *non-loopback* bind. Without it the bridge
**refuses to start** in that combination, because it would proxy your GPU to anyone who can route to
it. Set it only when the network itself is the boundary — a WireGuard/Tailscale overlay that already
authenticates peers. Details in [auth-and-security.md](auth-and-security.md).

A `listen_addr` that does not parse as a `SocketAddr` (a `hostname:port`) is treated as
**non-loopback**: fail-safe, since assuming an unknown bind is local would be the dangerous guess.

### `upstream_api_key`

Optional key the bridge presents to your model server (`vllm --api-key …`). Usually omitted — that
hop is same-host or trusted-LAN. It is injected by the bridge; the *caller's* `Authorization` is
never forwarded upstream.
If configured, it must contain visible ASCII without whitespace; omit the field for a keyless server.
All configured upstream origins, including routes, reject embedded credentials, path prefixes, queries
and fragments. Redirect responses are passed back, never followed to another destination.

### `max_body_bytes` (default `16777216` — 16 MiB)

Cap on the buffered request body, which is the whole prompt. Responses always stream and are never
subject to this.

Do not lower it casually: large-context models produce multi-MiB JSON, and a 1 MiB cap will `413`
them. It must also stay **below** the tunnel frame cap `MAX_WS_MSG_BYTES` (32 MiB), because in relay
mode the body crosses as a single `Req` frame.

### `upstream_connect_timeout_secs` (default `10`) and `upstream_read_timeout_secs` (default `900`)

The read timeout is **per read, not a total request deadline**. It covers both the wait for the
initial response during model prefill and pauses between response chunks. The bridge client does
not impose a whole-request timeout; an actively streaming generation can run longer than 900 seconds.

The desktop-launched bridge uses the same default without requiring an additional UI setting.
An explicit `upstream_read_timeout_secs` in `config.toml` or `BRIDGE_UPSTREAM_READ_TIMEOUT_SECS`
still overrides it. Existing overrides are not rewritten; audit them when updating a deployment.
Rebuild and restart the bridge binary to apply a changed built-in default.

### `rate_limit_per_sec`

Optional global GCRA rate limit protecting your model from an authenticated flood — a leaked
`api_key`, or a runaway client. Unset or `0` disables it. It is checked **after** auth, so
unauthenticated floods are rejected cheaply and never consume the budget. Over-limit requests get
`429`. The burst equals the per-second rate.

### `breaker_failures` (default `2`) and `breaker_cooldown_secs` (default `30`)

Circuit breaker thresholds. See [load-balancing.md](load-balancing.md#circuit-breaker).

### `log_format` (default `json`)

`json` or `text`. `RUST_LOG` overrides the filter in both cases.

### `log_units`

Whitelist of systemd units `/_node/logs` may tail. Defaults to the model server plus the bridge
pair. See [node-observability.md](node-observability.md).

### `supervisor_pipe` (default `false`) and `instance_id`

Set by the desktop supervisor, not by hand: with `supervisor_pipe` the bridge drains and stops when its
stdin ends; `instance_id` (visible ASCII, at most 128 bytes) is echoed on `/healthz` as
`x-nexa-instance`, so the supervisor knows the port is this process before it sends a bearer. See
[desktop.md](desktop.md#sidecar-lifecycle).

### `[[upstreams]]` — multi-upstream routing

An array of routes. A request whose `model` field appears in a route's `models` list goes to that
route; anything unmatched or model-less falls back to `upstream_url`. Omit the whole block for
single-upstream behaviour.

| field | meaning |
| --- | --- |
| `url` | Upstream base — scheme + host only, no `/v1`. |
| `models` | Served-model-names this upstream answers for, **exact match** against the request's `model`. |
| `upstream_api_key` | Optional bearer this upstream requires. |
| `upstream_model` | Rewrite the body's `model` before forwarding. Lets you expose one canonical name while the server serves it under a vendor id. |
| `system_first` | Coerce non-leading `system` messages to `user`. For strict chat templates that reject a system message that is not at index 0. |
| `max_body_bytes` | This device's context budget. A request larger than this excludes the device for that request. |
| `max_inflight` | This device's concurrency budget (match e.g. vLLM `--max-num-seqs`). Drives saturation demotion. |

Several routes sharing a `models` entry form a **pool** — that is how you scale one model across
heterogeneous devices. See [load-balancing.md](load-balancing.md).

## Connector

| field | meaning |
| --- | --- |
| `relay_url` (required) | The relay WebSocket to dial, e.g. `wss://relay.example.com/_tunnel`. |
| `tunnel_token` (required) | Presented as a bearer on the attach request — decides who may attach. |
| `upstream_url` (required) | The local bridge, e.g. `http://127.0.0.1:8088`. |
| `reconnect_secs` (default `3`) | First wait before re-dialling after the tunnel drops. |
| `reconnect_max_secs` (default `60`) | Ceiling of the wait, which doubles after every attempt that did not attach (at least `reconnect_secs`). |
| `supervisor_pipe` (default `false`) | Set by a supervisor: the connector stops when its stdin ends. See [desktop.md](desktop.md#sidecar-lifecycle). |
| `keepalive_secs` (default `25`) | WebSocket ping period, so an ingress idle timeout never trips the tunnel. |
| `log_format` (default `json`) | `json` or `text`. |

Both manual and managed relay attachments require WSS without URL-embedded credentials, query or
fragment. The tunnel token must be nonempty visible ASCII. Reconnect, keepalive, status-poll intervals
and upstream timeouts must be positive. Managed mode additionally requires explicit loopback HTTP
and a nonempty local status key; an IPv6 loopback origin is accepted.

## Robot

`Config::load` returns a configuration error for invalid TOML or values that cannot be deserialized
into the declared field types. Environment values override the file; parsing errors are surfaced,
not replaced with guessed runtime settings.

| field | meaning |
| --- | --- |
| `zenoh_endpoint` | The zenoh router to connect to, e.g. `tcp/192.0.2.2:7447`. |
| `topics` | The topic allowlist. **Empty means the module serves nothing** — fail-closed by design. |
| `foxglove_listen` (default `127.0.0.1:8765`) | Foxglove ws-protocol listen address for teleobservation. |
| `log_format` (default `json`) | `json` or `text`. |

See [robot.md](robot.md).
