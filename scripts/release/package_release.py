#!/usr/bin/env python3
"""Create reproducible release archives and SHA-256 checksum files."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
from pathlib import Path
import tarfile
import zipfile


PROJECT_ROOT = Path(__file__).resolve().parents[2]
LEGAL_FILES = [PROJECT_ROOT / "LICENSE", PROJECT_ROOT / "THIRD-PARTY-LICENSES.md"]


def archive_entries(binary: Path, binary_name: str) -> list[tuple[str, bytes, int]]:
    entries = [(binary_name, binary.read_bytes(), 0o755)]
    entries.extend((path.name, path.read_bytes(), 0o644) for path in LEGAL_FILES)
    return entries


def package_release(binary: Path, target: str, version: str, output: Path) -> tuple[Path, Path]:
    output.mkdir(parents=True, exist_ok=True)
    version = version.removeprefix("v")
    stem = f"cuke-dedup-v{version}-{target}"
    binary_name = "cuke-dedup.exe" if "windows" in target else "cuke-dedup"
    entries = archive_entries(binary, binary_name)

    if "windows" in target:
        archive = output / f"{stem}.zip"
        with zipfile.ZipFile(archive, "w") as package:
            for name, data, mode in entries:
                info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                info.create_system = 3
                info.external_attr = mode << 16
                info.compress_type = zipfile.ZIP_DEFLATED
                package.writestr(info, data, compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)
    else:
        archive = output / f"{stem}.tar.gz"
        tar_buffer = io.BytesIO()
        with tarfile.open(fileobj=tar_buffer, mode="w", format=tarfile.USTAR_FORMAT) as package:
            for name, data, mode in entries:
                info = tarfile.TarInfo(name)
                info.size = len(data)
                info.mode = mode
                info.mtime = 0
                info.uid = 0
                info.gid = 0
                info.uname = ""
                info.gname = ""
                package.addfile(info, io.BytesIO(data))
        with archive.open("wb") as destination:
            with gzip.GzipFile(fileobj=destination, mode="wb", filename="", mtime=0, compresslevel=9) as compressed:
                compressed.write(tar_buffer.getvalue())

    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    checksum = archive.with_suffix(archive.suffix + ".sha256")
    checksum.write_text(f"{digest}  {archive.name}\n", encoding="utf-8", newline="\n")
    return archive, checksum


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", type=Path, default=Path("dist"))
    args = parser.parse_args()
    archive, checksum = package_release(args.binary, args.target, args.version, args.output)
    print(archive)
    print(checksum)


if __name__ == "__main__":
    main()
