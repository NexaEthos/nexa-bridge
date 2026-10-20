# Robot module

Source: [`crates/robot/`](../../crates/robot/), package `nexa-robot`.

The robotics module applies the same shape as the LLM side: the device **dials out**, governance is
enforced **on the device**, and the human watches through an open standard.

**V1 is telemetry and teleobservation only.** There is no actuation and no command config — not as
an oversight, but as a boundary. Task-level command dispatch will arrive with its own approval-path
design. The emergency stop is and stays physical and local.

## Status

The `zenoh` transport is **feature-gated** (`zenoh-transport`) and not yet wired. Everything else —
config, the allowlist, the Foxglove encoders — is real, pure, and unit-tested, so the crate builds
and tests everywhere without any hardware attached.

The default service entry point still **exits loudly** with or without the feature: no continuous
ROS/Zenoh transport is implemented in either build. Do not deploy it as a working telemetry service.

The `inspect-uno-q` command is a separate, implemented read-only USB diagnostic. It reads a real
Arduino UNO Q through a locally installed `adb`; it does not pair a device or upload observations.
The pairing commands and `observe` implement the direct HTTPS hardware contract below.
`inspect-ros-battery` reads one local ROS 2 battery measurement. The ARM64 Linux executable has
passed a real ROS Jazzy battery read and the missing-environment refusal. Both adapters have also
passed staging delivery, owner readback and revocation acceptance described below.

## Shared hardware client

`hardware::Session` is the credential-bound HTTPS client shared by the Unix CLI and the desktop.
It validates the source class, explicitly expected workspace, approval origin and exact receipt.
`collect` creates a document; callers must persist it before `deliver`. The CLI keeps its existing
private-file lock and state format. The desktop owns a separate OS-keychain proof and atomic
pending-document store; it never serializes the session into its settings or webview.

`board_inventory::inventory` runs only `adb devices -l`. It preserves offline/unauthorized entries,
rejects malformed or duplicate selectors and never selects the first transport automatically.
Platform-provided refusal details are accepted only in the two documented text/coded shapes;
control characters are stripped before display. Debug formatting of a session excludes all fields.

## Inspect an Arduino UNO Q

The adapter checks no board model: any Arduino Q board that exposes adb and `arduino-router` is read the same
way. The UNO Q is accepted end to end; the VENTUNO Q's acceptance on real hardware is in progress.

UNO Q has a Linux processor and a separate microcontroller. Arduino's onboard RPC router connects
those two sides; it is not Nexa Bridge. See the [official board documentation](https://docs.arduino.cc/hardware/uno-q/).
This adapter reads the Linux side only. It does not claim MCU sensor readings, firmware installation,
pin control, or support for traditional serial-only Arduino boards.

Connect your board over USB-C and inspect the device inventory with `adb devices -l`. Select its
exact serial explicitly; the command never chooses the first attached board:

```sh
cargo run -p nexa-robot --locked -- inspect-uno-q YOUR_ADB_SERIAL
```

The JSON snapshot contains host receipt time, board-reported model, kernel, architecture, uptime,
memory in bytes, router version and actual router service state. It excludes USB serial and hostname.
The adapter runs only fixed diagnostic commands: `get-state`, reads of the model/uptime/memory files,
`uname`, router version and `systemctl is-active`. No command text comes from the cloud or the operator.

An inactive router is reported as inactive, not replaced with an online state. Missing fields, invalid
units, duplicate memory fields, overflow and non-finite uptime fail the observation. Each ADB process
has a ten-second deadline and is killed on cancellation; inspect USB access and retry after a timeout.
Raw stderr is not copied into diagnostics, because it can contain device identity. The error names
the operation and exit code and points to `adb devices -l` and the local Arduino installation.

The parser tests exercise captured Linux measurements and malformed input; they use no simulated
transport. Live USB inspection has been exercised on macOS ARM64 with a Debian 13 ARM64 UNO Q and
Arduino Router 0.10.0. This USB check establishes diagnostic reads; separate cloud evidence is
recorded below. It does not establish ROS support on the board.

## Inspect a ROS 2 robot

Run the adapter on the robot host with its installed ROS 2 environment already sourced. The fixed
command subscribes to `/battery` as `sensor_msgs/msg/BatteryState` and reads only `voltage` in volts:

```sh
source /opt/ros/jazzy/setup.bash
cargo run -p nexa-robot --locked -- inspect-ros-battery
```

The adapter invokes `ros2 topic echo --no-daemon --once --timeout 10 /battery
sensor_msgs/msg/BatteryState --field voltage --csv` using separate arguments, never a shell. It
creates no background ROS daemon and sends no motor command. The outer process deadline is fifteen
seconds, including startup. A missing publisher, missing ROS environment or failed command produces
an error; no default voltage or state is invented. Unknown (`NaN`), non-finite and ambiguous readings
are rejected. A genuine zero remains zero, without an invented battery-health interpretation.

The scalar framing follows the [ROS 2 Jazzy CLI implementation](https://github.com/ros2/ros2cli/blob/jazzy/ros2topic/ros2topic/verb/echo.py).
The JSON snapshot records the voltage, receipt time, selected ROS distribution, host kernel and
adapter version. It does not contain hostname or network address. The hardware contract's
`firmware_version` identifies the observing `nexa-robot` software for this adapter; it does not claim
to know motor-controller firmware. Only `robot.observe` is declared. No pose, camera, diagnostic
stream or actuation capability is implied by this battery-only increment.

Live acceptance used a static `aarch64-unknown-linux-musl` release binary built with Rust 1.96.0,
cargo-zigbuild 0.23.4 and Zig 0.17.0. Its transfer hash matched before execution, the snapshot
contained a finite voltage and the expected ROS/architecture metadata, and the temporary binary
was removed afterwards. No toolchain, background service or actuator command was installed or run
on the robot. This evidence covers that Linux/ROS combination, not all ROS distributions or boards.

## Pair and send an observation

Hardware has its own approved identity (`lab_board` or `robot`) and calls the platform directly over HTTPS.
It never obtains an inference lease, advertises a model, or uses the LLM reverse tunnel. Pairing
declares only `board.observe` for a board or `robot.observe` for the battery adapter. The reported
platform belongs to the observed device, not the USB host.

The current headless state-store acceptance covers Unix hosts. Create a private directory outside
your repository, then pair your explicitly selected board:

```sh
mkdir -m 700 "$HOME/.nexa-hardware"
cargo run -p nexa-robot --locked -- pair-uno-q \
  "$HOME/.nexa-hardware/board.json" https://api.example.com https://agora.example.com \
  YOUR_ADB_SERIAL "lab/board-1"
```

Open the printed approval link yourself and choose the intended workspace. The CLI accepts an
approval URL only on the explicitly configured approval origin, at `/bridge-pair`, with the returned
code in its fragment. The first origin is the API receiving the device proof and events; the second
is the browser approval application. Supply the same origin twice in a same-origin deployment.
Neither is learned from the server response. Redirects, URL credentials and API-path origins are rejected.

For a ROS robot, run this on the robot host with its ROS environment sourced, using a different
state file and a descriptive asset path:

```sh
cargo run -p nexa-robot --locked -- pair-ros-battery \
  "$HOME/.nexa-hardware/robot.json" https://api.example.com https://agora.example.com \
  "lab/rover-1/battery"
```

After approval, give the exact workspace slug when sending one observation:

```sh
cargo run -p nexa-robot --locked -- observe \
  "$HOME/.nexa-hardware/board.json" my-workspace
```

The client checks the approved device class and workspace, reads the configured source, sends a heartbeat,
and submits a Physical Event v0 containing board diagnostics or battery voltage. It does not run a daemon:
one invocation produces one observation, and an idle device becomes offline according to the server's
heartbeat policy. Timestamps from the device describe observations; server receipt time controls retention.

### Persistence, retries and revocation

The state file contains a bearer credential and the local USB selector. Never attach it to an issue,
commit it, or copy it between devices. On Unix the directory must be private (0700), and files are
created as 0600. Debug formatting omits the state fields. Pairing and sending commands are compiled
only on Unix. Windows support requires a verified credential store; no unprotected-file fallback exists.

An OS file lock serializes commands using the same state path. Each observation is persisted with its
random event ID before transmission, then removed only after a matching `stored` or `duplicate`
receipt. A crash after the server commits is recovered by rerunning the same command. A timeout,
invalid receipt, conflict, or HTTP error leaves the pending file intact and reports an error.
For rate limiting, wait the reported number of seconds and rerun with the same state file. Never
edit the pending payload or generate another identity to evade the limit.

If the server explicitly answers `invalid` or `conflict`, inspect and preserve the pending file in
the same private directory, correct the reported source or contract problem, and move that rejected
file aside before collecting a new observation. Do not do this for a timeout or an unreadable receipt:
the original event may already be stored, so retry it with its existing identity instead.

The server permits 120 new events per device in a rolling minute; duplicate retries do not consume
that allowance. The approved physical retention window for events and deduplication evidence is
719 hours minimum and 31 days maximum, and the server cleans up every 30 minutes. Do not replay an old
pending file after that retention period expecting permanent deduplication: an expired identity can
be recorded as a new event. The CLI never silently discards or rewrites a pending observation.

Pairing validates the API and approval origins and captures the source before writing the
credential. These local preflight failures leave the state path reusable. The same captured
declaration is then sent after durable persistence, so a lost response cannot destroy a newly
created identity. If approval expires or the request fails before yielding an approval link, retain that state
for diagnosis and use a different state filename for a new pairing. An existing approved device can
still be used with `observe`. Revoke an unwanted device in the platform; deleting its local file
alone does not revoke access. Revocation must make further heartbeat and event requests fail.

The HTTP client verifies TLS certificates, disables redirects and has a 15-second request deadline.
It honours the proxy the environment declares (`HTTPS_PROXY`, `NO_PROXY`), so a device on a network
with a mandatory proxy can reach the platform; TLS still runs end to end through it. Error formatting
excludes URLs so query parameters cannot leak through transport errors. No cloud-provided shell text is
ever executed.

A refusal from the platform keeps its HTTP status and, when the body has one of the platform's two
shapes — `{"detail": "<sentence>"}` or `{"detail": {"code": "...", "detail": "<sentence>"}}` — its code
and sentence: `workspace_unavailable`, `device_not_approved` or a revoked credential then tell the
operator the way out. No other field and no other body is ever printed, and control characters are
dropped before the text reaches the terminal.

## Live acceptance and remaining coverage

The staging acceptance used a real USB-connected UNO Q and a ROS Jazzy ARM64 robot. Each device
received a separate observation-only identity, sent a heartbeat and real observation, and had its
events read back through the workspace owner's API. A replay of a persisted accepted event returned
the same journal and receipt time; changing its payload under the same event ID returned a conflict.
This exercised persisted replay, not an injected network failure. Hardware identities could not
obtain model profiles. After explicit revocation, both device credentials were rejected with HTTP 401;
the local credentials and temporary robot executable were removed.

A separate synthetic endpoint exercised the deployed staging boundaries:

| Check | Observed result |
| --- | --- |
| Heartbeat before approval | HTTP 409, `device_not_approved` |
| Extra v0 field, mismatched provenance, timezone-free timestamp | Three invalid receipts, no stored events |
| Batch of 125 new events | 120 stored, five rate-limited, HTTP 429 and `Retry-After: 60` |
| Duplicate retry at capacity | HTTP 200, original journal and receipt time preserved |
| Heartbeat limit | 30 accepted in one UTC minute; the 31st returned HTTP 429 with `Retry-After: 60` |
| Another workspace | Endpoint absent from listing; event read and revoke both returned HTTP 404 |
| Cursor pagination | Two one-event pages returned consecutive distinct journal IDs |
| Cleanup | Owner revoke returned HTTP 204; device access then returned HTTP 401; local credential removed |

These are service and command-line results. Desktop hardware acceptance, interrupted-network
fault injection and packaged process restart behavior remain separate acceptance work. The checks do not establish continuous ROS streaming, MCU access or actuation.

## Topic governance

The allowlist is not a filter applied to data on its way out. It **is the subscribe set** — a topic
outside it is never read in the first place. Nothing non-allowlisted can leak toward the cloud or
into a Foxglove frame, by construction rather than by discipline.

An **empty allowlist serves nothing**, and the binary refuses to start. Fail-closed, the same posture
as the bridge's bind guard.

Entries are either exact topic names or a single trailing `/*` wildcard:

| entry | matches | does not match |
| --- | --- | --- |
| `/rover/odom` | `/rover/odom` | `/rover/odom2` |
| `/rover/sensors/*` | `/rover/sensors`, `/rover/sensors/imu`, `/rover/sensors/lidar/points` | `/rover/sensorsX` |

## Foxglove encoders

Pure message builders for the server side of the `foxglove.websocket.v1` protocol — an open
teleobservation standard, so no custom viewer is needed.

- `serverInfo` — the first op after a client connects.
- `advertise` — announces one channel per allowlisted topic.
- `message_data` — the binary frame: opcode `0x01`, then subscription id as little-endian `u32`, then
  receive timestamp in nanoseconds as little-endian `u64`, then the payload.

They are kept pure precisely so the protocol shape is verified by tests long before a live socket
exists.

## Changelog

- 2026-10-20 — The retention paragraph and the acceptance list keep to what the device and its owner see; the
  server's own storage mechanics live with the server.
- 2026-10-18 — CLI pairing now completes local origin/source preflight before credential persistence,
  and sends the same prepared declaration without a second capture.

- 2026-10-18 — Shared the hardware client with the desktop while retaining CLI persistence; added explicit ADB inventory.

- 2026-10-17 — The Arduino Q family by name: the adapter reads any Q board exposing adb and `arduino-router`; UNO Q
  accepted end to end, VENTUNO Q in acceptance in the lab.
- 2026-10-17 — The client honours the environment's proxy and reports the platform's refusal code and sentence,
  read from its two shapes only and stripped of control characters.
- 2026-10-17 — Recorded real board/robot cloud acceptance, boundary checks and cleanup; clarified the approved retention window and remaining UI/DR evidence.

- 2026-10-17 — Added the local ROS 2 battery reader and explicit API/approval origins; the ARM64 binary passed a real ROS Jazzy battery observation; cloud acceptance remains open.
- 2026-10-17 — Added hardware pairing, heartbeat and durable single-event delivery; cloud acceptance remains pending.
- 2026-10-17 — Added typed, read-only UNO Q USB inspection with explicit device selection and real-board verification.
- 2026-09-06 — Clarified that neither feature setting provides a running transport; the passing
  library tests validate governance and encoding only, not an end-to-end robotics integration.
