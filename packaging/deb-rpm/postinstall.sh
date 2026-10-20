#!/bin/sh
# Runs after the .deb or .rpm package is installed or upgraded.
# deb: "configure" [<old version>]; rpm: 1 on install, 2 or more on upgrade.

first_install=0
case "$1" in
  configure) [ -z "$2" ] && first_install=1 ;;
  1) first_install=1 ;;
esac

if command -v udevadm >/dev/null 2>&1; then
  udevadm control --reload-rules 2>/dev/null || true
  udevadm trigger --subsystem-match=hidraw 2>/dev/null || true
fi
if [ -d /run/systemd/system ]; then
  systemctl daemon-reload 2>/dev/null || true
  # Running input helpers pick up the new binary. The desktop user's
  # daemon keeps running the old one until it restarts; install.sh restarts
  # it.
  systemctl try-restart 'linexus-inputd@*.service' 2>/dev/null || true
fi

if [ "$first_install" = 1 ]; then
  cat <<'MSG'
Linexus is installed. As your desktop user, start the daemon now and at
every login, then unplug and replug the Corsair iCUE Nexus once:
  systemctl --user enable --now linexusd.service
Open "Linexus Studio" from the app menu; "linexusctl doctor" checks the setup.
MSG
fi
exit 0
