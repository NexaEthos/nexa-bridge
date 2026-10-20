# Contributing

Contributions are welcome — this project exists to be run on other people's hardware, and the
hardware we do not own is exactly where the interesting bugs live.

**Hardware contributions need real hardware evidence.** `crates/robot` provides read-only UNO Q
USB diagnostics, one-shot local ROS battery readings and direct HTTPS event delivery on Unix.
Continuous ROS/Zenoh transport remains unimplemented. Read [the adapter status](docs/wiki/robot.md)
and open an issue describing your setup and proposed capability before changing the contract.

## Before you start

Read [docs/wiki/](docs/wiki/README.md). It is the documentation — architecture, the configuration
reference, and the reasoning behind every non-obvious decision. Code comments are one line by design
and point into it.

[docs/wiki/development.md](docs/wiki/development.md) covers the conventions in detail. The essentials:

- **Comments are one short line** plus a wiki pointer. Anything longer belongs in the wiki, and a
  behaviour change updates its wiki page in the same commit.
- **No content identifying a particular deployment** — no real IPs, hostnames, home directories,
  person names, or internal identifiers. Examples use `192.0.2.0/24` and `relay.example.com`.
- Edition 2024, `#![forbid(unsafe_code)]`, `thiserror` at module boundaries and `anyhow` in `main`.

## The gate

CI runs this on every push and PR, and it must be green — run it locally first, on the current
stable toolchain, with real sidecars staged:

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
cargo deny --all-features --locked check -D warnings   # cargo install cargo-deny
```

CI also checks the workspace on the minimum supported Rust (`rust-version` in `Cargo.toml`) and
scans the working tree and the whole Git history with gitleaks. One test is Linux-only (it reads
`/proc`), so test counts differ between macOS and CI by design. No lint suppression, no `allow`,
no skipped check to get a green: fix the cause or explain in the pull request why it cannot be fixed.

## Pull requests

- One change per pull request, described in the template: what, why, how it was verified and on
  which platform. A behaviour change updates its wiki page in the same commit.
- Pull requests from forks run the same checks with no secrets and no privileged runner; nothing in
  this repository executes fork code with credentials.
- Merges, the name **Nexa Bridge** and official releases stay with the maintainers (see
  [CODEOWNERS](.github/CODEOWNERS) and [the release flow](docs/wiki/release.md)); forks are free to
  build and ship under their own name.

A new dependency needs its licence in `deny.toml`'s allow-list, or the gate fails. That is
deliberate — every licence is vetted before it ships in a binary.

## Security

Do not open a public issue for a security bug. See [SECURITY.md](SECURITY.md).

## Licensing of contributions

Unless you state otherwise, any contribution you intentionally submit for inclusion in this work, as
defined in the Apache-2.0 licence, shall be dual licensed as below, without any additional terms or
conditions.

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT licence ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.
