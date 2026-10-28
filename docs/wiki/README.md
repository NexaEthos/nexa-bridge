# nexa-bridge wiki

The reference documentation for this workspace. **Code comments are deliberately one line long** —
the reasoning behind every non-obvious decision lives here instead, so the source stays readable and
the explanation stays in one place.

## Start here

- [Architecture](architecture.md) — what the pieces are, how a request travels, the invariants that
  must never be broken.
- [Configuration](configuration.md) — every setting for the bridge, the connector and the robot
  module, with the traps.
- [Server setup](server-setup.md) — one command over SSH: install, link a server to a workspace, its services.
- [Deployment](deployment.md) — systemd `--user` units, the model server, static musl builds.

## Components

- [Bridge proxy](proxy.md) — streaming passthrough, header handling, body transforms, failover.
- [Load balancing](load-balancing.md) — pools, context fit, affinity, least-connections, saturation
  escape, circuit breaker.
- [Auth & security model](auth-and-security.md) — the bearer key, what the tunnel is and is not.
- [Tunnel protocol](tunnel-protocol.md) — the wire contract between connector and relay.
- [Connector](connector.md) — the dial-out loop, reconnection, keepalive, timeouts.
- [Node observability](node-observability.md) — `/_node/stats` and `/_node/logs`.
- [Robot module](robot.md) — topic governance and Foxglove encoders.
- [Desktop app](desktop.md) — the Tauri tray app, managed linking and verified connection state.
- [Model selection](model-selection.md) — exclusive serving and the workspace's models with Nexa's guides.
- [Profiles](profiles.md) — the operating presets Agora sends with, the Qwen reasoning budget, what each runtime
  must support.

## Contributing

- [Development](development.md) — the CI gate, code conventions, the comment policy.
- [Dependency version splits](dependency-version-splits.md) — the audited, scoped duplicate-package exception.
- [GLib backport](glib-backport.md) — the Linux iterator correction, source provenance and regression.
- [Release review](release-review.md) — the pre-release read of the contract and the desktop: findings,
  verified invariants, build diagnostics, protocol versioning.
- [Release](release.md) — the version's pull request, the tag, the candidate, the acceptance, the promotion
  by digest: verification, separate signing, the updater manifest, the published release; the inputs.
