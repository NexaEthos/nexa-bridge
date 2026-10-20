# Security policy

nexa-bridge exposes a self-hosted LLM to a cloud platform over a tunnel. It sits on the boundary
between a private network and the internet, so security is a first-class concern.

## Reporting a vulnerability

Please report privately — **do not open a public issue** for a security bug.

- Email **security@nexaethos.com** with a description, the affected component (bridge / connector /
  protocol / robot / desktop), and a reproduction if possible.
- We aim to acknowledge within 72h and to ship a fix or mitigation before any public disclosure.

## Supported versions

Pre-1.0: a security fix lands on `main` and ships in the next release; older builds are not patched.

## Security properties

- **`#![forbid(unsafe_code)]`** in every crate.
- **Constant-time** bearer digest comparison (`subtle`). Config and subprocess environments contain
  runtime credentials; desktop secrets persist in the OS credential store, not settings JSON.
- Key-bearing config `Debug` implementations redact credentials and configured endpoint values.
  Proxy and connector code do not log bodies or Authorization headers. Protect local process access,
  debug dumps and third-party runtime logs; redaction is not protection against a compromised host.
- **Fail-closed bind** — the bridge refuses to start when bound to a non-loopback address with no
  `api_key`, unless `BRIDGE_ALLOW_UNAUTHENTICATED=true` is set explicitly.
- Empty or malformed configured credentials are rejected even on loopback; only an omitted optional
  key disables that authentication hop. LM Studio may remain keyless on a trusted local hop.
- **Auth is enforced at the bridge even behind the tunnel.** The tunnel is reachability, not a
  security boundary.
- Hop-by-hop headers are stripped in both directions, and the caller's `Authorization` is never
  forwarded to the model server — the bridge injects its own upstream key, if one is configured.
- `/_node/*` runs commands **argv-style with no shell**, and can only tail a unit from a configured
  whitelist.
- TLS is rustls with the `ring` provider. No OpenSSL.
- Connector attachment requires WSS. Upstreams are HTTP(S) origins without embedded credentials,
  queries or fragments; redirects are not followed on the inference path.

Details, including the per-hop trust model, are in
[docs/wiki/auth-and-security.md](docs/wiki/auth-and-security.md).

## Trust boundary

In manual mode, caller Authorization travels through the relay to the local bridge: a relay that
terminates TLS can see that key as well as prompts and responses. Do not treat it as an untrusted
carrier or claim end-to-end encryption. The tunnel token separately authorizes attachment.

In managed mode, the relay validates the server-owned serving credential and the connector injects
a different local key only after positive managed negotiation. That local key never reaches the
relay or model server. The relay still sees inference traffic and remains a trusted component.
The user's model-server key, if any, belongs to the final hop only.

## Supply chain

CI runs strict Clippy, cargo-deny with warning-as-error checks, and working-tree/full-history gitleaks.
The new lint exception is the User-approved, verified duplicate-version inventory; vulnerability
checks remain blocking. See [development policy](docs/wiki/development.md) and [`deny.toml`](deny.toml).
