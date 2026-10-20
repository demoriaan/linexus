#!/bin/sh
# Runs before the .deb or .rpm package is removed. On removal (deb: "remove"
# or "purge", rpm: 0) it turns off the input helpers that use the package's
# system unit; upgrades leave them running.

case "$1" in
  remove|purge|0) ;;
  *) exit 0 ;;
esac

for link in /etc/systemd/system/*.wants/linexus-inputd@*.service; do
  [ -L "$link" ] || continue
  unit="${link##*/}"
  systemctl disable --now "$unit" 2>/dev/null || systemctl disable "$unit" 2>/dev/null || true
done
exit 0
