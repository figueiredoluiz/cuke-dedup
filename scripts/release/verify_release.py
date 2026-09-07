#!/usr/bin/env python3
"""Verify and smoke-test one packaged CukeDedup release archive."""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path
import subprocess
import tarfile
import tempfile
import zipfile


LEGAL_FILES = ["LICENSE", "THIRD-PARTY-LICENSES.md"]


def verify_and_extract(archive: Path, checksum: Path, destination: Path) -> Path:
    fields = checksum.read_text(encoding="utf-8").strip().split()
    if len(fields) != 2 or fields[1] != archive.name:
        raise ValueError(f"invalid checksum record in {checksum}")
    expected = fields[0].lower()
    if len(expected) != 64 or any(character not in "0123456789abcdef" for character in expected):
        raise ValueError(f"invalid SHA-256 digest in {checksum}")
    actual = hashlib.sha256(archive.read_bytes()).hexdigest()
    if actual != expected:
        raise ValueError(f"checksum mismatch for {archive}")

    binary_name = "cuke-dedup.exe" if archive.suffix == ".zip" else "cuke-dedup"
    destination.mkdir(parents=True, exist_ok=True)
    binary = destination / binary_name
    expected_names = [binary_name, *LEGAL_FILES]

    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as package:
            names = package.namelist()
            if names != expected_names:
                raise ValueError(f"unexpected ZIP members: {names}")
            binary.write_bytes(package.read(binary_name))
    else:
        with tarfile.open(archive, "r:gz") as package:
            members = package.getmembers()
            if [member.name for member in members] != expected_names or not all(
                member.isfile() for member in members
            ):
                raise ValueError(
                    f"unexpected tar members: {[member.name for member in members]}"
                )
            source = package.extractfile(members[0])
            if source is None:
                raise ValueError(f"archive member {binary_name} has no content")
            binary.write_bytes(source.read())

    binary.chmod(0o755)
    return binary


def archive_in(directory: Path) -> Path:
    archives = sorted([*directory.glob("*.tar.gz"), *directory.glob("*.zip")])
    if len(archives) != 1:
        raise ValueError(f"expected one release archive in {directory}, found {len(archives)}")
    return archives[0]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--directory", type=Path, required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()

    archive = archive_in(args.directory)
    with tempfile.TemporaryDirectory() as temporary:
        binary = verify_and_extract(
            archive,
            Path(f"{archive}.sha256"),
            Path(temporary),
        )
        completed = subprocess.run(
            [binary, "--version"],
            check=True,
            capture_output=True,
            text=True,
        )
    expected = args.version.removeprefix("v")
    if completed.stdout.strip() != f"cuke-dedup {expected}":
        raise ValueError(
            f"packaged binary reported {completed.stdout.strip()!r}, expected cuke-dedup {expected}"
        )
    print(f"verified {archive.name}")


if __name__ == "__main__":
    main()
