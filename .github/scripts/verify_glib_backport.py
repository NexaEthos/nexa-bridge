#!/usr/bin/env python3
"""Wiki: docs/wiki/glib-backport.md."""

import hashlib
import json
import sys
import tomllib
from pathlib import Path


def verify(root: Path) -> None:
    """Verify the reviewed source, Cargo override, lockfile and advisory policy."""
    provenance = json.loads((root / "vendor/glib-backport.json").read_text())
    vendor = root / "vendor/glib"
    paths = list(vendor.rglob("*"))
    if any(path.is_symlink() for path in paths):
        message = "The vendored GLib source must not contain symlinks"
        raise ValueError(message)
    actual = {
        path.relative_to(vendor).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest()
        for path in paths
        if path.is_file()
    }
    expected = provenance["patched_files_sha256"]
    changed = sorted(path for path in actual.keys() | expected.keys() if actual.get(path) != expected.get(path))
    if changed:
        message = f"GLib source differs from the reviewed backport: {', '.join(changed)}"
        raise ValueError(message)

    config = tomllib.loads((root / ".cargo/config.toml").read_text())
    if config.get("patch", {}).get("crates-io", {}).get("glib") != {"path": "vendor/glib"}:
        message = "Cargo must select the reviewed vendor/glib override"
        raise ValueError(message)
    lock = tomllib.loads((root / "Cargo.lock").read_text())
    packages = [package for package in lock["package"] if package["name"] == "glib"]
    if len(packages) != 1 or packages[0]["version"] != "0.18.5" or "source" in packages[0]:
        message = "Cargo.lock must resolve only the reviewed local GLib 0.18.5"
        raise ValueError(message)
    policy = tomllib.loads((root / "deny.toml").read_text())["advisories"]
    if policy.get("unsound") != "all":
        message = "cargo-deny must check transitive unsound advisories"
        raise ValueError(message)


if __name__ == "__main__":
    try:
        verify(Path(__file__).resolve().parents[2])
    except (KeyError, OSError, TypeError, ValueError) as error:
        message = f"GLib backport verification failed: {error}"
        raise SystemExit(message) from error
    sys.stdout.write("GLib backport source, Cargo resolution and advisory coverage verified\n")
