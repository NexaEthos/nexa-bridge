# Release — from a version to a signed, published build

Five workflows, one build, **two release approvals: signing and publication**. A release starts as a
reviewed **pull request** that names the version; its merge is **tagged** automatically; the tag builds a
**candidate** once, attested by GitHub and verified by people on their machines. Automated verification
precedes signing. The verified candidate is then **promoted** by digest: signed on macOS and Windows,
re-attested, given an updater manifest and published. Nothing is recompiled after the tag.

## The flow

```text
Release prepare (maintainer dispatch: X.Y.Z) ─► pull request release/vX.Y.Z ─► review, CI, merge
   generated files · automatic commit and PR                                          │
                                                                                     ▼
Release tag (push to main, CHANGELOG.md) ─► automatic annotated tag on the merge commit
                                                                                     │
                                                                                     ▼
Release candidate (tag push) ─► tag ✓ · native matrix · SBOM · provenance · candidate-vX.Y.Z
                                                                                     │
                                                                                     ▼
Release publish (candidate green; or maintainer dispatch: tag + run id)
   automatic verification ─► ✋ ① sign macOS / Windows ─► ✋ ② publish release and updater
```

✋ is a stop of the protected environment `release`. Approve the three pending platform-signing jobs
as one group after real-machine acceptance; approve publication separately after all signatures pass.
The version PR still requires ordinary review, CI and merge. Those source controls and the initial
maintainer dispatch are separate from the two release-environment approval rounds.

Preparation and tagging use `release-automation`, an environment restricted to the `main` branch with
only the release App's credentials and no required reviewer. Read-only candidate verification uses no
environment. Signing and publication retain the protected `release` environment.

### 0. The version — [`release-prepare.yml`](../../.github/workflows/release-prepare.yml) and [`release-tag.yml`](../../.github/workflows/release-tag.yml)

**Release prepare** is started by hand with the version (`X.Y.Z` or `X.Y.Z-N`). A pre-release is a number
up to 65535 because the Windows MSI (WiX) carries no other: `1.0.0-rc.1` fails its bundle, `1.0.0-1` builds
as MSI version `1.0.0.1`. Its first job holds no secret: it refuses a version that is already tagged or
already has a changelog section, and an empty
`## [Unreleased]`; it sets the workspace version with `cargo set-version` (cargo-edit) when the version
changes, renames `## [Unreleased]` into `## [X.Y.Z] - <date>` under a fresh empty one, and refuses any
change outside `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, `crates/desktop/Cargo.toml` and
`crates/connector/Cargo.toml`. The desktop manifest may only follow the new version of its internal `nexa-robot` and
`nexa-link` dependencies, the connector's only that of `nexa-link`; a semantic comparison rejects other changes. Its
second job automatically reads those five files, preserving the nested
manifest path, and writes **one commit through the GitHub API** (GitHub signs it) on
`release/vX.Y.Z`, then opens the pull request with the changelog section as its body. The token is the
**Nexa Bridge Release** GitHub App's, minted for the job: a pull request or a tag made with the workflow's
own token would start no workflow.

**Release tag** runs on every push to `main` that touches `CHANGELOG.md`. When the workspace version has
its section and no tag yet, it creates the annotated tag `vX.Y.Z` on that reviewed merge commit
through the API, with the App's token; the tag ruleset lets only maintainers and that App create `v*`
tags. The tag push starts the candidate.

### 1. The candidate — [`release-candidate.yml`](../../.github/workflows/release-candidate.yml)

Runs on a pushed tag `vX.Y.Z` (or `vX.Y.Z-N`). Three jobs:

- **`tag`** — the tag's commit is on `main`, the version in the tag equals the workspace version
  (`[workspace.package] version` in `Cargo.toml`; `tauri.conf.json` has no version of its own and reads
  it from the crate), and `CHANGELOG.md` has a `## [X.Y.Z]` section. Any of the three missing stops the
  run with the reason.
- **`build`** — the [native matrix](development.md#native-matrix), called as a reusable workflow: the
  same jobs, the same artifacts, one build. Beside each desktop bundle the matrix stores the **compiled
  bytes** (`bin/nexa-desktop`, the two sidecars) so the signing stage can sign and re-wrap them without
  compiling again.
- **`candidate`** — downloads every target, drops the compiled-only Windows ARM64 CLI (not a supported
  platform; it stays a matrix artifact), writes `candidate.json` (tag, commit, run id, workflow), generates
  the **SBOM** of the source at the tag (SPDX JSON, syft), writes one `SHA256SUMS` over everything, then
  asks GitHub for a **provenance attestation** of every file and an **SBOM attestation** of every binary.
  The result is the artifact `candidate-vX.Y.Z`, kept 90 days.

The candidate is unsigned. That is the point: it is what testers run on each platform before anyone
asks for a signature.

### 2. The acceptance

A person downloads `candidate-vX.Y.Z` from the run and, per platform, records the verification in
[desktop.md](desktop.md#verification-record): install from the bundle, the tray, autostart, the keystore,
pairing in a browser, connect and serve one request, stop and relaunch. Unsigned bundles need the usual
local override (`xattr -d com.apple.quarantine` on macOS, "Run anyway" on Windows); a signed build must
not. A candidate that fails is not fixed in place: the fix lands on `main`, a new tag is pushed, a new
candidate is built.

### 3. Promotion — [`release-publish.yml`](../../.github/workflows/release-publish.yml)

Starts by itself when a *Release candidate* run started by a tag push of this repository ends green
(`workflow_run`), with that run's tag and **run id**; it can also be started by hand (`workflow_dispatch`)
with the tag and the run id of an earlier candidate. Verification runs automatically with read-only
permissions and no environment secrets. The reviewers approve signing after acceptance and publication
after signing; both stages retain the environment `release`:

1. **`verify`** — the run id's head commit is the tag's commit, the run is green, it is a *Release
   candidate* run started by the tag push; the artifact is downloaded **by run id, never rebuilt**;
   `sha256sum --check --strict` over the whole list; `candidate.json` names this tag and this run; every
   file's provenance is verified with `gh attestation verify` against this repository *and* the candidate
   workflow, every binary's SBOM attestation too. The verified set moves on as a run artifact. This job
   cannot sign, publish or mint the release App's token.
2. **`sign-macos`** (ARM64 and Intel) — the Developer ID identity is imported into a keychain made for the job
   (a bundle macOS cannot read, OpenSSL 3's PBES2 encoding, is re-encoded in the job with a warning; a password
   that opens nothing stops the job with that reason); the compiled bytes are placed where the Tauri bundler reads
   them and `tauri bundle` signs the app with that identity (hardened runtime, timestamp),
   notarizes it, staples the ticket and re-creates the disk image around the signed app; the CLI binaries
   are signed and notarized the same way (a bare binary cannot be stapled; the ticket is online). Gatekeeper's
   own verdict (`spctl --assess`) is printed. **`sign-windows`** (x64) — the sidecars and command-line executables are
   signed with Azure Artifact Signing (SHA-256, RFC 3161 timestamp); the Tauri bundler re-creates the
   installers around them, marking and signing the desktop executable for each package
   ([why](#the-updater-channel)) and signing the installers with the same identity; every signature, the
   installers' embedded executables included, is checked with `Get-AuthenticodeSignature`.
   Each job writes a **signing report** (`signing-report-<target>.json`): the digests of the candidate
   bytes it started from, the digests of what it produced, which candidate files those replace, the tools.
   That is where a packaging or signing difference is tracked — the compiled bytes are the candidate's,
   the wrappers around them are new.
3. **`publish`** — assembles the release set (Linux packages and binaries as built; macOS and Windows as
   signed; the SBOM, `candidate.json`, the signing reports), gives every asset one stable name without the
   version (`nexa-bridge-<target>.<ext>`, `nexa-llm-bridge-<target>`), signs the updater bundles and writes
   [`latest.json`](#the-updater-channel), writes `SHA256SUMS` and
   `release-manifest.json` (asset → source → digest), asks GitHub for a provenance attestation of the
   **published bytes**, writes the notes from the changelog section plus the verification commands, and
   publishes the release with every asset — marked *latest*, or *pre-release* for `X.Y.Z-N`. The
   approval of this job is the person's word that makes it public: a published release is what installed
   apps update to. Because the names are stable, a page links one system's installer at
   `https://github.com/NexaEthos/nexa-bridge/releases/latest/download/<name>`.

Linux packages are not signed by a key of ours; their verifiability is the provenance attestation and the
checksum list. A detached signature would need a key to manage; the attestation needs none.

### The updater channel {#the-updater-channel}

The installed app updates itself from `releases/latest/download/latest.json` ([desktop.md](desktop.md#updates)).
The `publish` job writes that manifest: it signs each updater bundle of the release set with the updater key
(`tauri signer sign --app-version <version>`, so the signature names the version it was made for) and lists,
per platform key, the bundle's URL **under this tag** and its signature. The notes are the changelog section.
`latest.json` is a release asset like the others: it is in `SHA256SUMS`, in `release-manifest.json` and in
the provenance attestation. `latest` resolves only to a published release that is not a pre-release, so
installed apps see an update when a stable release is published, and never a pre-release.

| Platform key | Asset |
| --- | --- |
| `darwin-aarch64-app`, `darwin-x86_64-app` | `nexa-bridge-<target>.app.tar.gz` — the signed, notarized app |
| `windows-x86_64-nsis` | `nexa-bridge-x86_64-pc-windows-msvc-setup.exe` |
| `windows-x86_64-msi` | `nexa-bridge-x86_64-pc-windows-msvc.msi` |
| `linux-x86_64-appimage`, `linux-aarch64-appimage` | `nexa-bridge-<target>.AppImage` |
| `linux-x86_64-deb`, `linux-aarch64-deb` | `nexa-bridge-<target>.deb` |

The app learns which installer it came from by a marker the Tauri bundler writes into the desktop
executable for each package. On Windows that write changes the executable after any signature, so the
`sign-windows` job hands the bundler the candidate's **unsigned** executable and a sign command
([`artifact-sign.ps1`](../../.github/scripts/artifact-sign.ps1), the same Azure identity): for each package
the bundler marks the executable and signs it, then signs the uninstaller and the installer. The Azure
action signs the sidecars and the command-line executables before that. The job then extracts both
installers (`msiexec /a`, 7-Zip) and checks that every executable inside carries a valid signature and that
the desktop executable carries its package's marker; the digests of those embedded executables are in the
signing report under `embedded`.

## Verifying a download

```sh
gh attestation verify nexa-bridge-aarch64-apple-darwin.dmg --repo NexaEthos/nexa-bridge
sha256sum --check --ignore-missing SHA256SUMS
```

The attestation binds the file's digest to this repository, the publish workflow and the tag's commit;
`gh` checks it against GitHub's Sigstore instance. `signing-report-*.json` in the same release says which
candidate bytes each signed file was made from, and `candidate.json` which run built them.

## What a fork gets and what it does not

- Pull requests run `CI` and `Native` with no secrets and no privileged runner. There is no
  `pull_request_target` anywhere; fork code never runs with credentials.
- The release workflows run only on a pushed tag (the candidate), on a push to `main` (the tag), when a
  candidate of this repository's own tag push ends green (the publish) or by `workflow_dispatch` by someone
  with write access (the prepare, the publish). The App's key is confined to the main-only
  `release-automation` environment; signing credentials remain in the reviewed `release` environment.
  Neither environment admits pull-request refs, tags or non-main branches.
- A fork can run the same workflows with its own certificates, its own updater key and its own name. The provenance it gets is
  its own repository's, and the name and identifier rules in [the README](../../README.md#official-builds-and-the-name)
  apply.

## Release environment configuration

Create both environments and their branch policies before merging workflows that reference them.
GitHub otherwise creates missing environments without protection. Both admit only the branch `main`,
not tags. `release` requires the maintainer reviewer and disables administrator bypass;
`release-automation` has no required reviewer and contains only the App client id and private key.
Missing inputs fail explicitly; source configuration alone does not establish the live protections.

| Where | Name | What it is |
| --- | --- | --- |
| Repository rules | tag ruleset `v*` | only maintainers and the release App create tags; no deletion, no update |
| GitHub App | **Nexa Bridge Release** | installed on this repository only, *Contents* and *Pull requests* write; it opens the release pull request and creates the tag |
| `release-automation` variable | `RELEASE_APP_CLIENT_ID` | the App's client id |
| `release-automation` secret | `RELEASE_APP_PRIVATE_KEY` | the App's private key; restricted to trusted main workflows, never a repository-wide secret |
| Environment | `release` | main only, required maintainer reviewer, no administrator bypass; the signing secrets and variables below live here |
| Secret | `APPLE_CERTIFICATE` | the *Developer ID Application* certificate with its key, as a base64 `.p12` |
| Secret | `APPLE_CERTIFICATE_PASSWORD` | the `.p12` password |
| Secret | `APPLE_SIGNING_IDENTITY` | the identity string, `Developer ID Application: <name> (<team id>)` |
| Secret | `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | the Apple account for `notarytool` (an app-specific password) and the team |
| Secret | `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_SUBSCRIPTION_ID` | identifiers for the federated application; no client secret; role *Artifact Signing Certificate Profile Signer* scoped to the certificate profile |
| Variable | `AZURE_SIGNING_ENDPOINT` | the account's regional endpoint, e.g. `https://weu.codesigning.azure.net` |
| Variable | `AZURE_SIGNING_ACCOUNT`, `AZURE_SIGNING_PROFILE` | the signing account and the certificate profile (public trust) |
| Secret | `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | the updater's minisign private key and its password; the public key is `plugins.updater.pubkey` in `tauri.conf.json` |

### Moving from the five-approval flow

Finish in-flight releases first. Provision `release-automation` with its exact `main` branch policy and
no required reviewer; copy only `RELEASE_APP_CLIENT_ID` and `RELEASE_APP_PRIVATE_KEY` into it. Read back
the protection settings and credential names without exposing values, then merge the reviewed workflow
change. Remove the two App inputs from `release` after confirming no earlier prepare/tag job needs them.
Apple, Azure, updater inputs and the Azure federation remain unchanged.

The deliberate trust change is that workflows on reviewed `main` can now use the App without a separate
approval. The App still has only its existing repository installation and Contents/Pull requests rights;
there is no new main bypass or tag update/delete permission. Protect review of workflow changes as
carefully as source changes: a main-only environment restricts the ref, not the workflow filename.

Validate the next release by checking that prepare/tag/verify never wait for an environment review,
that signing waits until its candidate verification has passed, and that publication waits separately
for the maintainer after all signing jobs succeed. A rollback restores the App inputs to `release`
and the prior workflow references; it does not modify existing tags or published assets.

Not in this flow: store submissions and a GPG signature for Linux packages.

## Windows workload identity

The Windows signing job requests a GitHub OIDC token and authenticates through `azure/login`.
The signing action uses only the resulting Azure CLI credential; other credential sources are
explicitly excluded. No persistent application password is required.

The Entra federation trusts issuer `https://token.actions.githubusercontent.com`, audience
`api://AzureADTokenExchange` and the immutable subject
`repo:NexaEthos@206276720/nexa-bridge@1426724024:environment:release` (the repository emits subjects with the
owner's and the repository's numeric ids, so a repository recreated under the same name cannot present it).
This subject selects the repository environment, not a workflow filename. Any job admitted to
that environment and given `id-token: write` can request this identity. Keep the environment
restricted to `main`, with maintainer approval and administrator bypass disabled; review all
workflow changes accordingly. Forks must configure their own issuer subject and signing profile.

Grant the built-in Certificate Profile Signer role at the individual profile resource, never at
account or subscription scope. The role includes metadata read and deployment operations within
that scope as well as its signing data action. It does not restrict which binary can be signed;
reviewed workflows, verified candidate digests and release approval enforce the artifact boundary.

Release assembly requires exactly one source file for each final asset name. A missing or
ambiguous match fails before publication, preserving the candidate-to-release mapping.

## Rules

- **One build.** The candidate is built once by the tag push. The publish workflow refuses a run id
  whose commit is not the tag's, and downloads what that run stored. A rerun of the candidate on the same
  tag is a new run id; promote that one or none.
- **Digest, not name.** Every hop checks `SHA256SUMS` and the attestation before it touches a file.
- **Differences are written down.** Signing changes bytes; the signing report says which input became which
  output. The compiled binaries are never recompiled between the candidate and the release.
- **A rerun of the publish** on a tag that already has a release fails at `gh release create`: a published
  release is not replaced by a workflow.
- **No tag without its version.** The tag is made from the workspace version and its changelog section on
  `main`, never typed; the candidate checks both again.
- **Secrets stay in their environments.** App automation and artifact signing use separate main-only
  environments. No workflow that a pull request can trigger reads them; no job prints them; the macOS
  keychain is created for the job and deleted after it.

## Changelog

- 2026-10-24 — Reduce release approval rounds to signing and publication; isolate the release App in
  main-only automation and leave candidate verification read-only without environment access.

- 2026-10-20 — The Windows federation names this repository's id: the repository was born again under
  the same name, so its subject is new.
- 2026-10-19 — The connector and the desktop depend on `nexa-link`: release preparation lets its version follow the
  release in both manifests and commits the connector's manifest with the other four files.
- 2026-10-19 — Release preparation accepts only `X.Y.Z` or `X.Y.Z-N` (N up to 65535): the `1.0.0-rc.1`
  release pull request failed its Windows MSI bundle, which takes only a numeric pre-release.
- 2026-10-19 — Release preparation preserves the desktop manifest when cargo-edit updates its internal
  robot version across a minor-version boundary; other changes to that manifest remain rejected.
- 2026-10-17 — The Windows federation trusts the immutable OIDC subject (owner and repository ids); the old
  name-based subject failed `azure/login` with AADSTS700213 on the 0.1.2 promotion.
- 2026-10-17 — The macOS identity lives in one job keychain: imported once for the app and the CLI, re-encoded in
  the job when macOS cannot read the `.p12` (the 0.1.2 promotion stopped on «MAC verification failed»).
- 2026-10-17 — The release cycle runs by itself between approvals: *Release prepare* opens the version's
  pull request, *Release tag* tags its merge, *Release publish* starts when the candidate is green and
  publishes the release after its last approval instead of leaving a draft.
- 2026-10-17 — The updater channel: `publish` signs the updater bundles for their version and writes
  `latest.json`; on Windows the bundler marks and signs the desktop executable per package, and the job checks
  the executables inside both installers.
- 2026-10-17 — Prepare Windows signing through environment-scoped OIDC and document its trust boundary.

- 2026-10-09 — Asset names carry no version, so a download page can link one stable name: a download link is
  `releases/latest/download/nexa-bridge-<target>.<ext>` and keeps working across releases. The version stays in
  the tag, the release title, `candidate.json`, `release-manifest.json` and the SBOM's name.
- 2026-10-08 — The two workflows and this page: the candidate on a tag with checksums, SBOM and
  provenance; the promotion by run id with verification, separate signing jobs, signing reports, a second
  attestation and a draft release; the inputs still to provide listed. The native matrix became callable
  and stores the compiled bytes beside each bundle; `tauri.conf.json` reads the version from the crate.
