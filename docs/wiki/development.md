# Development

## The CI gate

[`.github/workflows/ci.yml`](../../.github/workflows/ci.yml) separates headless and desktop Linux
checks so GUI system dependencies do not slow the headless job. Together they cover the workspace.
Run the same strict profile locally after staging real desktop sidecars:

```sh
./crates/desktop/sync-sidecars.sh
cargo fmt --all -- --check
cargo check --all-targets --all-features --locked
cargo clippy --all-targets --all-features --locked -- \
  -D warnings -W clippy::pedantic -W clippy::nursery -W clippy::cargo \
  -W clippy::unwrap_used -W clippy::expect_used -W clippy::panic \
  -W clippy::todo -W clippy::unimplemented -W clippy::dbg_macro \
  -W clippy::print_stdout -W clippy::print_stderr
cargo test --all-targets --all-features --locked
cargo build --workspace --release --locked
node --check crates/desktop/ui/app.js
cargo deny --all-features --locked check -D warnings
```

CI additionally runs **gitleaks** on the working tree and full fetched Git history on every push and PR.
The pinned scanner archive is checked against its release checksum before extraction.
Install cargo-deny separately to run the supply-chain gate locally.

Two more rules the workflow enforces. Every third-party action is pinned to a **commit SHA** with the
release it corresponds to beside it; Dependabot's `github-actions` update raises the pin and the
comment together, and a bare tag never returns. The **`msrv` job** checks the whole workspace, sidecars
staged, on the workspace `rust-version`: a dependency that needs a newer compiler fails there, not on a
user's machine. The `stable` jobs use the current stable toolchain, so run the gate locally on it
(`rustup toolchain install stable`, or `RUSTUP_TOOLCHAIN=<version>`) before trusting a green that an
older local toolchain gave you.

A new third-party dependency must carry a licence in `deny.toml`'s approved allow-list, or the gate
fails. That is deliberate: every licence is vetted before it ships in a binary.

## Native matrix {#native-matrix}

[`.github/workflows/native.yml`](../../.github/workflows/native.yml) runs on every push to `main`, on pull
requests that touch the crates or the lockfile, and by hand. It is the proof per operating system that the
Ubuntu gate cannot give — the `cfg(unix)` and `cfg(windows)` halves of the supervisor, the musl static
binaries, the bundles — and it produces the **unsigned** candidate artifacts a release starts from.

| Job | Runner | Target | What runs |
| --- | --- | --- | --- |
| `cli` | `ubuntu-24.04` | `x86_64-unknown-linux-musl` | strict clippy, tests, release build of `nexa-llm-bridge` and `nexa-llm-connector` |
| `cli` | `ubuntu-24.04-arm` | `aarch64-unknown-linux-musl` | the same, natively on ARM64 |
| `cli` | `macos-15` | `aarch64-apple-darwin`, `x86_64-apple-darwin` | the same; the Intel tests run under Rosetta |
| `cli` | `windows-2025` | `x86_64-pc-windows-msvc` | the same |
| `cli` | `windows-2025` | `aarch64-pc-windows-msvc` | clippy and build only — compiled, not tested, not a supported platform |
| `desktop` | `ubuntu-24.04`, `ubuntu-24.04-arm` | `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` | sidecars staged for the target, strict clippy and tests of `nexa-desktop`, `deb` + `AppImage` |
| `desktop` | `macos-15` | `aarch64-apple-darwin`, `x86_64-apple-darwin` | the same, `.app` + `.dmg` |
| `desktop` | `windows-2025` | `x86_64-pc-windows-msvc` | the same, NSIS + MSI |

Every job uploads its files with a `SHA256SUMS` beside them and prints the sums in the run summary. The
bundles are **not signed and not notarized**: a signed, published release is the job of the
[release flow](release.md), which calls this workflow once from the tag and signs what it stored — the
compiled bytes travel beside each bundle in `bin/` for that. Official builds carry
`NEXA_AGORA_URL=https://agora.nexaethos.com` baked in; a custom build sets its own origin or none.

**Built is not verified.** A green matrix says the code compiles, the tests pass and the bundle is produced
on that runner; the acceptance of a platform — the tray, autostart, the keystore, pairing in a browser — is
recorded in [desktop.md](desktop.md#platform-matrix) only after a person ran it there.

## Working from an issue to a pull request

Start with the smallest complete behavior a person can use. Discuss a wire-contract, trust-boundary
or hardware-capability change with the maintainers before implementation; the public protocol and
its consumers must agree before either side changes. Do not add an inert adapter or advertise a
device as supported because its configuration parses.

1. Read [architecture.md](architecture.md) and the owning subsystem's wiki page. Check open issues
   and pull requests for overlapping work.
2. Create a topic branch from current `main`. External contributors use a fork. Keep unrelated
   changes out of the branch and preserve other contributors' uncommitted work.
3. Implement the behavior and its failure paths together. Add focused tests that exercise the
   contract, including authorization and revocation when the change crosses a trust boundary.
   Update the subsystem documentation in the same change.
4. Run the full local gate above, then open a pull request using the repository template. Record
   the exact commands, toolchain, operating system and hardware used. Label source inspection,
   compilation, automated tests and live acceptance separately.
5. Resolve review findings and rerun the affected checks. Required checks must belong to the final
   commit; an earlier green run does not validate later edits. Merge through the protected PR path.

Use [the security policy](../../SECURITY.md) for vulnerability reports rather than a public issue
or pull request containing exploit details. Never attach credentials, a full environment dump,
private deployment addresses or an unsanitized diagnostic log.

### Hardware changes

State the board or robot, operating system, adapter version, transport and observed capabilities.
Identify exactly what the test reads or changes. Observation, firmware installation, pin output and
motion are different scopes; successful USB discovery does not establish any of the latter three.

Use a real device for transport acceptance. Pure tests can prove parsing, framing and authorization
rules, but cannot establish USB access, a working ROS graph, reconnect behavior or electrical safety.
Record disconnect, reconnect, backend unavailability and credential revocation alongside the happy
path. Physical safety and emergency stop remain local to the hardware.

An adapter's documentation must name unsupported capabilities and explain actionable failures.
Raw telemetry, semantic events and model inference are distinct contracts; an inference credential
does not implicitly authorize hardware access. The current executable limitation remains explicit
in [robot.md](robot.md) until a real transport and its acceptance evidence replace it.

## Dependencies

The policy is **stay on the latest published version**.

- Requirements in `[workspace.dependencies]` name the version actually built and tested against
  (`serde = "1.0.229"`), not a bare major (`"1"`). Cargo still treats them as caret ranges, so this
  costs nothing at resolve time — it just makes the tested floor visible and gives Dependabot a
  manifest line to bump, so version history is reviewable instead of hidden in the lockfile.
- **Dependabot** runs weekly (`.github/dependabot.yml`): patch and minor bumps arrive grouped in one
  PR, a major bump gets its own PR because it needs a real review.
- To refresh by hand: `cargo update`, then run the gate. `cargo update --verbose --dry-run` also
  reports dependencies held back behind a semver-incompatible release, which `cargo update` will
  never take on its own.
- Every bump still passes `cargo deny check` — a transitive dependency arriving under an unlisted
  licence fails the gate even if the direct dependency looks harmless.

**Secret scanning** uses the gitleaks **binary**, not `gitleaks-action`: the action requires a paid
licence for organisation-owned repositories, while the tool itself is MIT. The version is pinned in
the workflow.

### Supply-chain exceptions

The User approved only the verified duplicate-package exception documented in
[dependency-version-splits.md](dependency-version-splits.md). Clippy keeps every other warning fatal;
cargo-deny now rejects new duplicate versions rather than merely warning. Its exact-version exceptions
are maintained separately because its active graph differs from Clippy's full metadata graph.
The compatibility re-export in webpki-roots is recognized by cargo-deny without an exception.

The existing unmaintained-crate policy checks workspace dependencies; Tauri's Linux GTK3 bindings
carry upstream unmaintained notices with no safe upgrade. This is distinct from vulnerabilities:
RUSTSEC vulnerability and yanked-crate checks still cover the entire dependency graph, without ignored IDs.
The new duplicate exception does not broaden the unmaintained policy.

The existing license list includes the CA bundle's CDLA-Permissive-2.0 data license and MPL-2.0
dependencies used unmodified by Tauri (cssparser, selectors, dtoa-short and option-ext).
MPL-2.0 is file-level copyleft, not a blanket permissive license; changes to covered files need a
separate review. Workspace crates are checked too: removing their declared MIT OR Apache-2.0 license
fails the gate. Registry wildcards are denied; internal workspace path dependencies are permitted.

## Comment policy

**Code comments are one short line.** A brief statement of the non-obvious thing, and — where there
is more to say — a pointer to the page in [`docs/wiki/`](README.md) that says it.

```rust
// Per-chunk, not total: long generations stream for minutes. See docs/wiki/configuration.md
```

The reasoning does not disappear; it moves. Anything longer than a line belongs in the wiki, where it
can be structured, linked, and read by someone who is not currently staring at that function.

Two rules follow from this:

- **Do not comment the obvious.** If the line reads clearly, it needs nothing.
- **Never reference a person, a date, a ticket, or an internal spec number.** Explain the constraint
  itself. "Matches the upstream read timeout" survives; "bumped 2026-06-17 per ticket X" does not.

When you change behaviour that the wiki describes, update the wiki page in the same commit.

## Conventions

- Edition **2024**, rust-version **1.96**, `#![forbid(unsafe_code)]` in every crate.
- `thiserror` at module boundaries, `anyhow` in `main` only.
- One shared `reqwest::Client` per process, so the connection pool is actually reused.
- Hand-written `Debug` for anything holding a secret — see
  [auth-and-security.md](auth-and-security.md#what-is-never-logged).
- Tests live next to the code in `#[cfg(test)] mod tests`. Pure logic is split into free functions
  specifically so it can be tested without constructing a TLS client or binding a socket —
  `select_candidates` and `record_observation` are the pattern to copy.

## Publishing hygiene

Nothing in this repo may identify a particular deployment: no real LAN IPs, hostnames, home
directories, person names, or internal identifiers. Examples use `192.0.2.0/24` (RFC 5737 TEST-NET)
and `relay.example.com`.

The test for anything you are about to add: *would a stranger installing this on their laptop need
it?* If not, it belongs in a private repo.

## Changelog

- 2026-10-17 — Added the contributor branch/review workflow and hardware acceptance requirements;
  distinguished device discovery, pure tests and live transport evidence.
- 2026-10-08 — The native matrix is callable by the release candidate and stores the compiled bytes beside
  each desktop bundle; the release flow has its own page.
- 2026-10-08 — The native matrix declares `NEXA_BUILD_CHANNEL=official` only from a release tag; every
  other build says `custom`.
- 2026-10-08 — The native matrix: clippy, tests and unsigned artifacts on Linux x64/ARM64 (musl for the box),
  macOS ARM64/Intel and Windows x64, Windows ARM64 compiled only; checksums beside every artifact.
- 2026-10-08 — Dependencies raised to the versions built against (rustls 0.23.45 closes RUSTSEC-2026-0285;
  Tauri 2.12 and its plugins; keyring 4.2); `flate2` held at 1.1.9 until `png` adopts `miniz_oxide` 0.9,
  so no new version split enters; the deny and clippy exception lists realigned to the graph (ten stale
  skips gone, `dirs` and `base64` moved with their consumers); actions pinned to commit SHAs; an `msrv` job.
- 2026-09-06 — Aligned full strict lint and supply-chain gates, recorded the scoped User-approved
  duplicate exception, and expanded secret scanning to fetched Git history with verified scanner downloads.
