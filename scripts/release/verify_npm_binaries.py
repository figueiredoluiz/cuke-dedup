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


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binaries", type=Path, required=True)
    parser.add_argument("--archives", type=Path, required=True)
    args = parser.parse_args()
    verify_npm_binaries(args.binaries, args.archives)
    print("verified npm binaries against release archives")


if __name__ == "__main__":
    main()
