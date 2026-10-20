#!/bin/sh
# Runs after the .deb or .rpm package is removed (deb: "remove" or "purge",
# rpm: 0) or replaced by an upgrade (nothing to do then).

case "$1" in
  remove|purge|0) ;;
  *) exit 0 ;;
esac

if command -v udevadm >/dev/null 2>&1; then
  udevadm control --reload-rules 2>/dev/null || true
  udevadm trigger --subsystem-match=hidraw 2>/dev/null || true
fi
if [ -d /run/systemd/system ]; then
  systemctl daemon-reload 2>/dev/null || true
fi
exit 0
