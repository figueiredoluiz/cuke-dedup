#!/usr/bin/env python3
"""Verify npm native binaries match the checksummed release archives."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
from typing import Callable

from verify_release import archive_in, verify_and_extract


PROJECT_ROOT = Path(__file__).resolve().parents[2]
SAFE_TARGET = re.compile(r"^[A-Za-z0-9_-]+$")
SAFE_BINARY_NAMES = frozenset({"cuke-dedup", "cuke-dedup.exe"})
RELEASE_REPOSITORY = "figueiredoluiz/cuke-dedup"


def validated_targets(
    run: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
) -> list[tuple[str, str]]:
    """Load targets through the shared JavaScript allowlist validator."""
    command = ["node", str(PROJECT_ROOT / "scripts" / "release" / "release-matrix.mjs")]
    try:
        completed = run(
            command,
            cwd=PROJECT_ROOT,
            capture_output=True,
            text=True,
            check=False,
            timeout=10,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ValueError(f"could not validate release targets: {error}") from error
    if completed.returncode != 0:
        detail = completed.stderr.strip() or "target validator exited unsuccessfully"
        raise ValueError(f"could not validate release targets: {detail}")

    try:
        include = json.loads(completed.stdout)["include"]
    except (json.JSONDecodeError, KeyError, TypeError) as error:
        raise ValueError("target validator returned malformed JSON") from error
    if not isinstance(include, list) or not include:
        raise ValueError("target validator returned no targets")

    targets: list[tuple[str, str]] = []
    seen: set[str] = set()
    for entry in include:
        if not isinstance(entry, dict):
            raise ValueError("target validator returned a malformed target")
        rust_target = entry.get("target")
        binary_name = entry.get("binary")
        if (
            not isinstance(rust_target, str)
            or not SAFE_TARGET.fullmatch(rust_target)
            or not isinstance(binary_name, str)
            or binary_name not in SAFE_BINARY_NAMES
            or rust_target in seen
        ):
            raise ValueError("target validator returned an unsafe or duplicate target")
        seen.add(rust_target)
        targets.append((rust_target, binary_name))
    return targets


def verify_npm_binaries(binaries: Path, archives: Path) -> None:
    """Fail unless every npm binary matches the binary in its checksummed release archive."""
    for rust_target, binary_name in validated_targets():
        binary = binaries / f"binary-{rust_target}" / binary_name
        archive_directory = archives / f"release-{rust_target}"
        archive = archive_in(archive_directory)
        checksum = Path(f"{archive}.sha256")

        if not binary.is_file():
            raise ValueError(f"missing npm binary artifact: {binary}")
        with tempfile.TemporaryDirectory() as temporary:
            archived_binary = verify_and_extract(
                archive,
                checksum,
                Path(temporary),
            )
            raw_digest = hashlib.sha256(binary.read_bytes()).digest()
            archive_digest = hashlib.sha256(archived_binary.read_bytes()).digest()
            if raw_digest != archive_digest:
                raise ValueError(
                    f"npm binary differs from checksummed release archive for {rust_target}"
                )


def prepare_published_binaries(binaries: Path, tag: str, sha: str) -> None:
    """Recreate npm build inputs from the immutable release for an npm-only retry."""
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?", tag) or not re.fullmatch(
        r"[0-9a-f]{40}", sha
    ):
        raise ValueError("expected the release tag and its commit SHA")
    result = subprocess.run(
        ["gh", "release", "view", tag, "--repo", RELEASE_REPOSITORY, "--json", "isDraft,isImmutable"],
        check=True,
        capture_output=True,
        timeout=30,
    )
    if json.loads(result.stdout) != {"isDraft": False, "isImmutable": True}:
        raise ValueError("npm-only publishing requires an immutable published GitHub release")

    targets = validated_targets()
    archives = {
        target: f"cuke-dedup-{tag}-{target}{'.zip' if binary.endswith('.exe') else '.tar.gz'}"
        for target, binary in targets
    }
    with tempfile.TemporaryDirectory() as temporary:
        directory = Path(temporary)
        download = ["gh", "release", "download", tag, "--repo", RELEASE_REPOSITORY, "--dir", temporary]
        for archive in archives.values():
            download.extend(["--pattern", archive, "--pattern", f"{archive}.sha256"])
        subprocess.run(download, check=True, timeout=180)
        for target, binary in targets:
            archive = directory / archives[target]
            subprocess.run(
                [
                    "gh", "attestation", "verify", str(archive), "--repo", RELEASE_REPOSITORY,
                    "--signer-workflow", f"{RELEASE_REPOSITORY}/.github/workflows/release.yml",
                    "--source-ref", f"refs/tags/{tag}", "--source-digest", sha,
                    "--deny-self-hosted-runners",
                ],
                check=True,
                timeout=60,
            )
            extracted = verify_and_extract(archive, Path(f"{archive}.sha256"), binaries / f"binary-{target}")
            if extracted.name != binary:
                raise ValueError(f"unexpected release binary for {target}")


def main() -> None:
    """Verify the npm binary artifacts against the release archives named on the command line."""
    parser = argparse.ArgumentParser()
    parser.add_argument("--binaries", type=Path, required=True)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--archives", type=Path)
    source.add_argument("--published-release", nargs=2, metavar=("TAG", "SHA"))
    args = parser.parse_args()
    if args.published_release:
        prepare_published_binaries(args.binaries, *args.published_release)
        print("prepared npm binaries from the published release")
        return
    verify_npm_binaries(args.binaries, args.archives)
    print("verified npm binaries against release archives")


if __name__ == "__main__":
    main()
