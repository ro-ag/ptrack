#!/usr/bin/env python3
"""Keep the host's Wayland ABI with its Mesa driver, then repack the AppImage."""
import argparse
import os
from pathlib import Path
import subprocess

from release_contract import require_version


def repack(release: Path, version: str, arch: str) -> None:
    require_version(version)
    bundle = release / "bundle" / "appimage"
    appdir = (bundle / "p-track.AppDir").resolve()
    appimage_arch = {"amd64": "amd64", "arm64": "aarch64"}[arch]
    image = (bundle / f"p-track_{version}_{appimage_arch}.AppImage").resolve()
    if not appdir.is_dir() or not image.is_file():
        raise ValueError("expected the built AppDir and versioned AppImage")
    # Mesa is supplied by the host. Loading it against an older bundled
    # libwayland can abort WebKit even on X11 (tauri-apps/tauri#15665).
    # Keep GTK/WebKit and their helpers together; only exclude Wayland's ABI.
    for library in (appdir / "usr" / "lib").rglob("libwayland-*.so*"):
        if library.is_file() or library.is_symlink():
            library.unlink()
    plugin = Path.home() / ".cache/tauri/linuxdeploy-plugin-appimage.AppImage"
    if not plugin.is_file():
        raise ValueError("Tauri's AppImage output plugin was not found")
    environment = dict(os.environ, APPIMAGE_EXTRACT_AND_RUN="1", LDAI_OUTPUT=str(image),
                       ARCH={"amd64": "x86_64", "arm64": "aarch64"}[arch])
    subprocess.run([str(plugin), f"--appdir={appdir}"], env=environment, check=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("release", type=Path)
    parser.add_argument("version")
    parser.add_argument("arch", choices=["amd64", "arm64"])
    args = parser.parse_args()
    repack(args.release, args.version, args.arch)
