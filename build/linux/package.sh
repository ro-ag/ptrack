#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
version=${1:?usage: package.sh VERSION [RUST_TARGET]}
target=${2:-$(rustc -vV | sed -n 's/^host: //p')}
case "$target" in
  x86_64-unknown-linux-gnu) arch=amd64 ;;
  aarch64-unknown-linux-gnu) arch=arm64 ;;
  *) echo "Unsupported Linux target: $target" >&2; exit 1 ;;
esac
test "$(rustc -vV | sed -n 's/^host: //p')" = "$target" || {
  echo 'AppImage packaging must run on the target architecture.' >&2; exit 1;
}
export PTRACK_BUILD_VERSION="$version"
export APPIMAGE_EXTRACT_AND_RUN=1
# The caller builds the frontend once, allowing CI's shared frontend artifact.
config=$(python3 -c 'import json,sys; print(json.dumps({"version":sys.argv[1],"build":{"beforeBuildCommand":""}}))' "$version")
npm --prefix frontend run tauri -- build --target "$target" \
  --bundles deb,rpm --ci --config "$config" -- --locked
# AppRun incorporates the desktop Exec arguments. Leave this one argument-free
# so `image version` remains a CLI call; no arguments select GUI in main.rs.
appimage_config=$(python3 -c 'import json,sys; print(json.dumps({"version":sys.argv[1],"build":{"beforeBuildCommand":""},"bundle":{"linux":{"deb":{"desktopTemplate":"../build/linux/ptrack-appimage.desktop"}}}}))' "$version")
npm --prefix frontend run tauri -- build --target "$target" \
  --bundles appimage --ci --config "$appimage_config" -- --locked
python3 tools/linux_appimage.py "${CARGO_TARGET_DIR:-target}/$target/release" "$version" "$arch"
python3 tools/linux_packages.py collect \
  "${CARGO_TARGET_DIR:-target}/$target/release" "$version" "$arch" dist
