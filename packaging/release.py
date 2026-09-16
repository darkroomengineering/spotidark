#!/usr/bin/env python3
"""Small, deterministic helpers for Spotidark's continuous releases."""

from __future__ import annotations

import argparse
import re
from pathlib import Path


SEMVER = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")


def parse_version(text: str) -> tuple[int, int, int]:
    match = SEMVER.fullmatch(text.removeprefix("v"))
    if match is None:
        raise ValueError(f"expected MAJOR.MINOR.PATCH, got {text!r}")
    return tuple(int(part) for part in match.groups())


def package_version(manifest: Path) -> str:
    package = manifest.read_text(encoding="utf-8").split("[package]", 1)[1]
    package = package.split("\n[", 1)[0]
    match = re.search(r'^version = "([^"]+)"$', package, re.MULTILINE)
    if match is None:
        raise ValueError(f"missing package version in {manifest}")
    parse_version(match.group(1))
    return match.group(1)


def release_version(base: str, run_number: int) -> str:
    if run_number < 1:
        raise ValueError("GitHub run number must be positive")
    major, minor, patch = parse_version(base)
    release_patch = patch + run_number
    if max(major, minor, release_patch) > 65_535:
        raise ValueError(
            "release version exceeds Windows' 65535 field limit; start a new "
            "release version sequence before publishing"
        )
    return f"{major}.{minor}.{release_patch}"


def replace_once(text: str, old: str, new: str, source: Path) -> str:
    count = text.count(old)
    if count != 1:
        raise ValueError(f"expected one {old!r} in {source}, found {count}")
    return text.replace(old, new, 1)


def stamp_version(manifest: Path, lockfile: Path, version: str) -> None:
    parse_version(version)
    old_version = package_version(manifest)

    manifest_text = manifest.read_text(encoding="utf-8")
    package_prefix, package_and_rest = manifest_text.split("[package]", 1)
    package, rest = package_and_rest.split("\n[", 1)
    package = replace_once(
        package,
        f'version = "{old_version}"',
        f'version = "{version}"',
        manifest,
    )

    lock_text = lockfile.read_text(encoding="utf-8")
    package_marker = f'[[package]]\nname = "fastpotify"\nversion = "{old_version}"'
    stamped_marker = f'[[package]]\nname = "fastpotify"\nversion = "{version}"'
    lock_text = replace_once(lock_text, package_marker, stamped_marker, lockfile)

    manifest.write_text(package_prefix + "[package]" + package + "\n[" + rest, encoding="utf-8")
    lockfile.write_text(lock_text, encoding="utf-8")


def require_newer(candidate: str, latest: str) -> None:
    if parse_version(candidate) <= parse_version(latest):
        raise ValueError(f"release {candidate} must be newer than latest {latest}")


def verify_assets(directory: Path, expected: set[str]) -> None:
    actual = {path.name for path in directory.iterdir() if path.is_file()}
    if actual != expected:
        raise ValueError(
            f"release assets must be exactly {sorted(expected)}, found {sorted(actual)}"
        )
    empty = [name for name in expected if (directory / name).stat().st_size == 0]
    if empty:
        raise ValueError(f"release assets are empty: {sorted(empty)}")


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    version_parser = subparsers.add_parser("version")
    version_parser.add_argument("run_number", type=int)
    version_parser.add_argument("--manifest", type=Path, default=Path("Cargo.toml"))

    stamp_parser = subparsers.add_parser("stamp")
    stamp_parser.add_argument("version")
    stamp_parser.add_argument("--manifest", type=Path, default=Path("Cargo.toml"))
    stamp_parser.add_argument("--lockfile", type=Path, default=Path("Cargo.lock"))

    newer_parser = subparsers.add_parser("require-newer")
    newer_parser.add_argument("candidate")
    newer_parser.add_argument("latest")

    assets_parser = subparsers.add_parser("verify-assets")
    assets_parser.add_argument("directory", type=Path)
    assets_parser.add_argument("names", nargs="+")

    arguments = parser.parse_args()
    try:
        if arguments.command == "version":
            print(release_version(package_version(arguments.manifest), arguments.run_number))
        elif arguments.command == "stamp":
            stamp_version(arguments.manifest, arguments.lockfile, arguments.version)
        elif arguments.command == "require-newer":
            require_newer(arguments.candidate, arguments.latest)
        elif arguments.command == "verify-assets":
            verify_assets(arguments.directory, set(arguments.names))
    except (OSError, ValueError) as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
