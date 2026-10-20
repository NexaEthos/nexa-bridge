# Tunnel protocol

Source: [`crates/protocol/src/lib.rs`](../../crates/protocol/src/lib.rs), package
`nexa-tunnel-protocol`.

This crate is the **public wire contract** between the connector and whatever terminates the tunnel.
Treat it as a stable API: a change here is a change on both sides of the wire, and they must ship
together.

## Shape

Many concurrent HTTP requests share **one** WebSocket, demultiplexed by id. One in-flight request is
one `RequestId` (`u64`).

The two directions are deliberately asymmetric:

- The **request is buffered** and sent as a single `Req` frame. LLM prompts are small enough that
  streaming them would add complexity for nothing.
- The **response is streamed**: `RespStart` → `RespChunk*` → `RespEnd`. This is what lets SSE tokens
  flow through token by token, preserving time-to-first-token end to end.

## Frames

One WebSocket Binary message is one frame:

```text
[u8 type][u64 id big-endian][payload…]
```

| type | frame | direction | payload |
| --- | --- | --- | --- |
| `0x01` | `Req` | relay → connector | `[u32 meta_len][meta JSON][raw body]` |
| `0x02` | `RespStart` | connector → relay | meta JSON (status + headers) |
| `0x03` | `RespChunk` | connector → relay | raw body bytes |
| `0x04` | `RespEnd` | connector → relay | empty |
| `0x05` | `RespError` | connector → relay | UTF-8 message; terminates the stream |
| `0x06` | `Status` | connector → relay | Schema-versioned JSON status; id is zero |

`ReqMeta` carries `method`, `path` and `headers`; `RespMeta` carries `status` and `headers`.

Decoding rejects anything shorter than the 9-byte header, a `Req` whose meta length is missing or
truncated, and any unknown type byte.
`decode_lenient` instead skips unknown types with a complete header, preserving forward compatibility;
malformed known frames still return errors. Status carries bridge version, serving flavor, models
and upstream health. Model facts include optional task, loaded context length and quantization.
Tasks are runtime-reported free-form strings. Unknown JSON fields are tolerated; change the status
schema version only for breaking shapes. Model names alone do not establish serving conformity.

## `MAX_WS_MSG_BYTES` = 32 MiB

Enforced on **both** ends, replacing tungstenite's 64 MiB default.
`Frame::encode` also checks the complete frame length before copying variable payloads into the output.
It returns `Result<Bytes>` for serialization, length overflow and oversized-frame errors. Callers must
handle the error before sending or registering a pending request. This changes the Rust calling API,
not the bytes of valid frames: existing deployed peers remain wire-compatible.

The largest legitimate frame is a `Req`: the request body plus meta JSON plus headers. Response
chunks are small. So this cap must stay comfortably **above** the bridge's `max_body_bytes` (16 MiB
by default), or a large-context prompt that legitimately passed the HTTP body cap would be unable to
cross the tunnel. If you raise `max_body_bytes`, check this ceiling.

## Error semantics

`RespError` is terminal for that request id — the stream stops there. The connector sends a
**generic** message rather than the underlying error text, because transport errors can embed
internal URLs and addresses; the detail stays in the box's local log.
The body stays byte-opaque, including `stream_options.include_usage`; framing must not remove accounting
options or reinterpret SSE. Regression tests pin the exact request encoding and all frame round trips.

## Changelog

- 2026-09-05 — Made encoding fallible with checked metadata lengths and bounded frames; documented
  status/lenient decoding and added exact-wire and size-boundary regression coverage.
