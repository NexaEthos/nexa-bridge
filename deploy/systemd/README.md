# Durable box-side (systemd --user)

For a Nexa Agora workspace run the one-command setup instead — it writes and starts these services, linked with no
token to copy: [docs/wiki/server-setup.md](../../docs/wiki/server-setup.md). The files here are for a relay you run
yourself. Survives logout/reboot via user-linger.
Full notes: [docs/wiki/deployment.md](../../docs/wiki/deployment.md).

```sh
# 1. secrets (gitignored, chmod 600)
mkdir -p ~/.config/nexa-llm-bridge
cp deploy/systemd/bridge.env.example    ~/.config/nexa-llm-bridge/bridge.env     # edit
cp deploy/systemd/connector.env.example ~/.config/nexa-llm-bridge/connector.env  # edit
chmod 600 ~/.config/nexa-llm-bridge/*.env

# 2. build + install units
cargo build --release -p nexa-llm-bridge -p nexa-llm-connector
cp deploy/systemd/*.service ~/.config/systemd/user/
loginctl enable-linger "$USER"          # survive logout/reboot (once)
systemctl --user daemon-reload
systemctl --user enable --now nexa-llm-bridge.service nexa-llm-connector.service
```

Units assume the binaries at `~/nexa-bridge/target/release/`; adjust `ExecStart` if you cloned elsewhere.

## vLLM as a service (dedicated GPU box)

`vllm.service` is a reference unit for the upstream model server the bridge proxies to. On a box
dedicated to serving, run vLLM under systemd too (not a tmux/byobu pane) so it starts on boot AND
its journal is tailable — the bridge's `/_node/logs?unit=vllm` reads this exact unit. Adjust the
model + flags to your hardware; it defaults to `0.0.0.0:8000`, which is what `bridge.env`'s
`BRIDGE_UPSTREAM_URL` points at.

```sh
cp deploy/systemd/vllm.service ~/.config/systemd/user/   # edit model + flags for your box
systemctl --user daemon-reload
systemctl --user enable --now vllm.service               # ~2 min to load on an 8-GPU TP8 box
```
