# Deployment

Two binaries, no runtime dependencies. `nexa-llm-bridge` sits in front of your model server;
`nexa-llm-connector` holds the tunnel. They are separate processes so either can restart without
disturbing the other.

There is deliberately **no Docker path**. Both are single static binaries — wrapping them in an image
adds a base layer heavier than the program itself, and the container would need host networking
anyway to reach a LAN model server.

## Linked to a Nexa Agora workspace

`nexa-bridge setup` installs and runs the two programs as systemd user services, linked through Agora with
no token to copy: [server setup](server-setup.md). The units below are for a relay you run yourself.

## systemd `--user` with a relay of your own

Units live in [`deploy/systemd/`](../../deploy/systemd/). User scope means no root, and
`enable-linger` makes them survive logout and reboot.

```sh
# 1. secrets (gitignored, chmod 600)
mkdir -p ~/.config/nexa-llm-bridge
cp deploy/systemd/bridge.env.example    ~/.config/nexa-llm-bridge/bridge.env
cp deploy/systemd/connector.env.example ~/.config/nexa-llm-bridge/connector.env
chmod 600 ~/.config/nexa-llm-bridge/*.env     # then edit both

# 2. build + install
cargo build --release -p nexa-llm-bridge -p nexa-llm-connector
cp deploy/systemd/*.service ~/.config/systemd/user/
loginctl enable-linger "$USER"
systemctl --user daemon-reload
systemctl --user enable --now nexa-llm-bridge.service nexa-llm-connector.service
```

The units assume the binaries at `~/nexa-bridge/target/release/`; adjust `ExecStart` if you cloned
elsewhere.

Hardening directives beyond `NoNewPrivileges=true` are shipped commented out. The namespace-based
ones (`PrivateTmp`, `ProtectSystem`, `ProtectHome`) need unprivileged user namespaces in the user
manager, which not every distro grants — enable them once the service starts cleanly with them on
your system.

## The model server

[`deploy/systemd/vllm.service`](../../deploy/systemd/vllm.service) is a **reference unit** — edit the
model and flags for your hardware. Running your model server under systemd rather than in a terminal
multiplexer gets you two things: it starts on boot, and its journal is tailable, which is what
`/_node/logs?unit=vllm` reads.

Two settings in it are not optional on a multi-GPU box:

- `TimeoutStartSec=900` — loading a large model with tensor-parallel init takes minutes, and systemd
  would otherwise kill it as timed out.
- `LimitNOFILE=1048576` — NCCL and the tensor-parallel workers open far more descriptors than the
  1024 login default.

## Static builds for the box

The release profile uses `strip`, thin LTO, one codegen unit and `panic = "abort"`. The box target is
`x86_64-unknown-linux-musl`, producing a fully static binary with no glibc version to match.

One caveat if you build for musl: its default allocator serialises multi-threaded allocation and is
several times slower under tokio. Swap in jemalloc or mimalloc as the `#[global_allocator]` for that
target.

## Ports

| port | who | exposure |
| --- | --- | --- |
| 8088 | bridge | loopback — the connector is co-located |
| 8000 | your model server | loopback or LAN |

The bridge's built-in default is `127.0.0.1:8080`; every shipped artifact sets **8088**. Binding
anything non-loopback requires an `api_key` — see [auth-and-security.md](auth-and-security.md).
