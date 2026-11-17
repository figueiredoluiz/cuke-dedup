from __future__ import annotations

import hashlib
import json
from pathlib import Path
import os
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import Mock, patch
import zipfile

from package_release import package_release
from verify_release import archive_in, verify_and_extract
from verify_npm_binaries import (
    prepare_published_binaries,
    validated_targets,
    verify_npm_binaries,
)


class PackageReleaseTests(unittest.TestCase):
    def test_unix_archive_and_checksum_are_reproducible(self) -> None:
        """Packaging twice yields identical tar archives whose checksum record, mode and members match."""
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
                self.assertEqual(
                    archive.getnames(),
                    ["cuke-dedup", "LICENSE"],
                )

    def test_windows_archive_has_a_deterministic_executable(self) -> None:
        """Packaging twice yields identical ZIP archives holding `cuke-dedup.exe` and LICENSE."""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "binary.exe"
            binary.write_bytes(b"windows executable")
            first, _ = package_release(binary, "x86_64-pc-windows-msvc", "1.2.3", root / "first")
            second, _ = package_release(binary, "x86_64-pc-windows-msvc", "1.2.3", root / "second")

            self.assertEqual(first.read_bytes(), second.read_bytes())
            with zipfile.ZipFile(first) as archive:
                self.assertEqual(archive.read("cuke-dedup.exe"), b"windows executable")
                self.assertEqual(
                    archive.namelist(),
                    ["cuke-dedup.exe", "LICENSE"],
                )

    def test_verifier_checks_checksum_and_extracts_only_expected_binary(self) -> None:
        """A valid archive verifies and extracts its binary, and `archive_in` finds that archive."""
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
        """A checksum record whose digest does not match the archive is rejected."""
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

    def test_npm_binary_must_match_the_checksummed_release_archive(self) -> None:
        """Matching npm binaries verify; one that differs from its release archive is rejected."""
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binaries = root / "binaries"
            archives = root / "archives"
            manifest = json.loads(
                (Path(__file__).resolve().parents[2] / "npm" / "prebuilt-targets.json").read_text()
            )
            for target in manifest["targets"].values():
                rust_target = target["rustTarget"]
                binary_name = target["binaryName"]
                source = binaries / f"binary-{rust_target}" / binary_name
                source.parent.mkdir(parents=True)
                source.write_bytes(f"native executable for {rust_target}".encode())
                package_release(
                    source,
                    rust_target,
                    "1.2.3",
                    archives / f"release-{rust_target}",
                )

            verify_npm_binaries(binaries, archives)

            first = next(iter(manifest["targets"].values()))
            changed = binaries / f"binary-{first['rustTarget']}" / first["binaryName"]
            changed.write_bytes(b"different executable")
            with self.assertRaisesRegex(ValueError, "differs from checksummed release archive"):
                verify_npm_binaries(binaries, archives)

    def test_npm_binary_verifier_rejects_unvalidated_target_output(self) -> None:
        """A failed run, malformed JSON, or an unsafe or duplicate target from the validator raises."""
        cases = (
            Mock(returncode=1, stdout="", stderr="validation failed"),
            Mock(returncode=0, stdout="not json", stderr=""),
            Mock(
                returncode=0,
                stdout=json.dumps(
                    {"include": [{"target": "../outside", "binary": "cuke-dedup"}]}
                ),
                stderr="",
            ),
            Mock(
                returncode=0,
                stdout=json.dumps(
                    {
                        "include": [
                            {"target": "safe-target", "binary": "cuke-dedup"},
                            {"target": "safe-target", "binary": "cuke-dedup"},
                        ]
                    }
                ),
                stderr="",
            ),
        )
        for completed in cases:
            with self.subTest(stdout=completed.stdout, returncode=completed.returncode):
                with self.assertRaisesRegex(ValueError, "validate|validator"):
                    validated_targets(run=Mock(return_value=completed))

    def test_npm_recovery_verifies_and_extracts_the_published_archive(self) -> None:
        """Recover the exact archive bytes and stop before extraction when verification fails."""
        tag = "v1.2.3"
        sha = "a" * 40
        target = "x86_64-pc-windows-msvc"
        name = "cuke-dedup.exe"
        payload = b"published Windows binary"
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / name
            source.write_bytes(payload)
            archive, checksum = package_release(source, target, tag[1:], root / "published")
            for case, error in (
                ("success", None),
                ("draft", "immutable published"),
                ("mutable", "immutable published"),
                ("unattested", "non-zero exit status"),
                ("corrupt", "checksum mismatch"),
            ):
                with self.subTest(case=case):
                    output = root / case

                    def github(command, *, check, timeout, capture_output=False):
                        """Serve owned release assets and enforce the exact attestation arguments."""
                        self.assertEqual(command[0], "gh")
                        self.assertTrue(check)
                        self.assertGreater(timeout, 0)
                        if command[1:3] == ["release", "view"]:
                            self.assertTrue(capture_output)
                            self.assertEqual(command[3:], [
                                tag, "--repo", "figueiredoluiz/cuke-dedup",
                                "--json", "isDraft,isImmutable",
                            ])
                            metadata = {"isDraft": case == "draft", "isImmutable": case != "mutable"}
                            return subprocess.CompletedProcess(command, 0, json.dumps(metadata))
                        if command[1:3] == ["release", "download"]:
                            self.assertEqual(command[3], tag)
                            directory = Path(command[command.index("--dir") + 1])
                            self.assertEqual(command[command.index("--pattern"):], [
                                "--pattern", archive.name, "--pattern", checksum.name,
                            ])
                            for asset in [archive, checksum]:
                                shutil.copyfile(asset, directory / asset.name)
                            if case == "corrupt":
                                (directory / archive.name).write_bytes(b"different bytes")
                            return subprocess.CompletedProcess(command, 0)
                        self.assertEqual(command[1:3], ["attestation", "verify"])
                        self.assertEqual(Path(command[3]).name, archive.name)
                        self.assertEqual(command[4:], [
                            "--repo", "figueiredoluiz/cuke-dedup", "--signer-workflow",
                            "figueiredoluiz/cuke-dedup/.github/workflows/release.yml",
                            "--source-ref", f"refs/tags/{tag}", "--source-digest", sha,
                            "--deny-self-hosted-runners",
                        ])
                        if case == "unattested":
                            raise subprocess.CalledProcessError(1, command)
                        return subprocess.CompletedProcess(command, 0)

                    with (
                        patch("verify_npm_binaries.validated_targets", return_value=[(target, name)]),
                        patch("verify_npm_binaries.subprocess.run", side_effect=github) as github_calls,
                    ):
                        if error:
                            with self.assertRaisesRegex((ValueError, subprocess.CalledProcessError), error):
                                prepare_published_binaries(output, tag, sha)
                        else:
                            prepare_published_binaries(output, tag, sha)
                    binary = output / f"binary-{target}" / name
                    if error:
                        self.assertFalse(binary.exists())
                    else:
                        self.assertEqual(binary.read_bytes(), payload)
                    self.assertEqual(github_calls.call_count, 1 if case in {"draft", "mutable"} else 3)
            with patch("verify_npm_binaries.subprocess.run") as github_calls:
                for release_tag, commit in [("main", sha), (tag, "abcd")]:
                    with self.assertRaisesRegex(ValueError, "release tag and its commit SHA"):
                        prepare_published_binaries(root / "invalid", release_tag, commit)
            github_calls.assert_not_called()

    @unittest.skipIf(os.name == "nt", "The npm publication job packs on a POSIX runner")
    def test_npm_packaging_normalizes_native_modes(self) -> None:
        """Pack both source modes on the publish runner and compare binary modes and tarballs."""
        project = Path(__file__).resolve().parents[2]
        targets = json.loads((project / "npm/prebuilt-targets.json").read_text())["targets"].values()
        representatives = {"x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"}
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            packages = []
            for input_mode in [0o644, 0o755]:
                fixture = root / str(input_mode)
                fixture.mkdir()
                shutil.copyfile(project / "LICENSE", fixture / "LICENSE")
                for target in targets:
                    manifest = Path("npm/platforms", target["packageDirectory"], "package.json")
                    (fixture / manifest).parent.mkdir(parents=True)
                    shutil.copyfile(project / manifest, fixture / manifest)
                    binary = fixture / "native" / f"binary-{target['rustTarget']}" / target["binaryName"]
                    binary.parent.mkdir(parents=True)
                    binary.write_bytes(target["rustTarget"].encode())
                    binary.chmod(input_mode)
                subprocess.run(
                    ["node", str(project / "scripts/release/prepare-npm-packages.mjs"), "native"],
                    cwd=fixture, check=True, timeout=20,
                )
                tarballs = {}
                for target in targets:
                    if target["rustTarget"] not in representatives:
                        continue
                    package = fixture / "npm/platforms" / target["packageDirectory"]
                    packed = subprocess.run(
                        ["npm", "pack", "--offline", "--ignore-scripts", "--json"],
                        cwd=package, capture_output=True, check=True, timeout=20,
                    )
                    archive = package / json.loads(packed.stdout)[0]["filename"]
                    member = "package/bin/" + target["binaryName"]
                    expected_mode = 0o644 if member.endswith(".exe") else 0o755
                    with tarfile.open(archive) as tarball:
                        self.assertEqual(tarball.getmember(member).mode, expected_mode)
                    tarballs[target["rustTarget"]] = hashlib.sha256(archive.read_bytes()).digest()
                self.assertEqual(tarballs.keys(), representatives)
                packages.append(tarballs)
            self.assertEqual(packages[0], packages[1])


if __name__ == "__main__":
    unittest.main()
