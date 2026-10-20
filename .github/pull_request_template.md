## What

<!-- One change. What it does and why; link the wiki page it updates. -->

## Verified

<!-- The gate (docs/wiki/development.md#the-ci-gate) run locally: toolchain, OS and architecture.
     Say what you did NOT verify — a build that only compiled is not a tested platform. -->

- [ ] `cargo fmt`, `check`, pedantic `clippy`, `test`, release build, `node --check`, `cargo deny` green locally
- [ ] a behaviour change updates its `docs/wiki/` page in the same commit
- [ ] no new lint suppression, `allow`, skipped check or dependency exception
- [ ] nothing that identifies a particular deployment (hosts, IPs, people, internal references)
