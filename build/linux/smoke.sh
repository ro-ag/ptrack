#!/usr/bin/env bash
# Run inside a disposable distro container as root; the app runs unprivileged.
set -euo pipefail
version=${1:?usage: smoke.sh VERSION ARCH}
arch=${2:?}
export DEBIAN_FRONTEND=noninteractive
if command -v apt-get >/dev/null; then
  apt-get update
  apt-get install -y --no-install-recommends \
    "/packages/p-track_${version}_linux_${arch}.deb" \
    xvfb xauth xdotool x11-utils openbox dbus-x11 desktop-file-utils weston xwayland util-linux libgl1-mesa-dri
else
  dnf install -y "/packages/p-track_${version}_linux_${arch}.rpm" \
    xorg-x11-server-Xvfb xorg-x11-xauth xdotool xprop openbox dbus-daemon \
    desktop-file-utils weston xorg-x11-server-Xwayland util-linux util-linux-script mesa-dri-drivers
fi
test -f /usr/lib/ptrack/package-manager
desktop=$(find /usr/share/applications -iname '*track*.desktop' -print -quit)
test -n "$desktop"
desktop-file-validate "$desktop"
grep -qx 'Exec=ptrack gui' "$desktop"
test -n "$(find /usr/share/icons -iname '*track*.png' -print -quit)"
useradd --create-home --shell /bin/bash smoke
mkdir -p /home/smoke/runtime
chmod 700 /home/smoke/runtime
cp "/packages/p-track_${version}_linux_${arch}.AppImage" /home/smoke/ptrack.AppImage
chmod 755 /home/smoke/ptrack.AppImage
chown -R smoke:smoke /home/smoke
runuser -u smoke -- env XDG_RUNTIME_DIR=/home/smoke/runtime \
  EXPECTED_VERSION="$version" APPIMAGE_EXTRACT_AND_RUN=1 \
  WEBKIT_DISABLE_COMPOSITING_MODE=1 LIBGL_ALWAYS_SOFTWARE=1 \
  bash /scripts/smoke-user.sh
