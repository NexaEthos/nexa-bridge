# The connector

Source: [`crates/connector/src/main.rs`](../../crates/connector/src/main.rs).

The connector runs on your box. It dials **out** over WSS to the relay, presents its
`tunnel_token` as a bearer on the attach request, and then holds that one WebSocket open. For each
`Req` frame it forwards the call to the local bridge and streams the answer back as
`RespStart` / `RespChunk*` / `RespEnd`.

It is the only component that talks to the outside world, and it only ever originates connections —
that is what makes the whole thing work behind NAT with no firewall changes.

## The loop

`main` re-enters `serve` until Agora says the device is gone, so a dropped tunnel always comes back on
its own. The wait between attempts starts at `reconnect_secs` and doubles after every attempt that did
not attach, up to `reconnect_max_secs`, with a quarter of jitter either way so a fleet never knocks in
lockstep; one successful attach puts the wait back at the start. Under a supervisor
(`supervisor_pipe = true`) the loop also ends when stdin closes — the desktop's parent-death watch, see
[desktop.md](desktop.md#sidecar-lifecycle). The pipe is read on a thread of its own, never tokio's blocking pool:
that read cannot be cancelled, and a runtime waiting on it would keep an ended connector alive while the desktop's
pipe stays open, so the desktop would never see its exit status.

The relay answers `401` both for a credential the issuer no longer knows and for an issuer it could not
reach, so the refusal alone proves nothing. In managed mode a handshake `401` makes the connector ask
Agora's device door (`agora_url`) with its own credential. Agora's final word — the computer revoked, its
connection removed or its workspace suspended — ends the loop: the connector logs the reason at ERROR
and exits with status 77 (`nexa_link::agora::ENDED_EXIT_STATUS`). The user service carries
`RestartPreventExitStatus=77` and the desktop stops instead of restarting it. Any other answer — the
device still active, Agora unreachable, a refusal that is not `401` — keeps the loop. A manual
connector never asks Agora: its token is not a device credential.

The two outcomes are logged differently on purpose. A **clean close** is INFO: hosted ingresses
recycle long-lived WebSockets on a schedule, so an instant reattach is routine operational noise, not
an incident. Only genuine errors log at WARN. Getting this wrong means either alert fatigue or a
silent failure.

## Concurrency

Three tasks share the socket:

- **Reader** — the main loop, decoding frames. Each `Req` is spawned onto its own task, so a slow
  generation never blocks the next request off the wire.
- **Writer** — drains an unbounded MPSC channel into the WebSocket sink. All outgoing frames and
  pongs funnel through it, which is what serialises access to the single sink.
- **Keepalive** — pings every `keepalive_secs` so an ingress idle timeout never kills an otherwise
  healthy tunnel.

The channel is **unbounded by choice**. Bounding it would trade an OOM risk for silent drops or
deadlock mid-stream; the queue depth is the honest backpressure signal instead.

Both helper tasks are aborted when `serve` returns, so a reconnect starts from a clean slate.

## Header handling

Hop-by-hop headers and `Host` are dropped on the way to the bridge. The caller's `Authorization`
**is** forwarded in manual mode — the bridge authenticates it. Managed mode instead injects a local
key after positive relay negotiation; see [managed-auth.md](managed-auth.md).
On the way back, hop-by-hop and `Content-Length` are dropped, since the response
is streamed and has no length up front.

Request targets must be origin-form paths with an optional query. Relative targets, authority-form
targets, fragments and control characters are rejected before constructing a local request, including
in manual mode, so a malformed frame cannot replace the configured upstream origin.

## Timeouts

The connect timeout is 10s. The read timeout defaults to **900s and per-read, not per-request**: a large
model under concurrent fan-out can drop below one token per second, and the connector must not give
up before the bridge's own upstream read timeout has had its chance. There is no total-request
timeout anywhere in the stack — a long generation is a normal event, not a hang.

## Errors

A failed upstream call sends `RespError` with a **generic** message. The detailed transport error is
logged locally only, with reqwest's attached request URL removed before formatting. This prevents
query values from appearing in request and body-stream errors; the remaining transport detail must
still not cross the tunnel.
Encoding and queue errors propagate to the request task, which logs the failure and attempts one
generic terminal `RespError`. A failed terminal send is logged; there is no panic or fabricated success.
Status encoding errors follow the existing reporting-health path and do not terminate inference.

## Changelog

- 2026-10-26 — Under a supervisor, an ended connector exits at once: the pipe watch reads stdin on its own thread
  (found in the desktop acceptance: the connector logged the stop and stayed alive until stdin closed).
- 2026-10-26 — A handshake `401` asks Agora about the device; its final word ends the loop with exit status 77.
- 2026-10-17 — Removed attached request URLs before logging HTTP request and stream failures.
- 2026-10-08 — Reconnect waits double to `reconnect_max_secs` with jitter and reset on attach; the
  supervisor pipe ends the loop when the desktop goes.

- 2026-09-06 — Aligned the documented 900-second read timeout and distinguished manual from managed authorization.

- 2026-09-05 — Adapted outgoing frames to fallible encoding without changing the wire protocol;
  preserved terminal-error behavior and local-only diagnostic detail.
