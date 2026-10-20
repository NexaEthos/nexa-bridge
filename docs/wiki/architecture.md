# Architecture

## The problem

A GPU box behind NAT has no public IP, so a cloud platform cannot dial into it. But the box can dial
out. Everything in this workspace follows from that single fact.

## The pieces

```text
  cloud platform                              your box (no public IP)
  ┌────────────────┐                          ┌──────────────────────────────────┐
  │ model preset:  │                          │  nexa-llm-connector              │
  │  base_url ─────┼──▶ relay ◀───── WSS ─────┼── dials OUT, holds the tunnel    │
  │  api_key       │   (not in this repo)     │        │                         │
  └────────────────┘                          │        ▼                         │
                                              │  nexa-llm-bridge (127.0.0.1)     │
                                              │   auth · stream passthrough · LB │
                                              │        │                         │
                                              │        ▼                         │
                                              │  vLLM / Ollama / LM Studio :8000 │
                                              └──────────────────────────────────┘
```

| component | what it is |
| --- | --- |
| `nexa-tunnel-protocol` | The wire contract. Public and stable — both ends move together. |
| `nexa-llm-bridge` | An OpenAI-compatible reverse proxy in front of your local model server. Does auth, streaming passthrough, load balancing, and box observability. |
| `nexa-llm-connector` | Dials out over WSS to the relay, holds the tunnel, forwards each request to the bridge and streams the answer back. |
| `nexa-robot` | Read-only USB board diagnostics, one-shot local ROS battery readings and direct HTTPS hardware observations. Continuous ROS/Zenoh transport remains unimplemented; see [robot.md](robot.md). |
| the relay | The cloud-side endpoint the connector attaches to. **Not part of this repo.** Anything speaking `nexa-tunnel-protocol` over a WebSocket can play this role. |

## The path of one request

1. The platform calls the relay as if it were an OpenAI endpoint, with a bearer key.
2. The relay serialises the request into one `Req` frame and pushes it down the WebSocket the
   connector is holding open.
3. The manual connector forwards caller Authorization to the local bridge on loopback. A positively
   negotiated managed connector replaces it with its local key after relay serving authorization;
   see [managed-auth.md](managed-auth.md).
4. The bridge authenticates it, picks an upstream, and forwards it to the model server.
5. The response streams back the whole way — `RespStart` → `RespChunk*` → `RespEnd` — so SSE tokens
   arrive token by token, never buffered end to end.

## Invariants

These are load-bearing. Breaking one is a correctness or security bug, not a style choice.

- **Everything dials out.** The bridge binds loopback; the connector dials the relay. Hardware observations
  use direct HTTPS with a separate approved hardware identity. Nothing requires public inbound access.
- **Auth is enforced at the bridge, even behind the tunnel.** The tunnel is obfuscation, not the
  security boundary. See [auth-and-security.md](auth-and-security.md).
- **Exclusive desktop model selection is enforced before routing.** Discovery and tunnel status
  expose the same selected model; alternate models and runtime-native paths cannot bypass it.
  See [model-selection.md](model-selection.md).
- **No response body is ever buffered.** Streams pass through byte for byte. See [proxy.md](proxy.md).
- **No gzip/brotli on the proxy path.** Compressing would corrupt SSE streams. Do not enable those
  features on `reqwest` or `axum`.
- **`upstream_url` is scheme + host only, never `/v1`.** The bridge forwards the full incoming path.
- **Tunnel channels are unbounded by choice.** The send-queue depth is the honest backpressure
  signal; bounding it would trade an OOM risk for silent drops.
- **`#![forbid(unsafe_code)]` in every crate.**

## Why not just a VPN?

You can use one — a Tailscale/WireGuard overlay plus the bridge works, and the bridge's
`allow_unauthenticated` escape hatch exists for exactly that case. The tunnel exists because it
needs no coordination service, no client on the cloud side, and no inbound firewall rule anywhere:
one outbound WSS connection is the entire network requirement.

## Desktop hardware boundary

The desktop uses the `nexa-robot` library for explicit ADB inventory and hardware HTTPS transport.
Model transport remains in its supervised sidecars. Hardware has its own OS-keychain identities,
atomic pending observations and approved workspace; it cannot acquire or publish model authority.
The Rust hardware scheduler and manual device actions use a hardware-only operations lock.
Model operations use a separate lock. Update installation takes the hardware lock first, then
the model lock, and holds both through installation. Hardware keychain I/O runs on blocking
workers rather than async runtime threads. See [desktop.md](desktop.md#devices-panel) for storage, cadence and failure recovery.
