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
  # Keep the display alive between short-lived xdotool probes during startup.
  # Otherwise the last probe disconnecting can reset Xvfb under GTK.
  dbus-run-session -- xvfb-run -a -s "-screen 0 1280x1024x24 -noreset" bash -c '
    set -euo pipefail
    app=""
    window_pid=""
    openbox > /tmp/ptrack-openbox.log 2>&1 &
    wm=$!
    cleanup() {
      cat /tmp/ptrack-openbox.log
      test ! -f /tmp/ptrack-x11.log || cat /tmp/ptrack-x11.log
      if [ -n "$window_pid" ]; then kill "$window_pid" 2>/dev/null || true; fi
      if [ -n "$app" ]; then kill "$app" 2>/dev/null || true; fi
      kill "$wm" 2>/dev/null || true
      wait "$wm" 2>/dev/null || true
    }
    trap cleanup EXIT
    # GTK queries EWMH properties while mapping the window. A bare Xvfb
    # lacks the window manager present in an actual desktop session.
    for attempt in {1..30}; do
      xprop -root _NET_SUPPORTING_WM_CHECK | grep -q "window id" && break
      kill -0 "$wm"
      sleep 1
    done
    xprop -root _NET_SUPPORTING_WM_CHECK | grep -q "window id"
    args=(gui)
    [[ "$PTRACK_SMOKE_BINARY" = *.AppImage ]] && args=()
    "$PTRACK_SMOKE_BINARY" "${args[@]}" > /tmp/ptrack-x11.log 2>&1 &
    app=$!
    for attempt in {1..30}; do
      window=$(xdotool search --onlyvisible --name "p-track" | head -1) || true
      if [ -n "$window" ]; then
        # AppImage launchers can hand off to another process. Check the
        # actual GTK window and its owner, not the short-lived launcher PID.
        window_pid=$(xdotool getwindowpid "$window")
        sleep 3
        if ! kill -0 "$window_pid" 2>/dev/null; then
          status=0
          wait "$app" || status=$?
          echo "Desktop exited during startup (launcher status $status)" >&2
          exit 1
        fi
        xdotool getwindowname "$window" >/dev/null
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
    set -e
    cat /tmp/ptrack-wayland.log
    test "$status" = 124
    if grep -Ei "Aborting|panicked|Failed to initialize GTK" /tmp/ptrack-wayland.log; then exit 1; fi
  '
done
echo 'Installed package and AppImage: version, CLI, X11 window and Wayland startup passed.'
