# Verified dependency version splits

The User approved a package-scoped duplicate-version exception. It does not waive vulnerabilities,
yanked crates, license policy, unknown sources, or warnings in workspace code.

Clippy can only exempt package names; `clippy.toml` lists the names actually reported by its cargo
lint. The separate `deny.toml` gate rejects duplicate versions by default and exempts only exact
older versions present in the active dependency graph. A dependency update must recheck both lists;
unused cargo-deny exceptions fail the warning-as-error gate and must be removed.
There is no blanket `allow`, subtree exclusion, or Rust lint suppression.

These splits come from the GUI framework and platform bindings, configuration/code-generation
libraries, and HTTP/cryptographic stacks requiring incompatible major versions. Updating our direct
dependency cannot force those consumers onto an incompatible API. For example, the connector uses
tokio-tungstenite 0.30 while axum requires 0.29; reqwest requires tower-http 0.6 while the bridge uses 0.7.

## Held back on purpose

A transitive crate is held at an older patch (`cargo update -p <crate> --precise <version>`) only when
its newer release would open a split in a package that is not on the approved list. Today:

| Crate | Held at | Why | Released when |
| --- | --- | --- | --- |
| `flate2` | `1.1.9` | `1.1.10` moves to `miniz_oxide` 0.9 while `png` 0.17/0.18 still use 0.8 | `png` (via `ico`, `muda`, `tray-icon`) adopts `miniz_oxide` 0.9 |
| `tauri-plugin-shell` | `2.4.1` | `2.5.0` depends on `windows-sys` 0.60 while the rest of the graph is on 0.61; 2.5.0 adds only the `processGroup` option and an iOS link fix, neither used here | the plugin moves to `windows-sys` 0.61 |

Dependabot will propose the bump again; the gate's `deny` job refuses it until the row above is
released, and the refusal is the reminder to look.

## Reproducing the inventory

```sh
cargo metadata --format-version 1 --all-features --locked
cargo tree --duplicates --all-features --target all --depth 1 --no-dedupe --locked
cargo deny --all-features --locked check -D warnings
```

The metadata graph also contains optional dependencies that Clippy examines but cargo-deny prunes
from the enabled graph, notably additional schemars and windows-core generations.
The table records immediate consumers, not a claim that every platform has been built locally.

| Package | Version | Immediate consumers |
| --- | --- | --- |
| `base64` | `0.21.7` | `swift-rs 1.0.8` |
| `base64` | `0.22.1` | `axum 0.8.9`, `tauri-codegen 2.7.1`, `wry 0.57.0` |
| `base64` | `0.23.1` | `hyper-util 0.1.21`, `plist 1.10.1`, `reqwest 0.13.5`, `serde_with 3.24.0` |
| `bitflags` | `1.3.2` | `defmt 1.1.1`, `javascriptcore-rs 1.1.2`, `png 0.17.16`, `webkit2gtk 2.0.2`, `webkit2gtk-sys 2.0.2` |
| `bitflags` | `2.13.2` | `cairo-rs 0.18.5`, `core-graphics 0.25.0`, `core-graphics-types 0.2.0`, `dispatch2 0.3.1`, `glib 0.18.5`, `keyboard-types 0.8.3`, `ndk 0.9.0`, `nix 0.31.3`, `objc2-app-kit 0.3.2`, `objc2-cloud-kit 0.3.2`, `objc2-core-foundation 0.3.2`, `objc2-core-graphics 0.3.2`, `objc2-core-text 0.3.2`, `objc2-foundation 0.3.2`, `objc2-io-surface 0.3.2`, `objc2-quartz-core 0.3.2`, `objc2-security 0.3.2`, `objc2-ui-kit 0.3.2`, `objc2-web-kit 0.3.2`, `png 0.18.1`, `raw-cpuid 11.6.0`, `redox_syscall 0.5.18`, `rustix 1.1.5`, `security-framework 3.7.0`, `selectors 0.38.0`, `tao 0.37.1`, `tower-http 0.6.11`, `tower-http 0.7.1` |
| `block-buffer` | `0.10.4` | `digest 0.10.7` |
| `block-buffer` | `0.12.1` | `digest 0.11.3` |
| `cpufeatures` | `0.2.17` | `sha1 0.10.7`, `sha2 0.10.9` |
| `cpufeatures` | `0.3.1` | `aes 0.9.3`, `chacha20 0.10.2`, `sha1 0.11.0`, `sha2 0.11.0` |
| `crypto-common` | `0.1.7` | `digest 0.10.7` |
| `crypto-common` | `0.2.2` | `cipher 0.5.2`, `digest 0.11.3` |
| `digest` | `0.10.7` | `sha1 0.10.7`, `sha2 0.10.9` |
| `digest` | `0.11.3` | `hmac 0.13.0`, `sha1 0.11.0`, `sha2 0.11.0` |
| `dirs` | `6.0.0` | `auto-launch 0.6.0` |
| `dirs` | `7.0.0` | `tauri 2.12.1`, `tauri-build 2.7.1`, `tray-icon 0.25.1`, `wry 0.57.0` |
| `getrandom` | `0.2.17` | `ring 0.17.14` |
| `getrandom` | `0.3.4` | `governor 0.10.4`, `rand_core 0.9.5`, `tauri 2.12.1` |
| `getrandom` | `0.4.3` | `nexa-desktop 0.1.0`, `rand 0.10.3`, `secret-service 5.2.0`, `tempfile 3.27.0`, `uuid 1.27.0` |
| `hashbrown` | `0.12.3` | `indexmap 1.9.3` |
| `hashbrown` | `0.14.5` | `dashmap 6.2.1` |
| `hashbrown` | `0.16.1` | `governor 0.10.4` |
| `hashbrown` | `0.17.1` | `indexmap 2.14.2` |
| `heck` | `0.4.1` | `glib-macros 0.18.5` |
| `heck` | `0.5.0` | `system-deps 6.2.2`, `tauri 2.12.1`, `tauri-build 2.7.1`, `tauri-macros 2.7.1` |
| `indexmap` | `1.9.3` | `schemars 0.8.22`, `serde_with 3.24.0` |
| `indexmap` | `2.14.2` | `h2 0.4.20`, `plist 1.10.1`, `serde_with 3.24.0`, `toml 1.1.7+spec-1.1.0`, `toml_edit 0.19.15`, `toml_edit 0.20.2`, `toml_edit 0.25.16+spec-1.1.0` |
| `jni` | `0.21.1` | `tao 0.37.1`, `tauri 2.12.1`, `tauri-runtime 2.12.1`, `tauri-runtime-wry 2.12.1`, `wry 0.57.0` |
| `jni` | `0.22.4` | `rustls-platform-verifier 0.7.1` |
| `jni-sys` | `0.3.1` | `jni 0.21.1`, `ndk 0.9.0`, `ndk-sys 0.6.0+11769913` |
| `jni-sys` | `0.4.1` | `jni 0.22.4`, `jni-sys 0.3.1` |
| `png` | `0.17.16` | `ico 0.5.0` |
| `png` | `0.18.1` | `muda 0.20.0`, `tauri-codegen 2.7.1`, `tray-icon 0.25.1` |
| `proc-macro-crate` | `1.3.1` | `gtk3-macros 0.18.2` |
| `proc-macro-crate` | `2.0.2` | `glib-macros 0.18.5` |
| `proc-macro-crate` | `3.5.0` | `num_enum_derive 0.7.6`, `zbus_macros 5.19.0`, `zvariant_derive 5.15.0` |
| `r-efi` | `5.3.0` | `getrandom 0.3.4` |
| `r-efi` | `6.0.0` | `getrandom 0.4.3` |
| `rand` | `0.9.5` | `governor 0.10.4`, `tungstenite 0.29.0` |
| `rand` | `0.10.3` | `tungstenite 0.30.0` |
| `rand_core` | `0.9.5` | `rand 0.9.5`, `rand_chacha 0.9.0` |
| `rand_core` | `0.10.1` | `chacha20 0.10.2`, `getrandom 0.4.3`, `rand 0.10.3` |
| `schemars` | `0.8.22` | `tauri-build 2.7.1`, `tauri-plugin 2.7.1`, `tauri-plugin-opener 2.7.0`, `tauri-plugin-shell 2.4.1`, `tauri-utils 2.10.1` |
| `schemars` | `0.9.0` | `serde_with 3.24.0` |
| `schemars` | `1.2.2` | `serde_with 3.24.0` |
| `serde_spanned` | `0.6.9` | `toml 0.8.2`, `toml_edit 0.20.2` |
| `serde_spanned` | `1.1.2` | `toml 1.1.7+spec-1.1.0` |
| `sha1` | `0.10.7` | `axum 0.8.9`, `tungstenite 0.29.0` |
| `sha1` | `0.11.0` | `tungstenite 0.30.0` |
| `sha2` | `0.10.9` | `tauri-codegen 2.7.1`, `wry 0.57.0` |
| `sha2` | `0.11.0` | `nexa-desktop 0.1.0`, `nexa-llm-bridge 0.1.0`, `secret-service 5.2.0` |
| `syn` | `1.0.109` | `proc-macro-error 1.0.4` |
| `syn` | `2.0.119` | `defmt-macros 1.1.1`, `derive_more-impl 2.1.1`, `dlopen2_derive 0.4.3`, `enumflags2_derive 0.7.12`, `glib-macros 0.18.5`, `gtk3-macros 0.18.2`, `jiff-static 0.2.38`, `jni-macros 0.22.4`, `jni-sys-macros 0.4.1`, `num_enum_derive 0.7.6`, `pear_codegen 0.2.9`, `phf_macros 0.13.1`, `proc-macro2-diagnostics 0.10.1`, `schemars_derive 0.8.22`, `serde_derive_internals 0.29.1`, `serialize-to-javascript-impl 0.1.2`, `tao-macros 0.1.4`, `tauri-codegen 2.7.1`, `tauri-macros 2.7.1`, `thiserror-impl 1.0.69`, `tracing-attributes 0.1.31`, `webview2-com-macros 0.8.1`, `windows-implement 0.60.2`, `windows-interface 0.59.3`, `zerocopy-derive 0.8.62` |
| `syn` | `3.0.6` | `async-recursion 1.2.0`, `async-trait 0.1.92`, `cssparser-macros 0.7.1`, `darling_core 0.24.1`, `darling_macro 0.24.1`, `displaydoc 0.2.7`, `foreign-types-macros 0.2.4`, `futures-macro 0.3.34`, `ref-cast-impl 1.0.27`, `serde_derive 1.0.229`, `serde_repr 0.1.21`, `serde_with_macros 3.24.0`, `synstructure 0.14.0`, `thiserror-impl 2.0.21`, `tokio-macros 2.7.2`, `wasm-bindgen-macro-support 0.2.129`, `yoke-derive 0.8.4`, `zbus_macros 5.19.0`, `zerofrom-derive 0.1.8`, `zerovec-derive 0.11.6`, `zvariant_derive 5.15.0`, `zvariant_utils 4.2.0` |
| `thiserror` | `1.0.69` | `cairo-rs 0.18.5`, `gio 0.18.4`, `glib 0.18.5`, `jni 0.21.1`, `ndk 0.9.0` |
| `thiserror` | `2.0.21` | `auto-launch 0.6.0`, `cargo_metadata 0.19.2`, `defmt-parser 1.0.0`, `jni 0.22.4`, `json-patch 4.2.0`, `muda 0.20.0`, `redox_users 0.5.3`, `smappservice-rs 0.1.3`, `tauri 2.12.1`, `tauri-codegen 2.7.1`, `tauri-plugin-autostart 2.7.0`, `tauri-plugin-opener 2.7.0`, `tauri-plugin-shell 2.4.1`, `tauri-plugin-single-instance 2.5.3`, `tauri-runtime 2.12.1`, `tauri-utils 2.10.1`, `tray-icon 0.25.1`, `tungstenite 0.29.0`, `tungstenite 0.30.0`, `webview2-com-sys 0.39.1`, `wry 0.57.0` |
| `thiserror-impl` | `1.0.69` | `thiserror 1.0.69` |
| `thiserror-impl` | `2.0.21` | `thiserror 2.0.21` |
| `tokio-tungstenite` | `0.29.0` | `axum 0.8.9` |
| `tokio-tungstenite` | `0.30.0` | `nexa-llm-connector 0.1.0` |
| `toml` | `0.8.2` | `figment 0.10.19`, `system-deps 6.2.2` |
| `toml` | `1.1.7+spec-1.1.0` | `cargo_toml 1.0.1`, `embed-resource 3.0.12`, `tauri-utils 2.10.1`, `tauri-winres 0.3.6` |
| `toml_datetime` | `0.6.3` | `proc-macro-crate 2.0.2`, `toml 0.8.2`, `toml_edit 0.19.15`, `toml_edit 0.20.2` |
| `toml_datetime` | `1.1.2+spec-1.1.0` | `toml 1.1.7+spec-1.1.0`, `toml_edit 0.25.16+spec-1.1.0` |
| `toml_edit` | `0.19.15` | `proc-macro-crate 1.3.1` |
| `toml_edit` | `0.20.2` | `proc-macro-crate 2.0.2`, `toml 0.8.2` |
| `toml_edit` | `0.25.16+spec-1.1.0` | `proc-macro-crate 3.5.0` |
| `tower-http` | `0.6.11` | `reqwest 0.13.5` |
| `tower-http` | `0.7.1` | `nexa-llm-bridge 0.1.0` |
| `tungstenite` | `0.29.0` | `tokio-tungstenite 0.29.0` |
| `tungstenite` | `0.30.0` | `tokio-tungstenite 0.30.0` |
| `webpki-roots` | `0.26.11` | `tokio-tungstenite 0.30.0` |
| `webpki-roots` | `1.0.9` | `webpki-roots 0.26.11` |
| `windows-sys` | `0.45.0` | `jni 0.21.1` |
| `windows-sys` | `0.52.0` | `ring 0.17.14` |
| `windows-sys` | `0.60.2` | `tauri-plugin-shell 2.5.0` |
| `windows-sys` | `0.61.2` | `async-io 2.6.0`, `async-signal 0.2.14`, `dbus 0.9.12`, `dirs-sys 0.5.0`, `errno 0.3.14`, `mio 1.2.4`, `muda 0.20.0`, `nu-ansi-term 0.50.3`, `os_info 3.15.0`, `os_pipe 1.2.3`, `polling 3.11.0`, `rustix 1.1.5`, `rustls-platform-verifier 0.7.1`, `schannel 0.1.29`, `shared_child 1.1.2`, `socket2 0.6.5`, `softbuffer 0.4.8`, `tauri-plugin-single-instance 2.5.3`, `tempfile 3.27.0`, `tokio 1.53.3`, `tray-icon 0.25.1`, `uds_windows 1.2.1`, `winapi-util 0.1.11`, `window-vibrancy 0.8.1`, `windows-native-keyring-store 1.1.0`, `winreg 0.56.0`, `zbus 5.19.0` |
| `windows-targets` | `0.42.2` | `windows-sys 0.45.0` |
| `windows-targets` | `0.52.6` | `windows-sys 0.52.0` |
| `windows_aarch64_gnullvm` | `0.42.2` | `windows-targets 0.42.2` |
| `windows_aarch64_gnullvm` | `0.52.6` | `windows-targets 0.52.6` |
| `windows_aarch64_msvc` | `0.42.2` | `windows-targets 0.42.2` |
| `windows_aarch64_msvc` | `0.52.6` | `windows-targets 0.52.6` |
| `windows_i686_gnu` | `0.42.2` | `windows-targets 0.42.2` |
| `windows_i686_gnu` | `0.52.6` | `windows-targets 0.52.6` |
| `windows_i686_gnullvm` | `0.52.6` | `windows-targets 0.52.6` |
| `windows_i686_msvc` | `0.42.2` | `windows-targets 0.42.2` |
| `windows_i686_msvc` | `0.52.6` | `windows-targets 0.52.6` |
| `windows_x86_64_gnu` | `0.42.2` | `windows-targets 0.42.2` |
| `windows_x86_64_gnu` | `0.52.6` | `windows-targets 0.52.6` |
| `windows_x86_64_gnullvm` | `0.42.2` | `windows-targets 0.42.2` |
| `windows_x86_64_gnullvm` | `0.52.6` | `windows-targets 0.52.6` |
| `windows_x86_64_msvc` | `0.42.2` | `windows-targets 0.42.2` |
| `windows_x86_64_msvc` | `0.52.6` | `windows-targets 0.52.6` |
| `winnow` | `0.5.40` | `toml_edit 0.19.15`, `toml_edit 0.20.2` |
| `winnow` | `1.0.4` | `toml 1.1.7+spec-1.1.0`, `toml_edit 0.25.16+spec-1.1.0`, `toml_parser 1.1.4+spec-1.1.0`, `zbus 5.19.0`, `zbus_names 4.3.4`, `zvariant 5.15.0`, `zvariant_utils 4.2.0` |


## Changelog

- 2026-10-18 — User approved adding `toml` to the Clippy package list: desktop hardware now shares
  the robot library, joining Figment's existing 0.8.2 and Tauri's existing 1.1.7 in one graph.
  The exact cargo-deny exception was already present; no dependency version or other gate changed.
- 2026-10-17 — `tauri-plugin-shell` held at 2.4.1: 2.5.0 opens a `windows-sys` 0.60/0.61 split.
- 2026-09-06 — Recorded the User-approved, verified duplicate-version exception; all other gates remain blocking.
