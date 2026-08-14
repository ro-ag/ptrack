#!/usr/bin/env python3
"""Validate and publish the frozen p-track release artifact contract."""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tarfile
import zipfile


MAX_PACKAGE_BYTES = 512 << 20
MAX_ENTRY_BYTES = 128 << 20
MAX_EXPANDED_BYTES = 160 << 20
MAX_NOTES_BYTES = 32 << 10
VERSION = re.compile(r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$")
ARCHES = ("amd64", "arm64")


class ContractError(ValueError):
    pass


def require_version(version: str) -> str:
    if not VERSION.fullmatch(version):
        raise ContractError("release version must be canonical stable SemVer")
    return version


def package_names(version: str) -> tuple[str, ...]:
    require_version(version)
    names: list[str] = []
    for arch in ARCHES:
        names.extend(
            (
                f"p-track_{version}_darwin_{arch}.dmg",
                f"ptrack_{version}_darwin_{arch}.tar.gz",
                f"ptrack_{version}_linux_{arch}.tar.gz",
                f"ptrack_{version}_windows_{arch}.zip",
            )
        )
    return tuple(sorted(names))


def _safe_archive_name(raw: str) -> str:
    name = raw.removeprefix("./")
    path = PurePosixPath(name)
    if (
        not name
        or name != path.as_posix()
        or path.is_absolute()
        or "\\" in name
        or any(part in {"", ".", ".."} for part in path.parts)
        or len(path.parts) != 1
    ):
        raise ContractError(f"unsafe archive entry: {raw}")
    return name


def _machine(data: bytes, os_name: str) -> str:
    if os_name == "linux" and len(data) >= 20 and data[:4] == b"\x7fELF":
        machine = int.from_bytes(data[18:20], "little")
        return {62: "amd64", 183: "arm64"}.get(machine, "")
    if os_name == "windows" and len(data) >= 64 and data[:2] == b"MZ":
        offset = int.from_bytes(data[60:64], "little")
        if offset >= 64 and offset + 6 <= len(data) and data[offset : offset + 4] == b"PE\0\0":
            machine = int.from_bytes(data[offset + 4 : offset + 6], "little")
            return {0x8664: "amd64", 0xAA64: "arm64"}.get(machine, "")
    if os_name == "darwin" and len(data) >= 8:
        magic = data[:4]
        if magic == b"\xcf\xfa\xed\xfe":
            cpu = int.from_bytes(data[4:8], "little")
            return {0x01000007: "amd64", 0x0100000C: "arm64"}.get(cpu, "")
        if magic == b"\xfe\xed\xfa\xcf":
            cpu = int.from_bytes(data[4:8], "big")
            return {0x01000007: "amd64", 0x0100000C: "arm64"}.get(cpu, "")
    return ""


def _expected_archive(filename: str) -> tuple[str, str, set[str]]:
    match = re.fullmatch(
        r"ptrack_(?P<version>[^_]+)_(?P<os>darwin|linux|windows)_(?P<arch>amd64|arm64)\.(?:tar\.gz|zip)",
        filename,
    )
    if match is None:
        raise ContractError(f"unexpected archive name: {filename}")
    os_name = match.group("os")
    executable = "ptrack.exe" if os_name == "windows" else "ptrack"
    return os_name, match.group("arch"), {executable, "README.md", "LICENSE"}


def validate_archive(path: Path) -> None:
    os_name, arch, expected = _expected_archive(path.name)
    seen: set[str] = set()
    expanded = 0
    executable = b""
    if path.name.endswith(".tar.gz"):
        with tarfile.open(path, "r:gz") as archive:
            for entry in archive:
                if entry.name in {".", "./"} and entry.isdir():
                    continue
                name = _safe_archive_name(entry.name)
                if not entry.isfile() or name in seen:
                    raise ContractError(f"non-regular or duplicate archive entry: {entry.name}")
                if entry.size <= 0 or entry.size > MAX_ENTRY_BYTES:
                    raise ContractError(f"invalid archive entry size: {entry.name}")
                expanded += entry.size
                if expanded > MAX_EXPANDED_BYTES:
                    raise ContractError("archive expands beyond the release limit")
                seen.add(name)
                if name == "ptrack":
                    source = archive.extractfile(entry)
                    executable = b"" if source is None else source.read(4_096)
    else:
        with zipfile.ZipFile(path) as archive:
            for entry in archive.infolist():
                name = _safe_archive_name(entry.filename)
                unix_type = (entry.external_attr >> 16) & 0o170000
                if entry.is_dir() or entry.flag_bits & 1 or unix_type == 0o120000 or name in seen:
                    raise ContractError(f"unsafe or duplicate ZIP entry: {entry.filename}")
                if entry.file_size <= 0 or entry.file_size > MAX_ENTRY_BYTES:
                    raise ContractError(f"invalid ZIP entry size: {entry.filename}")
                expanded += entry.file_size
                if expanded > MAX_EXPANDED_BYTES:
                    raise ContractError("ZIP expands beyond the release limit")
                seen.add(name)
                if name == "ptrack.exe":
                    with archive.open(entry) as source:
                        executable = source.read(4_096)
    if seen != expected:
        raise ContractError(f"archive entries differ: expected {sorted(expected)}, got {sorted(seen)}")
    if _machine(executable, os_name) != arch:
        raise ContractError(f"archive executable machine does not match {os_name}/{arch}")


def validate_dist(directory: Path, version: str) -> tuple[Path, ...]:
    expected = package_names(version)
    actual = tuple(sorted(path.name for path in directory.iterdir() if path.is_file()))
    allowed = expected if "checksums.txt" not in actual else tuple(sorted((*expected, "checksums.txt")))
    if actual != allowed:
        raise ContractError(f"release assets differ: expected {list(allowed)}, got {list(actual)}")
    packages = tuple(directory / name for name in expected)
    for path in packages:
        if path.is_symlink():
            raise ContractError(f"release package must not be a symbolic link: {path.name}")
        size = path.stat().st_size
        if size <= 0 or size > MAX_PACKAGE_BYTES:
            raise ContractError(f"release package size is invalid: {path.name}")
        if path.name.endswith((".tar.gz", ".zip")):
            validate_archive(path)
    return packages


def write_checksums(directory: Path, version: str) -> Path:
    packages = validate_dist(directory, version)
    lines = []
    for path in packages:
        digest = hashlib.sha256()
        with path.open("rb") as source:
            while chunk := source.read(1 << 20):
                digest.update(chunk)
        lines.append(f"{digest.hexdigest()}  {path.name}\n")
    destination = directory / "checksums.txt"
    destination.write_text("".join(lines), encoding="ascii", newline="\n")
    return destination


def extract_release_notes(changelog: Path, version: str, destination: Path) -> None:
    require_version(version)
    lines = changelog.read_text(encoding="utf-8").splitlines(keepends=True)
    heading = re.compile(rf"^## \[{re.escape(version)}\](?: - \d{{4}}-\d{{2}}-\d{{2}})?$")
    selected: list[str] = []
    found = False
    for line in lines:
        if heading.fullmatch(line.rstrip("\r\n")):
            found = True
            continue
        if found and line.startswith("## ["):
            break
        if found:
            selected.append(line)
    notes = "".join(selected)
    encoded = notes.encode("utf-8")
    if not found or not notes.strip() or len(encoded) > MAX_NOTES_BYTES:
        raise ContractError("release notes must be non-empty and at most 32 KiB")
    destination.write_bytes(encoded)


def validate_binary(path: Path, version: str, os_name: str, arch: str) -> None:
    with path.open("rb") as source:
        header = source.read(4_096)
    if _machine(header, os_name) != arch:
        raise ContractError(f"binary machine does not match {os_name}/{arch}")
    completed = subprocess.run(
        [path, "version"], check=False, capture_output=True, timeout=15
    )
    expected = f"ptrack {require_version(version)}\n".encode()
    if completed.returncode != 0 or completed.stdout != expected or completed.stderr:
        raise ContractError("binary does not report the exact release version")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    validate = commands.add_parser("validate-dist")
    validate.add_argument("directory", type=Path)
    validate.add_argument("version")
    checksums = commands.add_parser("checksums")
    checksums.add_argument("directory", type=Path)
    checksums.add_argument("version")
    notes = commands.add_parser("release-notes")
    notes.add_argument("changelog", type=Path)
    notes.add_argument("version")
    notes.add_argument("destination", type=Path)
    binary = commands.add_parser("validate-binary")
    binary.add_argument("path", type=Path)
    binary.add_argument("version")
    binary.add_argument("os", choices=("darwin", "linux", "windows"))
    binary.add_argument("arch", choices=ARCHES)
    args = parser.parse_args(argv)
    try:
        if args.command == "validate-dist":
            validate_dist(args.directory, args.version)
        elif args.command == "checksums":
            write_checksums(args.directory, args.version)
        elif args.command == "release-notes":
            extract_release_notes(args.changelog, args.version, args.destination)
        else:
            validate_binary(args.path, args.version, args.os, args.arch)
    except (ContractError, OSError, subprocess.SubprocessError, tarfile.TarError, zipfile.BadZipFile) as error:
        print(f"release contract failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
