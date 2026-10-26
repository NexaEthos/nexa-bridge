# Desktop app

> **Status: implemented, under validation.** `crates/desktop` contains the Tauri tray app,
> managed linking through Agora, native credential storage, runtime detection, and sidecar supervision.
> Managed browser pairing has native/staging acceptance evidence; the workspace's model guides replace the
> earlier model profiles (see [model selection](model-selection.md#your-workspaces-models)).
> Releases are signed and notarized for macOS and signed for Windows ([release](release.md)).

A minimal **system-tray app** for **Windows / Linux / macOS** that connects the LLM running on your
own machine to your Nexa workspace in a few clicks. The `bridge` + `connector` already do the work
on the command line; the desktop app is a friendly shell around them for people who don't live in a
terminal. **The product is the pairing, not the UI** — the entire value is "connect without a
terminal, without editing a config file."

## Managed pairing flow

1. In the desktop, **Connect to Agora** opens authenticated browser approval for a named computer.
2. The workspace owner/admin checks the code and approving workspace, then approves once.
3. The desktop receives non-secret approval metadata using its own private device proof, lets the
   user select a loaded model and requires explicit confirmation of the approved workspace.

No key is typed, pasted or put in a deep link. Credentials remain in the OS keychain or server-side
registry. Canceled/expired pairing has explicit recovery actions. The earlier proposal used pasted
one-time tokens/deep links; it is superseded by the browser-consent protocol in
[managed-auth.md](managed-auth.md), not an alternative implementation.

The approval page must sit on the explicitly configured browser approval origin (`same_origin`).
Advanced separates the API origin from that browser origin; an empty browser origin explicitly
selects the existing same-origin deployment behavior. A response cannot choose another host. While an approval is pending the panel polls every five seconds; after an error
the wait doubles up to a minute and the state is re-read. Agora's final answers end the pairing on the
desktop side — 401, 403 and 410 (`PairingEnded`), a device that is neither pending nor active, and the
local expiry all clear the pending row and the pending proof (`managed::clear_pending`, the same path
Cancel uses), so a pairing that cannot complete is never polled again and never resumed after a
restart. Before every Connect the stored relay address and the server's are compared as parsed URLs
(`same_relay`), so a trailing slash or a host's case never fails a Connect.

Native credential stores use platform-specific backends. Linux uses the pure Rust zbus Secret
Service implementation rather than the C-library-backed dbus implementation. Runtime probe I/O
uses the workspace Tokio runtime with `io-util`; response-shape checks are pure parser tests.
The generated bridge API key uses OS entropy through `getrandom`, with no application-seeded PRNG.
Loopback runtime probes use `reqwest`; Unix sidecar termination uses `nix` without application unsafe code.

## Devices panel

The Models and Devices views share the native shell. Device actions do not disconnect model
serving. The header's connection status describes the model tunnel, not hardware presence.

Arduino Q setup lists ADB transports and requires an explicit selection. An inventory label is not
board certification: **Inspect locally** runs the fixed Linux diagnostic adapter before any cloud
pairing. Unauthorized/offline transports remain visible with USB recovery instructions. Android
Platform Tools must be installed and `adb` must be on the app's process PATH; on macOS, launch from
that configured environment when Finder does not inherit your shell PATH. No tool is downloaded or
installed by the desktop. UNO Q has prior CLI acceptance; VENTUNO Q requires its own live trial.

Enter both trusted API and browser approval origins and an asset name to connect a board. A
separate hardware proof is saved in the OS keychain before a pairing request leaves the machine.
Browser approval grants only `lab_board` / `board.observe`. **Check approval** reads the actual
workspace; manually send only after checking that workspace. The desktop never imports an owner's
browser session. Hardware credential names are `hardware_device:<local-id>`, distinct from every
managed LLM entry. Removing one hardware record cannot erase the model's credentials.

The local `hardware.json` contains configuration, pending documents and receipt metadata, never
proofs. An atomic write and file sync precede every event delivery. A matching `stored` or
`duplicate` receipt clears the pending document; timeouts and refused/malformed responses retain
its ID and body. Manual device actions and the scheduler share a hardware-only operations lock.
Model autostart, Connect/Disconnect and model status use a separate lock. Signed update installation
waits for hardware first, then takes the model lock and retains both through installation; waiting
for hardware does not hold up model controls. Hardware keychain reads, writes and deletion use
blocking workers, so an OS keychain prompt does not occupy an async executor thread. An abrupt
exit after transmission leaves the same document available for replay. HTTP 429 delays are retained
and respected by delivery; approval checks also retain any delay returned by the server.

Automatic observations are explicitly enabled after approval and scheduled every 15 minutes while
Bridge runs, including while its window is hidden. They survive process restart as local settings.
Pause prevents further automatic sends. Sleep does not produce a burst of historical samples:
only a current observation (or the exact pending retry) is sent when execution resumes. Expired or
revoked authorization (HTTP 401/403/410) stops automatic sends and reports the refusal.
An approval check that encounters a network error, 429 or 5xx preserves the automatic setting
and pending observation, records the error and applies any supplied retry delay. Local persistence failures
are errors; they must never be hidden by reporting an event as delivered.
A fatal scheduler storage error remains visible when the Devices panel is refreshed, with an
instruction to preserve state, repair local storage and reopen the app. Manual controls stay
available. Removing a record pauses it before keychain cleanup, so a locked keychain can be
unlocked and removal retried without losing the record that identifies the credential.

A card's **last receipt** is delivery evidence, not online status. The platform's heartbeat expiry
can mark a board offline between periodic observations. **Manage / revoke in Agora** opens the
trusted web application; the user revokes there and checks approval again to observe refusal.
Local removal stops local use and removes its key, but is not server revocation. If removal is explicitly confirmed with an unresolved pending event, a private archive preserves
the document and metadata before local removal, and the file manager reveals it. The dialog explains
that it will no longer be retried and may already exist server-side. Archive failure leaves the
record and proof intact; there is no silent discard or permanent UI dead end.

ROS 2 guidance runs the existing battery-only CLI on the ROS host, with the environment sourced.
Build it from the matching source using `cargo build --release --locked -p nexa-robot` and run
`target/release/nexa-robot` (or add that directory to PATH). Current Bridge release assets do not
include the robot CLI. The guide does not imply a downloadable robot binary.
The panel is not a remote SSH client or ROS graph bridge. The host CLI sends one observation per
invocation; the desktop's USB scheduler is not claimed for the remote host. Generic serial Arduino,
MCU pin control, flashing, camera streaming and actuation are outside this implementation.

The panel uses native HTML dialogs for explicit send/schedule/removal confirmation and text-node
rendering for all device/server content. No raw document or credential is passed to the webview.
Desktop hardware integration remains under acceptance until native keychain, real USB, staging
readback/revocation and packaged restart/update trials are recorded. Existing CLI acceptance is not
substituted for those trials.

## Shape (Rust + Tauri v2)

- The workspace crate **`crates/desktop`**.
- **Runs `bridge` and `connector` as Tauri sidecars — child processes, not libraries.** The app
  bundles the two binaries, spawns them, supervises them, and restarts them with backoff if they
  exit.
- **Tauri v2 shell with tray + autostart first-class.** The app lives in the tray, starts on login,
  and is single-instance. The window is a thin status/settings panel; the tray is the primary
  surface (status dot · start/stop · quit).
- **Frontend is plain HTML/CSS/JS — no bundler, no `package.json`, no npm tree.** The panel is a
  status view with a handful of fields; a framework would buy nothing and would put a dependency
  tree outside `cargo deny`, which is the only supply-chain gate this repo has.

### Why sidecars and not in-process libraries

An earlier revision of this design embedded both crates as libraries. That was reversed, for three
reasons:

- **`panic = "abort"` in the release profile.** In-process, a panic in a proxy request handler does
  not fail one request — it takes down the whole application, tray included. Keeping the app alive
  would mean dropping `panic = "abort"`, changing the binaries everyone else ships.
- **One code path.** As sidecars, a desktop user runs byte-identical binaries to a headless install.
  A bug reproduces the same way in both, and there is one thing to support instead of two builds
  that drift.
- **Supervision beats coupling.** A child process that dies can be restarted in place, with the tray
  showing "reconnecting", and the upstream config can be changed by restarting only the bridge.

The cost is real and accepted: process lifecycle, stdio capture, no zombies on quit, and two extra
binaries in the bundle.

## Runtime flavors (what desktop users actually run)

Desktop users rarely run vLLM — they run **Ollama / LM Studio / llama.cpp**, which are all
OpenAI-compatible on `/v1`, so proxying already works. The panel's **Find local models** button discovers running servers on the usual local ports,
so the person does not need to enter an address for those runtimes.

### How detection identifies a runtime

Probing is loopback-only and takes milliseconds: a live server answers immediately and a dead port
refuses immediately, so the 800 ms timeout only bounds a port that accepts and then stalls. The
default ports are the entire search space — scanning wider would be guesswork.

| runtime | default port | identified by |
| --- | --- | --- |
| Ollama | 11434 | `/api/tags` |
| LM Studio | 1234 | `/api/v0/models` |
| llama.cpp | 8080 | `/props` |
| vLLM | 8000 | a `max_model_len` field on each `/v1/models` entry |

Each one is confirmed by an endpoint **only it serves**, so the answer is evidence rather than a
guess from the port number — a llama.cpp server on 11434 is still reported as llama.cpp. Anything
that answers `/v1/models` and nothing else is reported honestly as an "OpenAI-compatible server".

HTTP success alone is not runtime evidence: LM Studio can return HTTP 200 with an `error` object
for unsupported endpoints, including `/api/tags`. Probes reject those errors and validate the native
response shape: Ollama needs `models[].name`, LM Studio needs `data[].id`, and llama.cpp needs an
object-valued `default_generation_settings`. OpenAI listings need `data[].id`; valid empty lists
mean no models, while malformed lists are rejected. Redirects and system proxies are disabled.
The 800 ms bound applies to each request, including explicitly entered model-server addresses.

In Advanced, **Check address** probes only the entered server and reports its error without
substituting another runtime. **Find local models** explicitly scans the default ports. Addresses are normalised —
a missing scheme is added, and a trailing `/` or `/v1` is stripped, since the bridge wants scheme
and host only.

### Loaded models in LM Studio

Find local models reads LM Studio's native `data[].state` alongside the OpenAI model listing. The desktop-only
runtime result keeps `models` as the reported inventory and adds `loaded_models`: a list when every
native entry has a recognized `loaded` or `not-loaded` state, or `null` when loading state is unavailable.
An empty list explicitly means no loaded models; it is not replaced with the downloaded inventory.
The panel shows selectable loaded model names first and summarizes the remaining inventory as a count.
If state is unavailable, it labels the inventory as reported models with an explicit warning, not as
loaded. Other runtimes retain reported-model semantics without inferred load state.
Detection is a snapshot refreshed by pressing Find local models or Check address; it neither loads/unloads models nor selects one
automatically. Clicking a model marks it Selected; Save persists it and Connect exposes only that id.
The caller must use the same id. Changing the model server clears the selection; connected or busy
panels disable model selection. Policy and error semantics: [model-selection.md](model-selection.md).

### The concurrency hint

The panel shows how the runtime behaves when several agents call it at once, because that is what
decides whether a multi-agent workload runs or just queues:

- **vLLM** — parallel. Continuous batching; the right choice for multi-agent work.
- **Ollama**, **llama.cpp** — serialised by default. Ollama needs `OLLAMA_NUM_PARALLEL` raised;
  llama.cpp needs `--parallel`.
- **LM Studio**, unidentified servers — unknown; the note points at the setting to check.

These defaults move between releases, so the notes say **what to check** rather than asserting what
is true on a given machine.

Box stats degrade gracefully without a GPU (macOS: CPU/RAM only). Guidance: **vLLM recommended for
multi-agent work; Ollama / LM Studio / llama.cpp supported.**

## Platform matrix

| Platform | Tray | Autostart | Package |
| --- | --- | --- | --- |
| macOS | menu-bar item | LaunchAgent | notarized `.app` in a `.dmg` |
| Windows | notification-area icon | Startup / Run key | signed `.msi` / `.exe` |
| Linux | StatusNotifier (AppIndicator) | XDG autostart `.desktop` | AppImage + `.deb` |

The single-instance guard is implemented. A per-platform `nexabridge://` URL-scheme handler remains unimplemented.

The [native matrix](development.md#native-matrix) builds every row's bundle, unsigned, on every push to `main`
and runs the desktop crate's tests on each operating system; a built bundle is evidence of compilation, not
of acceptance — that stays in the verification record below.

## Distribution & signing

The [release flow](release.md) builds every platform once from a tag, lets people verify the unsigned
candidate, then signs macOS (Developer ID, notarized and stapled) and Windows (Azure Artifact Signing)
in separate jobs and publishes the release after the last approval. The certificates and accounts it
needs are listed there; none exists in this repository.

## Updates {#updates}

The app updates itself from the published release, through the Tauri updater plugin. When the panel loads
it asks `https://github.com/NexaEthos/nexa-bridge/releases/latest/download/latest.json` whether a newer
version exists; **Check for updates** at the bottom asks again. There is no periodic check. A newer version
shows a banner at the top of the panel with the changelog section (*What's new*) and **Install and
restart**; nothing is downloaded until the person clicks it.

- **What is trusted.** The manifest itself is not signed; each bundle it names is, with the release's
  minisign key, and the app carries the public key in `tauri.conf.json`. `requireSignedVersion` is on: the
  signature must be bound to the version the manifest announces, so a tampered manifest cannot pair a new
  version number with an older signed bundle. A bundle that fails either check is refused with the
  plugin's reason, and nothing is installed.
- **Which bundle.** The plugin looks for `{os}-{arch}-{installer}`: `darwin-*-app` (the signed app in a
  `.tar.gz`), `windows-x86_64-nsis` and `windows-x86_64-msi` (the signed installers, run in passive mode),
  `linux-*-appimage` and `linux-*-deb` (a `.deb` installs through `pkexec`, so the system asks for the
  administrator's password). The installer kind is the marker the bundler writes into the app for each
  package; an app outside a package has none and the check says no target was found.
- **Order of the install.** The bundle downloads while the connection keeps running; then the app takes
  the hardware lock followed by the model operations lock, stops the sidecars, installs and restarts. If the connection does not stop cleanly
  the install is refused with the supervisor's reason and the downloaded bundle is discarded; if the
  install fails, the error names it and Connect resumes the old version. On Windows the installer closes
  the app itself and opens the new version.
- **Forks.** A fork that ships its own builds replaces the public key and the endpoint with its own, as it
  replaces the identifier; otherwise its builds would offer this repository's releases.

The release side — the signatures, `latest.json`, the stable names — is in [release.md](release.md#the-updater-channel).

## Building it

```sh
./crates/desktop/sync-sidecars.sh
cargo run -p nexa-desktop
```

The sidecar helper builds the bridge and connector before staging their target-suffixed names for
Tauri. An optional target triple is passed to Cargo, not merely appended to host binaries. The
default is `CARGO_BUILD_TARGET` or the compiler host. Both executables must exist before copying;
Windows targets retain `.exe`. `CARGO_TARGET_DIR` is honored (relative paths resolve from the
workspace root); otherwise the helper explicitly uses the workspace `target` directory. A target
toolchain and linker must be installed separately. Native acceptance does not certify cross-platform
builds. Run the helper before building the desktop or starting Tauri development.

Tauri finds a sidecar by `<name>-<target-triple>` under `crates/desktop/binaries/`, which is what
the script produces. Those binaries are build artifacts and are gitignored.

The app stores non-secret settings in `settings.json` under the OS config directory, and the tunnel
token and api key in the **OS keychain** — never on disk in plaintext. The panel receives a
*presence flag* for each secret rather than its value, so a save cannot round-trip a secret through
the webview or wipe one by omission.

CI keeps the desktop app in its own job. The GUI toolchain is far heavier than the core crates, and
`check` is the job that should stay fast.

## Sidecar lifecycle {#sidecar-lifecycle}

The desktop starts both sidecars with their working directory set to the app's data directory — never
the launch directory, so no `config.toml` or `connector.toml` lying there is merged in — and with a
small contract in the environment on top of the settings: `BRIDGE_SUPERVISOR_PIPE=true` and
`CONNECTOR_SUPERVISOR_PIPE=true` tell each sidecar to read its stdin until it ends and then stop (the
bridge drains in-flight requests first); `BRIDGE_INSTANCE_ID` is a random token of each bridge process
that the bridge echoes on `/healthz` as `x-nexa-instance`. The pipe is the parent-death watch: when
the app stops, crashes, aborts or is force-quit, the OS closes the pipe and the sidecars exit on their
own instead of holding the port and dialling the relay as orphans.

Nothing authenticated leaves the app until `/healthz` has answered with the current process's instance id:
the status poll sends no bearer before that, and the connector is not even started until the bridge is
verified. A stranger that bound the loopback port first therefore receives nothing but an
unauthenticated liveness probe, and the person sees the bridge's own last stderr line in the status
detail (`Address already in use …`) instead of a bare exit code.

Disconnect sends SIGTERM first and kills after three seconds (Windows has no SIGTERM: the kill is
immediate). The retry ladder (1, 2, 5, 10, 30 seconds) starts again from the bottom after a sidecar
has run for a minute, so one bad hour never makes every later recovery wait thirty seconds.
Settings never hold a secret and are written owner-only (`0600` on Unix) and synced before the rename.

## Diagnostics {#diagnostics}

The panel's footer names the build: version, commit, target, profile, declared channel and the Agora
origin it was built for — the [build block](release-review.md#build-diagnostics). **Save diagnostics**
writes `diagnostics.txt` beside the settings (the app's config directory) and reveals it: the build
block, the platform, the connection phase with its last detail and restart count, and the non-secret
settings — the addresses, the selected model, the managed identity's ids, whether a
pairing is pending and when it expires, never its code. No credential, token, key or proof is ever
written: the keystore is not read. A report to the maintainers is that file plus what the person saw.

## Connection state

Process creation is not connection evidence. The panel reports `Connected` only when both managed
processes exist, a recent structured connector event confirms the relay connection, and an authenticated
bridge `/status` response contains models and a healthy upstream. The bridge snapshot follows its existing
upstream polling interval; the panel is not a benchmark or a guarantee that every listed model is loaded.

The connector emits JSON tracing fields `desktop_tunnel_connected=true` after a successful WebSocket
handshake and on received pongs, and `false` when its connection loop returns. The desktop reads the typed
field rather than matching human log messages. Evidence expires after 60 seconds without a confirmation.
The desktop checks local status every two seconds with a two-second timeout, without an HTTP proxy or redirects.
It passes the bridge key as `CONNECTOR_STATUS_API_KEY`; the status endpoint remains authenticated.
The bridge identifies its serving flavor as `desktop`.

Both missing binaries and process exits enter an actual capped retry loop (1, 2, 5, 10, 30 seconds),
reset after a minute of healthy running.
Stopping invalidates the generation so old event readers and delayed retries cannot adopt another child.
Every bridge spawn also gets a fresh random instance ID. A bridge exit invalidates that instance and
stops its dependent connector; the connector waits for verification of the replacement before it can
restart. The spawn checks verification again under the registration lock. A health response from an
older instance cannot verify its replacement, and late connector events are accepted only from the
currently tracked process. Monitor results are discarded when their instance changes in flight.
The generation check, process spawn and handle registration share one lock, so Stop cannot miss a
process created concurrently. A confirmed process exit needs no second kill. If cleanup of an
unconfirmed process fails, retries stop, the other tracked sidecars are stopped, and failure stays visible.
Shutdown errors are surfaced, not reported as a clean disconnect. Raw sidecar output is not forwarded to
the webview; the panel receives connection state and controlled diagnostics only.
A failed shutdown stays failed on repeated Disconnect attempts; inspect residual processes and restart
the app before reusing the profile.

A computer Agora no longer accepts is not retried. The desktop gives the connector `CONNECTOR_AGORA_URL`;
when the relay refuses the attach and Agora's device door answers with its final word (revoked computer,
removed connection or suspended workspace), the connector exits with status 77
(`nexa_link::agora::ENDED_EXIT_STATUS`). That exit stops both sidecars and the panel stays `Not connected`
with the reason and the way out: **Link to Agora again**, or **Connect** once a suspended workspace is
active. The status carries `relink: true`, so the panel leaves its compact view and opens the link step with that
button in sight — in the compact view it sat behind Workspace settings. The credentials stay in the keystore, so a resumed workspace needs no new approval; a new link
replaces them. Any other refusal, or an Agora that does not answer, stays in the retry loop.

## Settings and credential changes

The panel uses a fixed light theme, including the native window, independently of the OS theme.
Its warm canvas, ivory surfaces, charcoal text, indigo actions, stone borders, sage success and
amber warning colors match the Agora frontend's default light palette. The window starts at
560 × 800 logical pixels and can be resized down to 460 × 480. Header and diagnostics footer remain
visible while only the form scrolls; model names wrap instead of widening the panel.
The guided layout shows one setup step at a time, keeping the shorter default window usable on
scaled laptop displays; users can enlarge it without an application-imposed maximum. The saved
connection summary, Advanced control and footer were also checked at 560 × 800 on macOS.
Workspace, model and device settings use separate bordered surfaces with consistent fields and
a compact diagnostics/update footer. The build identifier stays centered and wraps at narrow widths.
The **Show advanced settings** checkbox reveals custom model-server addresses, deployment origins
and local preferences without changing or saving any values. The desktop links only through Agora: a relay
run by hand is configured on the command-line programs ([deployment](deployment.md)). If Connect to Agora needs an unset Agora origin,
it checks this control and focuses the address field before showing the existing error.
Collapsing advanced settings does not discard edits or alter connection permissions.

First setup presents three freely navigable steps: browser approval, local model selection and a
connection review. Find local models checks the existing usual local ports; Check address in Advanced
probes only the entered origin. Neither action chooses a model for the person. The review
shows the managed workspace and connection from native state, the selected model and the operating presets the
workspace chose for this connection, which run on that model, and
identifies the host as This computer because the saved state does not retain its pairing label.
Completed browser approval advances to model selection. A ready saved connection opens a compact
summary; entering the running state also switches to that summary. Settings remain reachable through
the summary buttons. Navigation itself adds no readiness gate and does not save settings. Connect
retains its existing save and native readiness checks; explicit Save settings lives in Advanced.
Existing failures and recovery guidance remain visible. This UI does not add unlink, cleanup retry or
log-reveal commands; those are not part of this version.

Disconnect before saving settings or rotating credentials. Both the command boundary and the Save button
enforce this; existing processes never continue silently on old secrets after a successful save.
The next Connect reads the saved settings and credentials again. Saving does not automatically reconnect.
Panel Connect saves dirty settings before checking and starting the connection. Settings, detection and key generation are disabled
while connected or while a command is pending, preventing overlapping panel operations. A generated key
is visible for copying only until a successful state refresh masks it again. Failed Saves keep the panel
dirty and prevent startup; the tray continues to operate on persisted settings only.
Status details remain available in a tooltip when truncated, and clipboard/subscription errors are shown.
Tray/menu/window errors are logged rather than discarded; application construction and TLS-provider
initialization return errors instead of panicking or continuing silently.

The desktop requires a `wss://` relay with no credentials, query, or fragment in its URL, and a nonzero
loopback listen port. Model-server addresses are normalized on Save as well as discovery; only HTTP(S) origins
are accepted. Headless bridge/connector configuration now enforces credential-free origins, WSS relay
attachment and nonempty configured keys too; see [configuration.md](configuration.md).

Only a missing settings file means first run. Unreadable or malformed settings return an explicit error.
An autostart failure fails Save rather than claiming the requested login behavior was installed.
Keychain, autostart, and settings-file writes are separate OS operations: on a failed Save, review the
state and save again before connecting; no cross-store atomic transaction is claimed.

## Verification record

### Guided setup acceptance

The local macOS ARM64 package was installed over an existing managed connection. After the native
Keychain prompt, the panel restored its workspace and model. Explicit model switching in LM Studio
updated the same connection with the selected model and profile; switching back restored the earlier
model without retaining the other profile's settings. Synthetic arithmetic turns and a fresh
conversation using knowledge tools completed through the connected model.

These checks establish local setup and transport behavior, not model certification. A small model
repeated an earlier answer after one topic change, and a larger model exhausted practical memory
headroom on the test machine. Qualification on another machine and signed candidate installation
remain separate evidence. A connected tunnel does not establish that its upstream model is loaded.

### Hardware desktop acceptance

The development preview was exercised on macOS ARM64 with a real UNO Q. Explicit ADB selection,
native diagnostics, browser approval, manual delivery and owner readback passed on staging. The
stored receipt and automatic-send deadline survived process restart with a distinct hardware
keychain entry. The installed model-serving app remained connected during the separate preview.
The panel was checked at its minimum 460-by-480 size, including scrolling and keyboard focus.

The complete workspace format, check, strict Clippy, 94 tests, release build and cargo-deny gates
passed; both shipped JavaScript files passed `node --check`. A source snapshot including new files
passed gitleaks with no findings. These are local macOS results. Scheduled delivery and revocation
are still under live acceptance; Windows/Linux native CI and signed package/update acceptance
remain separate release gates. The exact quality commands are in [development.md](development.md)
and the repository contributor instructions.

### Model transport acceptance

Native macOS validation exercised Save with OS Keychain storage, verified `Connected`, a disabled
Save button while connected, a real completion through the remote relay, and Disconnect with Save
enabled again. The temporary credentials were cleared after the test and the sidecars stopped.
Separate live core checks exercised authenticated status, rejection of absent/wrong inference keys,
SSE completion with usage and `[DONE]`, eight simultaneous short requests, and relay heartbeat evidence.
These checks do not certify model quality, full-context concurrency, or another operating system.
Additional native fault checks verified that an invalid tunnel token never reaches `Connected`, unsaved
edits block panel Connect, connected fields are disabled, and a forcibly terminated owned bridge process
is replaced automatically before a successful relay completion. Failed OS-level kills remain a
code-reviewed fail-closed path, not a fault injected on another user's process.

Local quality commands:

```sh
cargo fmt --all -- --check
cargo clippy -p nexa-desktop --all-targets --locked --offline -- -D warnings -W clippy::pedantic
cargo clippy -p nexa-desktop -p nexa-llm-connector --all-targets --locked --offline -- -D warnings
cargo test -p nexa-desktop --locked --offline
cargo test -p nexa-llm-bridge -p nexa-llm-connector -p nexa-tunnel-protocol --locked --offline
node --check crates/desktop/ui/app.js
```

The current macOS suite contains 20 desktop, 32 bridge, 5 connector, 6 robot-library and 9 protocol tests.
Use the full workspace lint/test commands in [development.md](development.md), not just the earlier
focused commands above. Windows/Linux UI, login autostart, signed packages and long-running fault recovery remain
outside this native macOS verification. Rebuild and synchronize the sidecars before launching the
desktop; otherwise the GUI can exercise older binaries despite a freshly compiled application.
The protocol/connector follow-up makes encoding fallible and resolves their pedantic findings.
The full all-target/all-feature workspace gate is checked with pedantic, nursery, cargo and explicit
panic/unwrap/expect diagnostics. Only verified transitive duplicate versions have a User-approved exception.

Runtime classification regressions use pure JSON inputs, including HTTP-200 error bodies and
malformed listings. They replace the previous canned-response TCP servers; transport detection
must additionally be checked against a running runtime through the native Find local models button. The removed
test-server helper needed its own TLS-provider initialization because tests do not run application
startup; pure classification no longer needs a network client or that setup.

Native macOS verification of the light-panel correction: rebuild with
`cargo build -p nexa-desktop --locked --offline`, quit the disconnected instance, then launch
`./target/debug/nexa-desktop`. In Advanced, enter `http://127.0.0.1:1234`, press Check address, and verify the LM Studio
label and model-name buttons. This passed against a real LM Studio server, including the LFM 2.5
listing. With the loaded-model panel enabled, native discovery showed `Loaded models (1)` with only
`lmstudio-community/lfm2.5-2.6b-mlx` as a selectable model and six other listed models summarized separately.
The native API confirmed that exact model as `loaded`; the other six were `not-loaded`.
Resizing to 720 × 820 was verified through macOS Accessibility and a window-only screenshot confirmed
the light native title bar, matching panel palette, wrapped model chips, and visible Save footer.
This check leaves the application disconnected and does not change or generate credentials.

## Icons

Everything under `crates/desktop/icons/` is generated from `brand/logo-source.png` by
`brand/make-icons.py` (macOS only — it uses `sips` and `iconutil`). Re-run it when the logo changes;
do not hand-edit the outputs.

It derives two different things from the one source. The **app icon** is the badge with its white
surround removed, because a white square behind rounded artwork looks broken on macOS and Windows.
The **menu-bar glyph** is a monochrome template image of the mark alone — a macOS status item needs
alpha-only artwork so the system can recolour it for light and dark menu bars; handing it the full
colour badge would render a dark blob.

The source is the 1024×1024 master of the Nexa Agora mark (dark badge `#1C1B19`, the white N with its slanted
left leg, the lilac dot `#B07DA6`), the same artwork the app and the site use: `brand/logo-source.png` in this
repository.

## Non-goals

- **Not a dashboard.** Metrics, logs, and monitoring live in the Nexa web app; the desktop app shows
  only local status (connected? which model? start/stop).
- **No embedded model server.** It connects an EXISTING local runtime — it does not ship or manage
  vLLM/Ollama.
- **No config-file UX.** If a user is editing a TOML, the desktop app failed its one job.

## Changelog

- 2026-10-26 — A computer Agora no longer accepts stops both sidecars and says why, instead of retrying for ever;
  the panel opens the link step with «Link to Agora again» in sight (found in the acceptance on a Mac).
- 2026-10-25 — the review and the guides show the operating presets the workspace chose, on the model this computer serves; no preset is matched to the selected id.
- 2026-10-20 — The status names the signed releases.
- 2026-10-19 — Reduced the default window height for scaled laptop displays, aligned native discovery messages with the guided UI and clarified hardware release prerequisites.

- 2026-10-18 — Added a three-step connection setup and compact saved-connection summary using existing native commands. Refined desktop spacing, section surfaces and application footer; replaced the advanced disclosure with an explicit checkbox while preserving the missing-origin recovery path.

- 2026-10-18 — Separated hardware/model serialization, moved hardware keychain work
  off async workers, retained scheduling after transient approval failures and corrected ROS installation guidance.

- 2026-10-18 — Added the Devices implementation and split approval-origin configuration; native and live acceptance remain separate.

- 2026-10-17 — Updates: the app checks the published release when the panel loads and on demand, shows the
  changelog section and installs a signed, version-bound bundle on the person's click, stopping the sidecars first.
- 2026-10-17 — Bound verification to individual bridge processes, stopped dependent connectors on bridge
  exit, and prevented stale probes or process events from restoring a replaced instance's authority.
- 2026-10-15 — Icons: the source is the 1024px master of the Nexa Agora mark (dark badge, slanted N, lilac
  dot) instead of the old 400px grey badge, so the app icon and the menu-bar glyph are no longer upscaled.
- 2026-10-08 — Distribution: the release flow replaces the wiring task; the version lives in the crate only.
- 2026-10-08 — Diagnostics: the build block in the footer, *Save diagnostics* writes and reveals
  `diagnostics.txt` without a secret in it.
- 2026-10-08 — Sidecar lifecycle: the supervisor pipe stops orphans, SIGTERM before the kill, the
  instance id on `/healthz` before any bearer leaves the app, the data directory as working directory,
  the last stderr line in the exit detail, the retry ladder reset; pairing polls stop on Agora's final
  answers and the approval page must sit on the named origin; settings written owner-only and synced.

- 2026-09-06 — Build sidecars for the actual requested Cargo target, preserve Windows executable
  suffixes and honor an explicit target directory. Verified native artifacts and argument rejection.

- 2026-09-06 — Added shared profile selection acceptance, corrected Connect auto-save and managed
  pairing status, and aligned the verification record with the full strict workspace gate.

- 2026-09-05: Replace model-name copying with persisted exclusive selection. Highlight Selected,
  require a selection before connecting, clear it on server changes, and pass the exact id to
  the bridge-side admission policy. Native click and Save verified against LM Studio.

- 2026-09-05: Show LM Studio's explicitly loaded models separately from its downloaded inventory;
  distinguish an empty loaded set from unavailable loading state, with pure parser regression checks.

- 2026-09-05: Validate runtime response shapes and reject JSON errors instead of misidentifying
  LM Studio as Ollama. Add a resizable light-themed panel matching Agora's default light palette,
  with fixed header/footer and wrapping model names. Replace canned HTTP detection tests with pure
  classifier regressions; verify actual runtime discovery separately in the native app.

- 2026-09-05: Corrected implemented/planned status; added verified connection evidence, authenticated
  connector status polling, spawn-failure retries, disconnected-only settings changes, address validation,
  explicit configuration/autostart errors, and focused regression tests. Live acceptance results are
  recorded separately from compilation and pure tests.
- 2026-09-05: Serialized spawn/adoption against Stop, stopped retries after failed cleanup, and hardened
  panel command serialization, unsaved-edit handling, credential masking, explicit runtime selection,
  and visible error states.
