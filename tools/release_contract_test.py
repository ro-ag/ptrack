"""Producer-side fixtures for the exact release/update compatibility surface."""

from __future__ import annotations

import io
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile

from tools import release_contract


def executable_header(os_name: str, arch: str) -> bytes:
    if os_name == "linux":
        data = bytearray(512)
        data[:4] = b"\x7fELF"
        data[18:20] = (62 if arch == "amd64" else 183).to_bytes(2, "little")
        return bytes(data)
    if os_name == "windows":
        data = bytearray(512)
        data[:2] = b"MZ"
        data[60:64] = (64).to_bytes(4, "little")
        data[64:68] = b"PE\0\0"
        data[68:70] = (0x8664 if arch == "amd64" else 0xAA64).to_bytes(2, "little")
        return bytes(data)
    data = bytearray(512)
    data[:4] = b"\xcf\xfa\xed\xfe"
    data[4:8] = (0x01000007 if arch == "amd64" else 0x0100000C).to_bytes(4, "little")
    return bytes(data)


def write_tar(path: Path, os_name: str, arch: str, *, extra: bool = False) -> None:
    executable = executable_header(os_name, arch)
    with tarfile.open(path, "w:gz") as archive:
        for name, data in (("ptrack", executable), ("README.md", b"readme"), ("LICENSE", b"license")):
            entry = tarfile.TarInfo(name)
            entry.size = len(data)
            entry.mode = 0o755 if name == "ptrack" else 0o644
            archive.addfile(entry, io.BytesIO(data))
        if extra:
            entry = tarfile.TarInfo("extra")
            entry.size = 1
            archive.addfile(entry, io.BytesIO(b"x"))


def write_zip(path: Path, arch: str) -> None:
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("ptrack.exe", executable_header("windows", arch))
        archive.writestr("README.md", b"readme")
        archive.writestr("LICENSE", b"license")


class ReleaseArtifactTests(unittest.TestCase):
    def test_exact_six_target_package_set_layout_machines_and_checksums(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            dist = Path(temporary)
            for arch in release_contract.ARCHES:
                (dist / f"p-track_1.2.3_darwin_{arch}.dmg").write_bytes(b"dmg")
                write_tar(dist / f"ptrack_1.2.3_darwin_{arch}.tar.gz", "darwin", arch)
                write_tar(dist / f"ptrack_1.2.3_linux_{arch}.tar.gz", "linux", arch)
                write_zip(dist / f"ptrack_1.2.3_windows_{arch}.zip", arch)
            release_contract.validate_dist(dist, "1.2.3")
            checksum_path = release_contract.write_checksums(dist, "1.2.3")
            lines = checksum_path.read_text(encoding="ascii").splitlines()
            self.assertEqual(len(lines), 8)
            self.assertEqual(
                [line.split("  ", 1)[1] for line in lines],
                list(release_contract.package_names("1.2.3")),
            )
            release_contract.validate_dist(dist, "1.2.3")

    def test_archive_extra_entry_and_release_asset_extra_file_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "ptrack_1.2.3_linux_amd64.tar.gz"
            write_tar(archive, "linux", "amd64", extra=True)
            with self.assertRaisesRegex(release_contract.ContractError, "entries differ"):
                release_contract.validate_archive(archive)
            (root / "unexpected.txt").write_text("no", encoding="utf-8")
            with self.assertRaisesRegex(release_contract.ContractError, "release assets differ"):
                release_contract.validate_dist(root, "1.2.3")

    def test_release_note_heading_is_literal_bounded_and_nonempty(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            changelog = root / "CHANGELOG.md"
            output = root / "notes.md"
            changelog.write_text(
                "## [1x2x3]\nwrong\n## [1.2.3]\n\nright\n## [1.2.2]\nold\n",
                encoding="utf-8",
            )
            release_contract.extract_release_notes(changelog, "1.2.3", output)
            self.assertEqual(output.read_text(encoding="utf-8"), "\nright\n")
            with self.assertRaisesRegex(release_contract.ContractError, "canonical stable"):
                release_contract.extract_release_notes(changelog, "01.2.3", output)


class WorkflowTests(unittest.TestCase):
    def test_release_workflow_is_tag_only_native_rust_and_exactly_six_targets(self) -> None:
        workflow = (Path(__file__).resolve().parent.parent / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )
        for target in (
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "x86_64-apple-darwin",
            "aarch64-apple-darwin",
            "x86_64-pc-windows-msvc",
            "aarch64-pc-windows-msvc",
        ):
            self.assertEqual(workflow.count(f"rust_target: {target}"), 1)
        self.assertIn('tags:\n      - "v*"', workflow)
        self.assertIn("cargo build --locked --release", workflow)
        self.assertIn("run tauri -- build", workflow)
        self.assertIn("tools/release_contract.py validate-dist", workflow)
        self.assertNotIn("cmd/wails", workflow.lower())
        self.assertNotIn("wails build", workflow.lower())

    def test_native_acceptance_is_nonpublishing_and_exactly_six_native_hosts(self) -> None:
        workflow = (
            Path(__file__).resolve().parent.parent
            / ".github/workflows/native-acceptance.yml"
        ).read_text(encoding="utf-8")
        for target in (
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "x86_64-apple-darwin",
            "aarch64-apple-darwin",
            "x86_64-pc-windows-msvc",
            "aarch64-pc-windows-msvc",
        ):
            self.assertEqual(workflow.count(f"rust_target: {target}"), 1)
        self.assertIn("pull_request:", workflow)
        self.assertIn("branches:\n      - main", workflow)
        self.assertIn("needs: portable", workflow)
        self.assertIn("native-acceptance-approved", workflow)
        self.assertIn('paths:\n      - ".github/workflows/native-acceptance.yml"', workflow)
        self.assertIn("cargo test --workspace --all-targets --no-fail-fast", workflow)
        self.assertEqual(
            workflow.count('chmod 700 "$RUNNER_TEMP/ptrack-home"'), 2
        )
        self.assertIn('"ptrack-home-$([guid]::NewGuid())"', workflow)
        self.assertIn("native desktop smoke home must start absent", workflow)
        self.assertNotIn("icacls $env:PTRACK_HOME", workflow)
        self.assertIn("permissions:\n  contents: read", workflow)
        self.assertNotIn("gh release", workflow)
        self.assertNotIn("actions/upload-artifact", workflow)
        self.assertNotIn("secrets.", workflow)


if __name__ == "__main__":
    unittest.main()
