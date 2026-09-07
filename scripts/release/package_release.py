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


def package_release(binary: Path, target: str, version: str, output: Path) -> tuple[Path, Path]:
    data = binary.read_bytes()
    output.mkdir(parents=True, exist_ok=True)
    version = version.removeprefix("v")
    stem = f"cuke-dedup-v{version}-{target}"
    binary_name = "cuke-dedup.exe" if "windows" in target else "cuke-dedup"

    if "windows" in target:
        archive = output / f"{stem}.zip"
        info = zipfile.ZipInfo(binary_name, date_time=(1980, 1, 1, 0, 0, 0))
        info.create_system = 3
        info.external_attr = 0o755 << 16
        info.compress_type = zipfile.ZIP_DEFLATED
        with zipfile.ZipFile(archive, "w") as package:
            package.writestr(info, data, compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)
    else:
        archive = output / f"{stem}.tar.gz"
        tar_buffer = io.BytesIO()
        info = tarfile.TarInfo(binary_name)
        info.size = len(data)
        info.mode = 0o755
        info.mtime = 0
        info.uid = 0
        info.gid = 0
        info.uname = ""
        info.gname = ""
        with tarfile.open(fileobj=tar_buffer, mode="w", format=tarfile.USTAR_FORMAT) as package:
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
