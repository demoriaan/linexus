#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
udev_src="$repo_root/packaging/udev/99-linexus.rules"
udev_dst="/etc/udev/rules.d/99-linexus.rules"
service_src="$repo_root/packaging/systemd/user/linexusd.service"
inputd_user_service_src="$repo_root/packaging/systemd/user/linexus-inputd.service"
inputd_service_src="$repo_root/packaging/systemd/system/linexus-inputd.service"
desktop_src="$repo_root/packaging/desktop/io.github.demoriaan.linexus.desktop"

install_prefix="${NEXUS_INSTALL_PREFIX:-$HOME/.local}"
systemd_user_dir="${NEXUS_SYSTEMD_USER_DIR:-$HOME/.config/systemd/user}"
config_home="${XDG_CONFIG_HOME:-$HOME/.config}"
config_path="$config_home/linexus/config.json"
applications_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"

want_udev=1
want_build=1
want_config=1
want_enable=1
want_start=0
want_uninstall=0
want_purge_config=0

usage() {
  cat <<USAGE
Usage: $0 [OPTIONS]

Installs Linexus from this checkout into your home directory.

Default non-root behavior:
  - cargo build --release -p nexusctl -p nexus-daemon -p nexus-inputd -p nexus-studio
  - install user binaries to ~/.local/bin
  - create default config if missing
  - install and enable the systemd --user service, but do not start it
  - install and enable the rootless systemd --user input helper when /dev/uinput is user-writable
  - install udev rule through sudo unless --no-udev is used

Options:
  --no-udev      Skip udev installation/reload
  --no-build     Reuse existing release binaries
  --no-config    Do not create default config
  --no-enable    Do not enable systemd --user service
  --start        Start/restart systemd --user service after install
  --uninstall    Remove installed binaries/services; keep user config by default
  --purge-config Remove user config as part of --uninstall
  -h, --help     Show this help

Environment overrides for tests/custom installs:
  NEXUS_INSTALL_PREFIX       default: ~/.local
  NEXUS_SYSTEMD_USER_DIR     default: ~/.config/systemd/user
  XDG_CONFIG_HOME            default: ~/.config
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-udev) want_udev=0 ;;
    --no-build) want_build=0 ;;
    --no-config) want_config=0 ;;
    --no-enable) want_enable=0 ;;
    --start) want_start=1 ;;
    --uninstall) want_uninstall=1 ;;
    --purge-config) want_purge_config=1 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

uninstall_root_artifacts() {
  rm -f "$udev_dst"
  rm -f /etc/systemd/system/linexus-inputd.service
  rm -f /usr/local/lib/linexus/linexus-inputd
  rmdir /usr/local/lib/linexus 2>/dev/null || true
  systemctl daemon-reload || true
  udevadm control --reload-rules || true
  udevadm trigger --subsystem-match=hidraw || true
  echo "Removed root artifacts: udev rule, input helper service/binary"
}

uninstall_user_artifacts() {
  local bin_dir="$install_prefix/bin"
  local service_dst="$systemd_user_dir/linexusd.service"
  local inputd_user_service_dst="$systemd_user_dir/linexus-inputd.service"

  if [[ "$systemd_user_dir" == "$HOME/.config/systemd/user" ]] && command -v systemctl >/dev/null 2>&1; then
    systemctl --user disable --now linexus-inputd.service 2>/dev/null || true
    systemctl --user disable --now linexusd.service 2>/dev/null || true
    systemctl --user daemon-reload 2>/dev/null || true
  fi

  rm -f "$bin_dir/linexusctl" "$bin_dir/linexusd" "$bin_dir/linexus-inputd" "$bin_dir/linexus-studio" "$bin_dir/nexus-studio"
  rm -f "$service_dst"
  rm -f "$inputd_user_service_dst"
  rm -f "$applications_dir/io.github.demoriaan.linexus.desktop"
  find "${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor" -path '*/apps/io.github.demoriaan.linexus.*' -delete 2>/dev/null || true
  rm -rf "$install_prefix/share/linexus/studio"
  rm -rf "$install_prefix/share/linexus/docs" "${XDG_DATA_HOME:-$HOME/.local/share}/doc/linexus"
  rmdir "$install_prefix/share/linexus" 2>/dev/null || true
  rmdir "$install_prefix/share" 2>/dev/null || true

  if [[ $want_purge_config -eq 1 ]]; then
    rm -f "$config_path"
    rmdir "$(dirname "$config_path")" 2>/dev/null || true
    echo "Removed user config: $config_path"
  else
    echo "Kept user config: $config_path"
  fi

  echo "Removed user artifacts from $install_prefix and $systemd_user_dir"
}

install_udev_rule() {
  if [[ ! -f "$udev_src" ]]; then
    echo "missing $udev_src" >&2
    exit 1
  fi
  install -m 0644 "$udev_src" "$udev_dst"
  if [[ -f "$repo_root/target/release/linexus-inputd" ]]; then
    install -d -m 0755 /usr/local/lib/linexus
    install -m 0755 "$repo_root/target/release/linexus-inputd" /usr/local/lib/linexus/linexus-inputd
    if [[ -f "$inputd_service_src" ]]; then
      tmp_service="$(mktemp)"
      sed "s/--allow-uid 1000/--allow-uid ${SUDO_UID:-1000}/" "$inputd_service_src" > "$tmp_service"
      install -m 0644 "$tmp_service" /etc/systemd/system/linexus-inputd.service
      rm -f "$tmp_service"
      systemctl daemon-reload || true
      echo "Installed /etc/systemd/system/linexus-inputd.service for uid ${SUDO_UID:-1000}"
      echo "Start input helper after review with: sudo systemctl enable --now linexus-inputd.service"
    fi
  else
    echo "Input helper binary not built yet; skipping /usr/local/lib/linexus/linexus-inputd"
  fi
  udevadm control --reload-rules
  udevadm trigger --subsystem-match=hidraw || true
  echo "Installed $udev_dst"
}

if [[ ${EUID:-$(id -u)} -eq 0 ]]; then
  if [[ $want_uninstall -eq 1 ]]; then
    uninstall_root_artifacts
    exit 0
  fi
  install_udev_rule
  cat <<MSG

Root part complete.
Now unplug and replug the Corsair iCUE Nexus.
For user binaries/config/service, run this script once as your normal user:
  $repo_root/packaging/scripts/install-dev.sh --no-udev
MSG
  exit 0
fi

cd "$repo_root"

if [[ $want_uninstall -eq 1 ]]; then
  uninstall_user_artifacts
  if [[ $want_udev -eq 1 ]]; then
    if command -v sudo >/dev/null 2>&1; then
      echo "Removing root artifacts via sudo..."
      sudo "$0" --uninstall --no-build --no-config --no-enable
    else
      echo "sudo not found; remove root artifacts manually:" >&2
      echo "  rm -f '$udev_dst' /etc/systemd/system/linexus-inputd.service /usr/local/lib/linexus/linexus-inputd" >&2
    fi
  fi
  exit 0
fi

if [[ $want_build -eq 1 ]]; then
  cargo build --release -p nexusctl -p nexus-daemon -p nexus-inputd -p nexus-studio
fi

install -d -m 0755 "$install_prefix/bin"
install -m 0755 "$repo_root/target/release/linexusctl" "$install_prefix/bin/linexusctl"
install -m 0755 "$repo_root/target/release/linexusd" "$install_prefix/bin/linexusd"
install -m 0755 "$repo_root/target/release/linexus-inputd" "$install_prefix/bin/linexus-inputd"
install -m 0755 "$repo_root/target/release/linexus-studio" "$install_prefix/bin/linexus-studio"
# Older installs copied the UI next to the binary; it is embedded now.
rm -rf "$install_prefix/share/linexus/studio" "$install_prefix/bin/nexus-studio"
echo "Installed binaries:"
echo "  $install_prefix/bin/linexusctl"
echo "  $install_prefix/bin/linexusd"
echo "  $install_prefix/bin/linexus-inputd"
echo "  $install_prefix/bin/linexus-studio"
doc_dir="${XDG_DATA_HOME:-$HOME/.local/share}/doc/linexus"
install -d -m 0755 "$doc_dir"
install -m 0644 "$repo_root/docs/input-helper.md" "$doc_dir/input-helper.md"

if [[ -f "$desktop_src" ]]; then
  install -d -m 0755 "$applications_dir"
  tmp_desktop="$(mktemp)"
  sed "s#^Exec=linexus-studio#Exec=$install_prefix/bin/linexus-studio#" "$desktop_src" > "$tmp_desktop"
  install -m 0644 "$tmp_desktop" "$applications_dir/io.github.demoriaan.linexus.desktop"
  rm -f "$tmp_desktop"
  echo "Installed desktop launcher: $applications_dir/io.github.demoriaan.linexus.desktop"
fi

icons_src="$repo_root/packaging/icons/hicolor"
if [[ -d "$icons_src" ]]; then
  icons_dir="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor"
  while IFS= read -r -d '' icon; do
    install -D -m 0644 "$icon" "$icons_dir/${icon#"$icons_src"/}"
  done < <(find "$icons_src" -type f -print0)
  if command -v gtk-update-icon-cache >/dev/null 2>&1 && [[ -f "$icons_dir/index.theme" ]]; then
    gtk-update-icon-cache -q -t "$icons_dir" || true
  fi
  echo "Installed app icon: $icons_dir/*/apps/io.github.demoriaan.linexus.*"
fi

if [[ $want_config -eq 1 ]]; then
  if [[ -f "$config_path" ]]; then
    "$install_prefix/bin/linexusctl" config upgrade --path "$config_path"
  else
    "$install_prefix/bin/linexusctl" config init --path "$config_path"
  fi
fi

install -d -m 0755 "$systemd_user_dir"
if [[ "$install_prefix" == "$HOME/.local" ]]; then
  install -m 0644 "$service_src" "$systemd_user_dir/linexusd.service"
  install -m 0644 "$inputd_user_service_src" "$systemd_user_dir/linexus-inputd.service"
else
  tmp_user_service="$(mktemp)"
  sed "s#%h/.local/bin/linexusd#$install_prefix/bin/linexusd#" "$service_src" > "$tmp_user_service"
  install -m 0644 "$tmp_user_service" "$systemd_user_dir/linexusd.service"
  rm -f "$tmp_user_service"

  tmp_inputd_user_service="$(mktemp)"
  sed "s#%h/.local/bin/linexus-inputd#$install_prefix/bin/linexus-inputd#g; s#%h/.local/share/doc/linexus/input-helper.md#$doc_dir/input-helper.md#g" "$inputd_user_service_src" > "$tmp_inputd_user_service"
  install -m 0644 "$tmp_inputd_user_service" "$systemd_user_dir/linexus-inputd.service"
  rm -f "$tmp_inputd_user_service"
fi
echo "Installed user service: $systemd_user_dir/linexusd.service"
echo "Installed user input helper service: $systemd_user_dir/linexus-inputd.service"

if [[ $want_enable -eq 1 && "$systemd_user_dir" == "$HOME/.config/systemd/user" ]]; then
  systemctl --user daemon-reload
  systemctl --user enable linexusd.service
  echo "Enabled systemd user service: linexusd.service"
  if [[ -r /dev/uinput && -w /dev/uinput ]]; then
    systemctl --user enable linexus-inputd.service
    echo "Enabled rootless user input helper: linexus-inputd.service"
  else
    echo "User input helper service installed but not enabled because /dev/uinput is not user-writable."
    echo "Use the root helper instead after review:"
    echo "  sudo systemctl enable --now linexus-inputd.service"
  fi
  if [[ $want_start -eq 1 ]]; then
    systemctl --user restart linexusd.service
    echo "Started systemd user service: linexusd.service"
    if [[ -r /dev/uinput && -w /dev/uinput ]]; then
      systemctl --user restart linexus-inputd.service
      echo "Started rootless user input helper: linexus-inputd.service"
    fi
  else
    echo "Service not started by default. Start after udev/replug with:"
    echo "  systemctl --user start linexusd.service"
    if [[ -r /dev/uinput && -w /dev/uinput ]]; then
      echo "  systemctl --user start linexus-inputd.service"
    fi
  fi
elif [[ $want_enable -eq 1 ]]; then
  echo "Skipping systemctl enable because NEXUS_SYSTEMD_USER_DIR is overridden: $systemd_user_dir"
else
  echo "Skipped systemd enable by request."
fi

if [[ $want_udev -eq 1 ]]; then
  if command -v sudo >/dev/null 2>&1; then
    echo "Installing udev rule via sudo..."
    sudo "$0" --no-build --no-config --no-enable
  else
    echo "sudo not found; install udev manually as root:" >&2
    echo "  install -m 0644 '$udev_src' '$udev_dst'" >&2
    echo "  udevadm control --reload-rules" >&2
  fi
else
  echo "Skipped udev install by request."
fi

cat <<MSG

Next steps:
  1. Unplug and replug the Nexus, then check the setup:
       $install_prefix/bin/linexusctl doctor
  2. Open Linexus Studio from the app launcher, or run:
       $install_prefix/bin/linexus-studio

  Keyboard shortcut and typing actions need the input helper. If /dev/uinput
  is writable for your user:
       systemctl --user enable --now linexus-inputd.service
  Otherwise use the system service:
       sudo systemctl enable --now linexus-inputd.service
MSG
