#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  cat <<USAGE
Usage: $0

Install the current iCUE Nexus udev rule, reload udev, trigger hidraw devices,
and restart the user daemon. This intentionally uses sudo and may prompt for
your password. Run it after doctor reports that /etc/udev/rules.d/99-linexus.rules
is stale or hidraw nodes are root-only after suspend/resume.
USAGE
  exit 0
fi

if ! command -v sudo >/dev/null 2>&1; then
  echo "sudo is required to install /etc/udev/rules.d/99-linexus.rules" >&2
  exit 1
fi
if ! command -v udevadm >/dev/null 2>&1; then
  echo "udevadm is required to reload and trigger udev rules" >&2
  exit 1
fi

rule_tmp="$(mktemp)"
trap 'rm -f "$rule_tmp"' EXIT

if command -v linexusctl >/dev/null 2>&1; then
  linexusctl print-udev-rule > "$rule_tmp"
else
  cargo run -q -p nexusctl -- print-udev-rule > "$rule_tmp"
fi

if ! getent group plugdev >/dev/null 2>&1; then
  echo "warning: plugdev group is missing; the rule's GROUP=plugdev fallback will not work until the group exists" >&2
elif ! id -nG | tr ' ' '\n' | grep -qx plugdev; then
  echo "warning: current user is not in plugdev; add the user or adjust the rule group" >&2
fi

echo "Installing current iCUE Nexus udev rule via sudo..."
sudo install -d -m 0755 /etc/udev/rules.d
sudo install -m 0644 "$rule_tmp" /etc/udev/rules.d/99-linexus.rules
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=hidraw || true

echo "Restarting iCUE Nexus user daemon..."
systemctl --user restart linexusd.service || true
sleep 1

echo "Current hidraw permissions:"
ls -l /dev/hidraw* 2>/dev/null | grep -E 'hidraw(9|10)|iCUE' || ls -l /dev/hidraw* 2>/dev/null || true

echo ""
echo "Doctor summary:"
if command -v linexusctl >/dev/null 2>&1; then
  linexusctl doctor | sed -n '1,80p'
else
  cargo run -q -p nexusctl -- doctor | sed -n '1,80p'
fi

echo ""
echo "If the Nexus hidraw nodes are still not group plugdev/rw, unplug and replug the Corsair iCUE Nexus, then run:"
echo "  systemctl --user restart linexusd.service"
echo "  linexusctl doctor"
echo "  ./packaging/scripts/verify-target-system-v1.sh --suspend"
