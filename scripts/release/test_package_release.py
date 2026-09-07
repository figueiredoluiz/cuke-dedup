from __future__ import annotations

import hashlib
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile

from package_release import package_release
from verify_release import archive_in, verify_and_extract


class PackageReleaseTests(unittest.TestCase):
    def test_unix_archive_and_checksum_are_reproducible(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "binary"
            binary.write_bytes(b"native executable")
            first, checksum = package_release(binary, "aarch64-apple-darwin", "1.2.3", root / "first")
            second, _ = package_release(binary, "aarch64-apple-darwin", "1.2.3", root / "second")

            self.assertEqual(first.read_bytes(), second.read_bytes())
            digest = hashlib.sha256(first.read_bytes()).hexdigest()
            self.assertEqual(checksum.read_text(), f"{digest}  {first.name}\n")
            with tarfile.open(first, "r:gz") as archive:
                member = archive.getmember("cuke-dedup")
                self.assertEqual(member.mode, 0o755)
                self.assertEqual(archive.extractfile(member).read(), b"native executable")

    def test_windows_archive_has_a_deterministic_executable(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "binary.exe"
            binary.write_bytes(b"windows executable")
            first, _ = package_release(binary, "x86_64-pc-windows-msvc", "1.2.3", root / "first")
            second, _ = package_release(binary, "x86_64-pc-windows-msvc", "1.2.3", root / "second")

            self.assertEqual(first.read_bytes(), second.read_bytes())
            with zipfile.ZipFile(first) as archive:
                self.assertEqual(archive.read("cuke-dedup.exe"), b"windows executable")

    def test_verifier_checks_checksum_and_extracts_only_expected_binary(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "binary"
            source.write_bytes(b"native executable")
            archive, checksum = package_release(
                source, "x86_64-unknown-linux-gnu", "1.2.3", root / "dist"
            )

            extracted = verify_and_extract(archive, checksum, root / "extracted")

            self.assertEqual(extracted.read_bytes(), b"native executable")
            self.assertEqual(archive_in(root / "dist"), archive)

    def test_verifier_rejects_a_checksum_for_another_archive(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "binary"
            source.write_bytes(b"native executable")
            archive, checksum = package_release(
                source, "x86_64-unknown-linux-gnu", "1.2.3", root / "dist"
            )
            checksum.write_text(f"{'0' * 64}  {archive.name}\n", encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                verify_and_extract(archive, checksum, root / "extracted")


if __name__ == "__main__":
    unittest.main()
