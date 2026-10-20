//! Wiki: docs/wiki/release-review.md#build-diagnostics
#![forbid(unsafe_code)]
#![deny(warnings, rust_2018_idioms, rust_2021_compatibility)]
#![warn(missing_docs, missing_debug_implementations)]

use std::io::{self, Write as _};
use std::process::Command;
use std::{env, fs};

/// The channels a build may declare about itself — a claim, never an authorisation.
pub const CHANNELS: [&str; 2] = ["official", "custom"];

/// The channel a build declares: `custom` unless `NEXA_BUILD_CHANNEL` names one of [`CHANNELS`].
///
/// # Errors
/// A value outside [`CHANNELS`]: a typo must stop the build, not ship a third word.
pub fn channel(declared: Option<&str>) -> io::Result<&'static str> {
    match declared {
        None | Some("") => Ok("custom"),
        Some(value) => CHANNELS
            .iter()
            .copied()
            .find(|known| *known == value)
            .ok_or_else(|| {
                io::Error::other(format!(
                    "NEXA_BUILD_CHANNEL must be one of {CHANNELS:?}, got {value:?}"
                ))
            }),
    }
}

/// Emit the build block as `cargo:rustc-env` values; call it from a binary crate's `build.rs`.
///
/// # Errors
/// A missing cargo variable, a channel outside [`CHANNELS`], or stdout refusing the directives.
pub fn emit() -> io::Result<()> {
    let manifest_dir = cargo_var("CARGO_MANIFEST_DIR")?;
    let target = cargo_var("TARGET")?;
    let profile = cargo_var("PROFILE")?;
    let channel = channel(env::var("NEXA_BUILD_CHANNEL").ok().as_deref())?;
    let mut out = io::stdout().lock();
    writeln!(out, "cargo:rerun-if-env-changed=NEXA_BUILD_CHANNEL")?;
    for path in watched_git_paths(&manifest_dir) {
        writeln!(out, "cargo:rerun-if-changed={path}")?;
    }
    writeln!(
        out,
        "cargo:rustc-env=NEXA_BUILD_COMMIT={}",
        commit(&manifest_dir)
    )?;
    writeln!(out, "cargo:rustc-env=NEXA_BUILD_TARGET={target}")?;
    writeln!(out, "cargo:rustc-env=NEXA_BUILD_PROFILE={profile}")?;
    writeln!(out, "cargo:rustc-env=NEXA_BUILD_CHANNEL={channel}")?;
    Ok(())
}

fn cargo_var(name: &str) -> io::Result<String> {
    env::var(name).map_err(|error| io::Error::other(format!("{name}: {error}")))
}

fn git(manifest_dir: &str, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(manifest_dir)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The short revision, `-dirty` when tracked files differ from it, `unknown` without a repository.
fn commit(manifest_dir: &str) -> String {
    let Some(short) = git(manifest_dir, &["rev-parse", "--short=12", "HEAD"]) else {
        return "unknown".to_string();
    };
    let dirty = Command::new("git")
        .args(["diff", "--quiet", "HEAD", "--"])
        .current_dir(manifest_dir)
        .status()
        .is_ok_and(|status| !status.success());
    if dirty {
        format!("{short}-dirty")
    } else {
        short
    }
}

/// HEAD, the branch it points to and the packed refs: a new commit triggers a rebuild.
fn watched_git_paths(manifest_dir: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let Some(head) = git(manifest_dir, &["rev-parse", "--git-path", "HEAD"]) else {
        return paths;
    };
    if let Ok(content) = fs::read_to_string(&head)
        && let Some(reference) = content.strip_prefix("ref: ")
        && let Some(path) = git(manifest_dir, &["rev-parse", "--git-path", reference.trim()])
    {
        paths.push(path);
    }
    paths.push(head);
    if let Some(packed) = git(manifest_dir, &["rev-parse", "--git-path", "packed-refs"]) {
        paths.push(packed);
    }
    paths
        .into_iter()
        .filter(|path| fs::metadata(path).is_ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channel_is_custom_unless_declared_and_never_a_third_word() -> io::Result<()> {
        assert_eq!(channel(None)?, "custom");
        assert_eq!(channel(Some(""))?, "custom");
        assert_eq!(channel(Some("official"))?, "official");
        assert_eq!(channel(Some("custom"))?, "custom");
        assert!(channel(Some("beta")).is_err());
        assert!(channel(Some("Official")).is_err());
        Ok(())
    }

    #[test]
    fn the_commit_of_this_repository_is_known_and_a_directory_without_git_is_not() {
        let here = env!("CARGO_MANIFEST_DIR");
        let revision = commit(here);
        assert!(revision.len() >= 12, "{revision}");
        assert_ne!(watched_git_paths(here), Vec::<String>::new());
        let nowhere = env::temp_dir().join("nexa-build-info-no-git");
        fs::create_dir_all(&nowhere).ok();
        assert_eq!(commit(&nowhere.display().to_string()), "unknown");
        assert_eq!(
            watched_git_paths(&nowhere.display().to_string()),
            Vec::<String>::new()
        );
    }
}
