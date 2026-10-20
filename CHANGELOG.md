# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Per-page history with the reasoning
lives in the wiki pages under `docs/wiki/`.

## [Unreleased]

### Changed

- The first public release. The programs are those of 1.0.0-3; only the documentation changed since: the pool
  examples use generic LAN addresses, and the desktop page says the releases are signed and notarized for macOS and
  signed for Windows.

## [1.0.0-3] - 2026-10-19

### Added

- `nexa-bridge pool add <url>` balances the selected model across several model servers serving the same exact id
  — this server's and others on your network — with the bridge's pool (context fit, warm-cache affinity, least
  connections, circuit breaker); `nexa-bridge pool` shows each member's health and `pool remove` takes one out.
- `nexa-bridge uninstall`, also `install.sh --uninstall`: unlink, then remove the three programs.
- The status reports each pool member's load: requests in flight, its ceiling and whether the circuit breaker sets it
  aside (`inflight`, `max_inflight`, `circuit_open`, optional); `nexa-bridge pool` prints them, and Nexa Agora's
  Nexa Bridge page reads them.
- The README shows high availability and load balancing together: failover across computers at the relay, balancing
  across model servers in the bridge.

### Changed

- A bridge with a selected model accepts `[[upstreams]]` routes that pool exactly that model, never rewritten; any
  other route is still refused at start.
- The command-line setup and `nexa-bridge status` send the administrator to Agora › Settings › Nexa Bridge, where the
  workspace now chooses the preset a connection serves.

### Fixed

- Running the installer again on a linked server updates it: the services restart on the programs just installed
  and setup ends successfully. Before, the old version kept running and the update ended with an error.

## [1.0.0-2] - 2026-10-19

### Added

- One-command server setup over SSH: `curl -fsSL …/install.sh | sh` installs the bridge and the connector after
  checking them against the release's checksums, then `nexa-bridge setup` finds the model server, links the server to
  a workspace through a browser approval (link, code and QR code in the terminal), lets you choose the model among
  the served ids and Nexa's presets with their guide (printing the runtime's flags), and starts both as systemd user
  services. `nexa-bridge status`, `model <id>` and `unlink` follow. Works before or after the model server starts.

### Removed

- The desktop's manual relay settings (relay address, tunnel token, serving key): the desktop links only through
  Agora. The command-line programs keep their manual mode for a relay you run yourself.

## [1.0.0-1] - 2026-10-19

### Changed

- The model step shows your workspace's models on this computer: the Nexa presets your workspace chose for this
  connection in Agora, each with the exact model ID to serve, the context window to load and Nexa's steps and
  flags for your model server (LM Studio, Ollama, llama.cpp, vLLM, SGLang or another OpenAI-compatible server).
  The review names the preset the selected model serves. Requires an Agora server with model guides.

### Removed

- Model profile selection: the presets your workspace chooses carry their settings. A saved profile is ignored
  and cleared on the next connection.

## [0.2.0] - 2026-10-19

### Added

- Desktop Devices page for observation-only Arduino UNO Q diagnostics: discover the USB board,
  approve a separate hardware endpoint in Agora, send an observation manually or every 15 minutes,
  and pause delivery independently of the model connection. Requires `adb` installed separately;
  the desktop opens Agora to manage or revoke the endpoint.
- Hardware operations use a separate lock from model controls, credential access runs outside async
  workers, and pairing recovery preserves scheduled observation after transient failures.
- ROS 2 setup guidance for running the existing one-shot battery observer on the robot's own host.
  The `nexa-robot` CLI must be built from source; it is not included in the release downloads.
  This release does not control actuators or provide a continuous ROS transport.

### Changed

- Guided model setup: link Agora in the browser, find a loaded local model, then review its workspace
  and profile before connecting. Saved connections open a compact summary with settings still accessible.
- Refreshed desktop layout with an Advanced settings checkbox, a resizable window sized for laptop
  displays, clearer section surfaces and a centered build identifier. Diagnostics and update controls
  remain available on both Models and Devices.

### Fixed

- Hardware CLI pairing validates the approval origin before creating its local state file.

## [0.1.2] - 2026-10-17

The first published release. 0.1.0 and 0.1.1 were tagged but never published: the 0.1.0 candidate failed the
desktop test below on every platform, and the 0.1.1 acceptance found the update banner always visible.

### Fixed

- The desktop's diagnostics test reads the channel the build declares instead of assuming a custom build,
  so the candidate of a release tag (`official`) passes it.
- The desktop panel hides what it marks hidden: the update banner and its *Install and restart* button no longer
  show when no update is available or the check failed.

### Security

- rustls raised to 0.23.45, closing RUSTSEC-2026-0285.

### Changed

- The hardware client honours the environment's proxy (`HTTPS_PROXY`, `NO_PROXY`) and reports the platform's
  refusal code and sentence instead of a bare HTTP status.
- Windows release signing: the Tauri bundler signs the desktop executable after marking it with each package
  type, and the uninstaller and installers with the same Azure identity; the executables inside both
  installers are checked and their digests recorded in the signing report.
- Release assets carry no version in their names (`nexa-bridge-<target>.<ext>`, `nexa-llm-bridge-<target>`), so a
  link to the latest release's download stays stable; the version stays in the tag and the release manifest.
- Dependencies raised to the versions built against (Tauri 2.12, keyring 4.2, tokio, reqwest);
  `flate2` held at 1.1.9 until `png` adopts `miniz_oxide` 0.9.
- CI actions pinned to commit SHAs; a job checks the workspace on the minimum supported Rust.
- README, CONTRIBUTING and SECURITY describe the repository as it is: the desktop app exists, the
  gate is the one CI runs, security reports go to `security@nexaethos.com`.

### Added

- Automatic updates for the desktop app: it checks the published release when its panel opens and on
  demand, shows what is new and installs on the person's click a bundle signed for that exact version,
  stopping the connection first. The release workflow signs the updater bundles and publishes `latest.json`.
- Code of conduct, code owners, issue and pull request templates, this changelog.
- The native CI matrix: clippy, tests and unsigned bundles on Linux x64/ARM64, macOS ARM64/Intel and
  Windows x64; Windows ARM64 compiled only.
- The release flow: a pull request that sets the version and dates the changelog, a tag on its merge, a
  candidate built once from the tag with checksums, SBOM and GitHub provenance, and a promotion that
  starts when the candidate is green, verifies, signs macOS and Windows in separate jobs, re-attests and
  publishes the release; every privileged step waits for the `release` environment's approval.
- The store study: what the Mac App Store, the Microsoft Store and Flathub would require of this app, and
  the recommendation.
- The build block every binary carries (version, commit, target, profile, declared channel): `--version`
  on the bridge and the connector, `build` on the bridge's `/status`, the desktop's footer and its
  *Save diagnostics* export.
