# Auth & security model

Source: [`crates/bridge/src/auth.rs`](../../crates/bridge/src/auth.rs). For reporting
vulnerabilities see [SECURITY.md](../../SECURITY.md).

## The tunnel is not the boundary

This is the single most important idea in the repo. The tunnel gives you reachability and
obfuscation. It does **not** authenticate the caller, and a compromised participant on the tunnel
path must not be able to reach your raw model server.

So the bridge **always** enforces its own bearer auth, including behind the tunnel, including on
loopback when a key is configured. Auth is checked before routing, before rate limiting, before
anything touches the upstream.

## How the bearer check works

The configured key is hashed with SHA-256 **once at startup**; the plaintext is never stored beyond
config. Each incoming `Authorization` value is stripped of its `Bearer ` prefix (either case, with
surrounding whitespace trimmed), hashed, and compared to the expected digest with
`subtle::ConstantTimeEq`.

Two properties come out of that:

- **No timing oracle.** A byte-by-byte `==` leaks how much of the key an attacker got right, which
  turns a brute-force into a linear search. Constant-time comparison does not.
- **Fixed-length comparison.** Because both sides are 32-byte digests, the comparison also leaks
  nothing about the key's length.

A missing, empty, or wrong header is a flat `401 unauthorized` with no detail.
Configured keys must contain visible ASCII without whitespace; an empty string is not equivalent
to an omitted key. Startup rejects invalid keys, and the bearer check independently rejects an
empty presented value even if called with an empty configured digest.

No `api_key` configured means auth is disabled and everything is authorized — see the bind guard
below for when that is permitted.

## The fail-closed bind guard

The bridge **refuses to start** when it is both *exposed* (non-loopback `listen_addr`) and
*keyless* (no `api_key`), unless `allow_unauthenticated` is explicitly set. The error message names
the three ways out: set a key, bind loopback, or opt in.

That combination would otherwise publish an unauthenticated proxy straight to your GPU to anyone who
can route to the box — the single worst misconfiguration available, and an easy one to reach by
setting `BRIDGE_LISTEN_ADDR=0.0.0.0:8088` to "make it work".

A `listen_addr` that does not parse as a `SocketAddr` counts as **exposed**. Guessing that an unknown
bind is local is the dangerous direction to be wrong in.

Keyless-on-loopback and keyless-with-opt-in both log a warning at startup describing exactly which
posture is in effect.

## What is never logged

- Config structs have **hand-written `Debug` impls** that print `<redacted>` for every key and token.
  A derived `Debug` would leak them into any error or trace line. If you add a secret-bearing field,
  add it to that impl.
- Key-bearing config debug views hide endpoint values too, including before validation. Validated
  endpoint addresses may appear in local transport diagnostics. Endpoint URLs cannot embed credentials.
- Runtime config and subprocess environments still hold plaintext credentials in memory. Desktop
  persistence uses the OS credential store; settings JSON contains no secret. Protect local process
  access and core dumps, and inspect third-party runtime logs separately.
- Request and response **bodies are never logged** — they are prompts and completions.
- The `Authorization` header is never logged.
- Errors crossing the tunnel are **genericised**: the connector logs the detailed `reqwest` error
  locally but sends the remote caller a flat message, because those errors can embed internal URLs
  and addresses.

## Other properties

- `#![forbid(unsafe_code)]` in every crate.
- TLS is **rustls with the `ring` provider**, installed process-wide at startup. No OpenSSL, no
  `aws-lc` (which would drag in a cmake/C toolchain and break the clean static musl build). One
  provider is shared by `reqwest` and `tokio-tungstenite`.
- `/_node/*` commands are built **argv-style with no shell**, and `/_node/logs` can only name a unit
  from a configured whitelist. See [node-observability.md](node-observability.md).
- Supply chain: CI runs `cargo deny check` (advisories, licence allow-list, bans, source allow-list)
  and `gitleaks` on every change.

## Trust model summary

| hop | authenticated by | notes |
| --- | --- | --- |
| platform → relay | the relay's own scheme | Not part of this repo. |
| connector → relay | `tunnel_token` | Decides who may attach a tunnel. |
| caller → bridge (manual) | `api_key` bearer | Passes through the trusted relay, which can read it. |
| relay → connector (managed) | Managed negotiation + serving authorization | Connector injects a separate local key; see [managed-auth.md](managed-auth.md). |
| bridge → model server | `upstream_api_key`, usually none | Assumed same-host or trusted LAN. |

The relay terminates TLS and can read inference traffic. Manual caller keys pass through it; managed
local keys do not. Neither mode provides end-to-end body encryption through an untrusted relay.

## Endpoint validation

Upstreams must be HTTP(S) origins without userinfo, query, fragment or path prefixes. Connector relay
URLs must use WSS and cannot embed credentials. Status URLs allow an endpoint path but no embedded
credentials, query or fragment; an explicitly empty manual status URL still disables reporting.
Redirects are refused on both proxy hops. Managed connectors additionally require an explicit
loopback IP and a local credential; custom/manual mode is not a downgrade from failed managed auth.
Connector intervals and upstream timeouts must be positive, preserving the 900-second read defaults.

## Changelog

- 2026-09-06 — Rejected empty credentials and unsafe endpoint configurations, disabled upstream
  redirects, and corrected the distinction between managed/manual trust and local runtime secret storage.
