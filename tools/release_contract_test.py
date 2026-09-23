"""Producer-side fixtures for the exact release/update compatibility surface."""

from __future__ import annotations

import io
from pathlib import Path
import re
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
    def test_exact_five_target_package_set_layout_machines_and_checksums(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            dist = Path(temporary)
            for arch in release_contract.DARWIN_ARCHES:
                (dist / f"p-track_1.2.3_darwin_{arch}.dmg").write_bytes(b"dmg")
                write_tar(dist / f"ptrack_1.2.3_darwin_{arch}.tar.gz", "darwin", arch)
            for arch in release_contract.ARCHES:
                write_tar(dist / f"ptrack_1.2.3_linux_{arch}.tar.gz", "linux", arch)
                write_zip(dist / f"ptrack_1.2.3_windows_{arch}.zip", arch)
            release_contract.validate_dist(dist, "1.2.3")
            checksum_path = release_contract.write_checksums(dist, "1.2.3")
            lines = checksum_path.read_text(encoding="ascii").splitlines()
            self.assertEqual(len(lines), 6)
            self.assertEqual(
                [line.split("  ", 1)[1] for line in lines],
                list(release_contract.package_names("1.2.3")),
            )
            release_contract.validate_dist(dist, "1.2.3")
            (dist / "checksums.txt.sig").write_bytes(b"s" * 64)
            release_contract.validate_dist(dist, "1.2.3")

    def test_signature_is_accepted_only_beside_its_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            dist = Path(temporary)
            for name in release_contract.package_names("1.2.3"):
                if name.endswith(".dmg"):
                    (dist / name).write_bytes(b"dmg")
                elif name.endswith(".zip"):
                    write_zip(dist / name, name.rsplit("_", 1)[1].removesuffix(".zip"))
                else:
                    os_name, arch = name.removesuffix(".tar.gz").split("_")[2:]
                    write_tar(dist / name, os_name, arch)
            (dist / "checksums.txt.sig").write_bytes(b"s" * 64)
            with self.assertRaisesRegex(release_contract.ContractError, "release assets differ"):
                release_contract.validate_dist(dist, "1.2.3")

    def test_pinned_public_key_matches_the_updater_and_encodes_as_ed25519_spki(self) -> None:
        source = (
            Path(__file__).resolve().parent.parent / "crates/ptrack-updater/src/signature.rs"
        ).read_text(encoding="utf-8")
        body = source.split("RELEASE_SIGNING_PUBLIC_KEY: [u8; 32] = [", 1)[1].split("];", 1)[0]
        updater_key = bytes(int(value, 16) for value in re.findall(r"0x([0-9a-f]{2})", body))
        self.assertEqual(updater_key.hex(), release_contract.RELEASE_SIGNING_PUBLIC_KEY_HEX)
        der = release_contract.release_public_key_der()
        self.assertEqual(len(der), 44)
        self.assertEqual(der[-32:], updater_key)
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary) / "public.der"
            self.assertEqual(release_contract.main(["public-key", str(destination)]), 0)
            self.assertEqual(destination.read_bytes(), der)

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
                "## [1x2x3] - 2026-08-13\nwrong\n"
                "## [1.2.3] - 2026-08-13\n\nright\n## [1.2.2] - 2026-08-12\nold\n",
                encoding="utf-8",
            )
            release_contract.extract_release_notes(changelog, "1.2.3", output)
            self.assertEqual(output.read_text(encoding="utf-8"), "\nright\n")
            with self.assertRaisesRegex(release_contract.ContractError, "canonical stable"):
                release_contract.extract_release_notes(changelog, "01.2.3", output)


class WorkflowTests(unittest.TestCase):
    def test_release_workflow_is_tag_only_native_rust_and_exactly_five_targets(self) -> None:
        workflow = (Path(__file__).resolve().parent.parent / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )
        for target in (
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "aarch64-apple-darwin",
            "x86_64-pc-windows-msvc",
            "aarch64-pc-windows-msvc",
        ):
            self.assertEqual(workflow.count(f"rust_target: {target}"), 1)
        self.assertNotIn("x86_64-apple-darwin", workflow)
        self.assertIn('tags:\n      - "v*"', workflow)
        self.assertEqual(workflow.count("npm --prefix frontend run tauri -- build"), 3)
        self.assertEqual(workflow.count("--no-bundle"), 2)
        self.assertEqual(workflow.count("--bundles app"), 1)
        self.assertEqual(workflow.count("-- --locked"), 3)
        self.assertNotIn("cargo build --locked --release", workflow)
        self.assertIn("tools/release_contract.py validate-dist", workflow)
        self.assertNotIn("actions/setup-go", workflow)
        self.assertNotIn("ptrack-db-export", workflow)
        self.assertNotIn("\n          go test", workflow)
        self.assertNotIn("go vet", workflow)
        self.assertNotIn("cmd/wails", workflow.lower())
        self.assertNotIn("wails build", workflow.lower())

    def test_release_workflow_signs_and_self_verifies_checksums_before_publishing(self) -> None:
        workflow = (Path(__file__).resolve().parent.parent / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )
        release_job = workflow.split("\n  release:\n", 1)[1]
        self.assertIn("PTRACK_RELEASE_SIGNING_KEY: ${{ secrets.PTRACK_RELEASE_SIGNING_KEY }}", release_job)
        self.assertIn('if [ -z "${PTRACK_RELEASE_SIGNING_KEY}" ]; then', release_job)
        self.assertIn("::error::The PTRACK_RELEASE_SIGNING_KEY secret is not set", release_job)
        self.assertIn('chmod 600 "$key"', release_job)
        self.assertIn('rm -f "$key"', release_job)
        self.assertIn("openssl pkeyutl -sign -inkey \"$key\" -rawin", release_job)
        self.assertIn("-out dist/checksums.txt.sig", release_job)
        self.assertIn("tools/release_contract.py public-key", release_job)
        self.assertIn("openssl pkeyutl -verify -pubin", release_job)
        self.assertIn("-sigfile dist/checksums.txt.sig", release_job)
        sign = release_job.index("openssl pkeyutl -sign")
        verify = release_job.index("openssl pkeyutl -verify")
        publish = release_job.index("gh release create")
        self.assertLess(release_job.index("tools/release_contract.py checksums"), sign)
        self.assertLess(sign, verify)
        self.assertLess(verify, publish)
        self.assertNotIn("secrets.PTRACK_RELEASE_SIGNING_KEY", workflow.split("\n  release:\n", 1)[0])

    def test_native_acceptance_is_nonpublishing_and_exactly_five_native_hosts(self) -> None:
        workflow = (
            Path(__file__).resolve().parent.parent
            / ".github/workflows/native-acceptance.yml"
        ).read_text(encoding="utf-8")
        for target in (
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "aarch64-apple-darwin",
            "x86_64-pc-windows-msvc",
            "aarch64-pc-windows-msvc",
        ):
            self.assertEqual(workflow.count(f"rust_target: {target}"), 1)
        self.assertNotIn("x86_64-apple-darwin", workflow)
        self.assertIn("pull_request:", workflow)
        self.assertIn("branches:\n      - main", workflow)
        self.assertIn("needs: portable", workflow)
        self.assertIn("native-acceptance-approved", workflow)
        self.assertIn('paths:\n      - ".github/workflows/native-acceptance.yml"', workflow)
        self.assertIn("cargo test --workspace --all-targets --no-fail-fast", workflow)
        self.assertNotIn("actions/setup-go", workflow)
        self.assertNotIn("ptrack-db-export", workflow)
        self.assertNotIn("\n          go test", workflow)
        self.assertNotIn("go vet", workflow)
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
