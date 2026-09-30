#!/usr/bin/env bash
set -euo pipefail
cd "$HOME"
for binary in /usr/bin/ptrack "$HOME/ptrack.AppImage"; do
  test "$("$binary" version)" = "ptrack $EXPECTED_VERSION"
  "$binary" --help >/dev/null
  # Exercise terminal-mode startup with a real PTY, separate from GUI launch.
  script -q -e -c "'$binary' --help" /dev/null >/dev/null
  export PTRACK_SMOKE_BINARY="$binary"
  echo "Testing $binary"
  # Variables intentionally expand in the child shell.
  # shellcheck disable=SC2016
  dbus-run-session -- xvfb-run -a bash -c '
    set -euo pipefail
    args=(gui)
    [[ "$PTRACK_SMOKE_BINARY" = *.AppImage ]] && args=()
    "$PTRACK_SMOKE_BINARY" "${args[@]}" > /tmp/ptrack-x11.log 2>&1 &
    app=$!
    trap "cat /tmp/ptrack-x11.log; kill $app 2>/dev/null || true" EXIT
    for attempt in {1..30}; do
      kill -0 "$app" || { cat /tmp/ptrack-x11.log; exit 1; }
      if xdotool search --onlyvisible --name "p-track" >/dev/null 2>&1; then
        sleep 3
        kill -0 "$app"
        if grep -Ei "Aborting|panicked|Failed to initialize GTK" /tmp/ptrack-x11.log; then exit 1; fi
        exit 0
      fi
      sleep 1
    done
    cat /tmp/ptrack-x11.log
    exit 1
  '
  # shellcheck disable=SC2016
  dbus-run-session -- bash -c '
    set -euo pipefail
    weston --backend=headless-backend.so --socket=ptrack-test --idle-time=0 --xwayland > /tmp/ptrack-weston.log 2>&1 &
    compositor=$!
    trap "kill $compositor 2>/dev/null || true" EXIT
    for attempt in {1..30}; do
      test -S "$XDG_RUNTIME_DIR/ptrack-test" && break
      kill -0 "$compositor"
      sleep 1
    done
    test -S "$XDG_RUNTIME_DIR/ptrack-test"
    backend=wayland
    if [[ "$PTRACK_SMOKE_BINARY" = *.AppImage ]]; then
      # Tauri linuxdeploy deliberately selects X11: validate XWayland in a
      # Wayland session, while native deb/rpm packages exercise GTK Wayland.
      backend=x11
      for attempt in {1..30}; do
        display=$(sed -n "s/.*xserver listening on display \\(:[0-9]*\\).*/\\1/p" /tmp/ptrack-weston.log)
        test -n "$display" && break
        sleep 1
      done
      test -n "$display" || { cat /tmp/ptrack-weston.log; exit 1; }
      export DISPLAY="$display"
    fi
    set +e
    GDK_BACKEND="$backend" WAYLAND_DISPLAY=ptrack-test timeout 10 "$PTRACK_SMOKE_BINARY" gui > /tmp/ptrack-wayland.log 2>&1
    status=$?
    cat /tmp/ptrack-wayland.log
    test "$status" = 124
    if grep -Ei "Aborting|panicked|Failed to initialize GTK" /tmp/ptrack-wayland.log; then exit 1; fi
  '
done
echo 'Installed package and AppImage: version, CLI, X11 window and Wayland startup passed.'
