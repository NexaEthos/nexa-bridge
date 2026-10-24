# GLib string iterator backport

The Linux desktop depends on GTK 0.18 and WebKitGTK 2.0.2 through Tauri. They require
`glib ^0.18`, whose latest published patch is 0.18.5. The server bridge, connector and robot
binaries do not use GLib; neither do the macOS and Windows desktop dependency graphs.

[RUSTSEC-2024-0429](https://rustsec.org/advisories/RUSTSEC-2024-0429.html) describes undefined
behavior in `VariantStrIter::impl_get`: a C output argument receives an immutable reference,
which can leave a null pointer and crash an optimized build. Source inspection found no use of
`VariantStrIter` or `array_iter_str` outside GLib in the resolved dependencies, but that is not
a complete dynamic reachability proof. The backport removes the defect regardless of reachability.

## Provenance

`vendor/glib` contains the complete published crate, including its original MIT license and
manifests, with only the two upstream mutability changes below. The package remains named `glib`
at version `0.18.5`. It is a third-party dependency, not a workspace member; its upstream comments,
formatting, tests and unsafe implementation are preserved rather than rewritten as application code.

| Evidence | Value |
| --- | --- |
| Original archive | `https://static.crates.io/crates/glib/glib-0.18.5.crate` |
| Archive SHA-256, verified against crates.io | `233daaf6e83ae6a12a52055f568f9d7cf4671dabb78ff9560ab6da230ce00ee5` |
| Original source commit | `42b9caf98e03ded086362d9653ca58fe94dc8658` |
| Upstream correction | [gtk-rs-core pull request 1343](https://github.com/gtk-rs/gtk-rs-core/pull/1343) |
| Correction commit | `05dff0ee696f9bcd8617cd48c4b812d046d440cb` |
| License | MIT, preserved in `vendor/glib/LICENSE` |

The complete source delta is:

```diff
--- glib-0.18.5/src/variant_iter.rs
+++ vendor/glib/src/variant_iter.rs
@@ -118,12 +118,12 @@
     fn impl_get(&self, i: usize) -> &'a str {
         unsafe {
-            let p: *mut libc::c_char = std::ptr::null_mut();
+            let mut p: *mut libc::c_char = std::ptr::null_mut();
             let s = b"&s\0";
             ffi::g_variant_get_child(
                 self.variant.to_glib_none().0,
                 i,
                 s as *const u8 as *const _,
-                &p,
+                &mut p,
                 std::ptr::null::<i8>(),
             );
             let p = std::ffi::CStr::from_ptr(p);
```

`vendor/glib-backport.json` records the hashes of every original and patched file. Its source
hashes were generated from the verified archive, not from a Cargo cache with unknown local edits.
`vendor/.gitattributes` preserves the crate's exact bytes on every checkout, including Windows.
`.cargo/config.toml` selects this copy through Cargo's `[patch.crates-io]` mechanism; Cargo
generates the corresponding path entry in `Cargo.lock`. The manifests are otherwise unchanged.

## Verification and advisory coverage

Run:

```sh
python3 .github/scripts/verify_glib_backport.py
cargo deny --all-features --locked check -D warnings
```

The verifier rejects missing, added, modified or symlinked vendor files, a different Cargo patch,
a registry GLib lockfile entry, an extra GLib version, or loss of `unsound = "all"`. It runs before
the advisory scanner and in the desktop native matrix, including release candidates. Its expected
hash inventory is reviewed with the source change; changing both is not independent proof of
upstream provenance and requires rechecking the published archive and upstream correction.

`unsound = "all"` explicitly includes transitive unsound advisories. Cargo-deny 0.20.2 otherwise
defaults to workspace scope for this category. With the registry 0.18.5 it reports this advisory;
with the patched local source its advisory check passes without an ignored ID. The local source
is therefore verified separately: a green advisory scan alone does not certify the vendor bytes.
No advisory exception, crate rename, version change or source allow-list relaxation is used.

This CI check protects dependency integrity and memory safety; it does not impose a model limit,
truncate information or affect a person's application workflow. A contributor can reproduce a
failure locally, inspect the reported paths and restore the reviewed source or submit a reviewed
dependency update. The User authorized the correction and expanded advisory coverage.

On Linux, also run the regression in an optimized build:

```sh
cargo test -p nexa-desktop --release --locked --test glib_variant
```

The test exercises forward, reverse and mixed iteration, `nth`, `nth_back`, `last`, empty strings,
Unicode and an empty array. The native matrix runs it on Linux x64 and ARM64, after the usual
desktop tests and before bundling. GLib is a Linux-only test dependency, so this does not add it
to the macOS or Windows application. An optimized test is necessary because the upstream defect
depends on compiler optimization of the C output argument.

The same regression was also run in isolation on macOS ARM64 with Rust 1.99.1 and native GLib
2.90.1, using the original registry source and then the reviewed path override. The original
optimized test process terminated with SIGSEGV; the patched test passed. This demonstrates the
iterator correction, not Linux desktop acceptance. The unmodified upstream crate emits compiler
style warnings on this toolchain; no lint suppression or unrelated vendor edit is included.

## Removing the backport

When Tauri's complete GTK/WebKit dependency chain supports a fixed upstream GLib release
(`>=0.20.0`), update through Cargo and verify the Linux graph and optimized regression. Remove
the local patch, vendor source, provenance inventory and verifier together. Keep transitive
unsound advisory coverage enabled. Do not force an incompatible GLib release into GTK 0.18,
rename the dependency or change its version just to make the advisory disappear.

## Changelog

- 2026-10-24 — User-authorized upstream backport and explicit transitive unsound coverage;
  Architect-approved vendoring scope, provenance verification and optimized Linux regression.
