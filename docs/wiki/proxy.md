# The bridge proxy

Source: [`crates/bridge/src/proxy.rs`](../../crates/bridge/src/proxy.rs).

The proxy is deliberately thin. It authenticates, picks an upstream, forwards, and pipes the
response back. It does not understand SSE, does not parse tokens, and does not buffer answers.

## Streaming passthrough is the whole design

The upstream's `bytes_stream()` is piped straight into the axum response body. Nothing re-parses it.

This matters for three reasons:

- **Time to first token.** A re-parsing proxy adds a frame of latency to every chunk. Passthrough
  forwards the first byte the moment it arrives.
- **Backpressure.** The stream is poll-driven end to end, so a slow client naturally slows the read
  from the upstream instead of accumulating in the proxy.
- **Memory.** A long generation is never held in RAM. Buffering one would be the difference between
  a few MiB and hundreds.

The rule that follows: **never call `.text()`, `.bytes()` or `.json()` on a streaming response.** Any
of them buffers the whole thing, destroys TTFT, and can OOM the box. Equally, **never enable
gzip/brotli** on `reqwest` or `axum` — a compressing hop mutates the body and corrupts SSE framing.

## Request path

1. **Auth.** Checked first, so nothing downstream runs for an unauthenticated caller.
2. **Rate limit.** Checked *after* auth on purpose: an unauthenticated flood is rejected by the
   cheaper check and never consumes the token budget. Over-limit gives `429`.
3. **Buffer the body.** The request body — the prompt — is buffered, both to enforce
   `max_body_bytes` and because routing needs to read its `model` field. Over the cap gives `413`.
   Having it buffered is also what makes failover safe: the same bytes can be re-sent to a peer.
4. **Pick candidates.** See [load-balancing.md](load-balancing.md).
5. **Forward, in order, until one answers.**

## Header handling

Hop-by-hop headers (RFC 7230 §6.1: `connection`, `keep-alive`, `proxy-authenticate`,
`proxy-authorization`, `te`, `trailer`, `transfer-encoding`, `upgrade`) are stripped in **both**
directions — they describe a single hop and must not be relayed.

Three more are dropped on the way out:

- **`Host`** — `reqwest` sets it for the real destination.
- **`Authorization`** — the caller's bearer authenticates them *to the bridge* and must never leak
  to the model server. If the upstream needs a key, the bridge injects its own.
- **`Content-Length`** — the bridge re-buffers and may rewrite the body, so the caller's length can
  be stale. `reqwest` recomputes it from what is actually sent.

`X-Affinity-Key` is also consumed here and never forwarded: it is a routing directive, not model
input.

On the way back, `Content-Type` is forwarded **verbatim** — it is what tells the caller the response
is `text/event-stream` — while `Content-Length` is dropped, since a streamed body has no length up
front.

## Body transforms

Two optional per-route rewrites, applied to the buffered body before forwarding. Both are skipped
entirely when the body is not a JSON object, so non-chat paths like `GET /v1/models` are never
disturbed. If nothing changes, the original bytes are forwarded untouched.

**`upstream_model`** replaces the body's `model` field. This lets the platform expose one stable
canonical name while the device serves it under a vendor-specific id, so swapping the underlying
model is a config change on the box rather than a change everywhere upstream.

**`system_first`** re-roles every `system` message that is not at index 0 into a `user` message,
keeping its position and content. Strict chat templates accept exactly one system message, at the
start, and reject anything else with a 400. Orchestrators routinely violate that: they compose a
stable plus a volatile system block, and they inject mid-conversation steering as further system
messages. Lenient templates tolerate it; strict ones do not. Only index 0 stays `system`.

## Failover

Candidates are tried in order. A **connect error** or a **5xx** falls over to the next one; the first
usable response head is streamed straight back to the caller. Only when the last candidate fails does
the caller see `502`.

Each outcome is recorded against the upstream's base URL, which drives two things: state-change
logging, and the circuit breaker.

The `url` field of these lines is the upstream base and the path, without the caller's query string.
Request errors also have their attached URL removed before logging with `reqwest::Error::without_url`:
sanitizing the separate `url` field alone leaves the full request URL in reqwest's error display.

**Logging is transition-only.** The first failure after health logs at WARN, repeats drop to DEBUG,
and recovery logs at INFO. Without this, a node restart produces one WARN per failed-over request and
floods the journal. If the health mutex is poisoned the code reports "changed" — never suppress a log
over bookkeeping.

## In-flight accounting

The counter that least-connections routing reads is incremented before the send and decremented by a
guard that **rides the response body**. It fires when the stream ends or the client disconnects — not
when the response head arrives. That is the point: what matters for load is how many generations are
actually running, not how many connections were opened. On failover the increment is undone
immediately.

## Health endpoints

- `/healthz` — static 200. Liveness only, unauthenticated, so a probe never touches the upstream.
- `/readyz` — probes the upstream's `/v1/models`. Returns `503` with the upstream status when it is
  reachable but unhealthy, or "upstream unreachable" when it is not.

## Changelog

- 2026-10-17 — Removed attached request URLs from reqwest error formatting as well as the explicit log field.
