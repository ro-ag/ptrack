#!/usr/bin/env python3
"""Collect native Linux bundles and reject host-specific executable linkage."""
import argparse
import re
import shutil
import subprocess
from pathlib import Path

from release_contract import require_version, validate_binary, validate_linux_package


def collect(release: Path, version: str, arch: str, output: Path) -> None:
    require_version(version)
    binary = release / "ptrack"
    validate_binary(binary, version, "linux", arch)
    linkage = subprocess.check_output(["readelf", "-l", "-d", "-V", str(binary)], text=True)
    if "/nix/store/" in linkage:
        raise ValueError("Linux release must not link to the build host's Nix store")
    versions = [tuple(map(int, v.split("."))) for v in re.findall(r"GLIBC_(\d+\.\d+)", linkage)]
    if not versions or max(versions) > (2, 35):
        raise ValueError("Linux release requires newer than glibc 2.35")
    output.mkdir(parents=True, exist_ok=True)
    native_arch = {"amd64": "x86_64", "arm64": "aarch64"}[arch]
    appimage_arch = {"amd64": "amd64", "arm64": "aarch64"}[arch]
    sources = {
        "AppImage": f"appimage/p-track_{version}_{appimage_arch}.AppImage",
        "deb": f"deb/p-track_{version}_{arch}.deb",
        "rpm": f"rpm/p-track-{version}-1.{native_arch}.rpm",
    }
    for extension in ("AppImage", "deb", "rpm"):
        source = release / "bundle" / sources[extension]
        if not source.is_file():
            raise ValueError(f"expected versioned bundle: {source}")
        destination = output / f"p-track_{version}_linux_{arch}.{extension}"
        shutil.copy2(source, destination)
        validate_linux_package(destination)
        if extension == "deb":
            metadata = subprocess.check_output(
                ["dpkg-deb", "--show", "--showformat=${Package} ${Version} ${Architecture}", str(destination)],
                text=True,
            )
            expected = f"p-track {version} {arch}"
        elif extension == "rpm":
            metadata = subprocess.check_output(
                ["rpm", "-qp", "--qf", "%{NAME} %{VERSION} %{ARCH}", str(destination)], text=True,
            )
            expected = f"p-track {version} " + {"amd64": "x86_64", "arm64": "aarch64"}[arch]
        else:
            metadata = expected = ""
        if metadata != expected:
            raise ValueError(f"wrong package identity: {metadata!r}, expected {expected!r}")
        print(destination)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["collect"])
    parser.add_argument("release", type=Path)
    parser.add_argument("version")
    parser.add_argument("arch", choices=["amd64", "arm64"])
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    collect(args.release, args.version, args.arch, args.output)
