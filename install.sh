#!/usr/bin/env bash
# Installs Linexus, the Linux driver and Studio for the Corsair iCUE Nexus,
# from the prebuilt files of a GitHub release.
#
#   curl -fsSL https://github.com/demoriaan/linexus/releases/latest/download/install.sh | bash
#
# Arch Linux and its derivatives get the linexus package through pacman.
# Other distributions get the prebuilt binaries in ~/.local/bin. Run it as
# your desktop user; it calls sudo itself for the parts that need root.
# Pass options after "bash -s --", for example: ... | bash -s -- --help
#
# Exit codes: 0 installed, 1 error, 2 bad option,
#             3 steps that need root are still open (printed; run them, then
#               run this script again).

set -euo pipefail

repo="demoriaan/linexus"

usage() {
  cat <<'USAGE'
Usage: install.sh [OPTIONS]

Installs Linexus for the Corsair iCUE Nexus from a GitHub release.

Options:
  --version X.Y.Z   Install this release instead of the latest one
  --method METHOD   auto (default), arch (pacman package) or tarball (~/.local)
  --from DIR        Use release files already downloaded to DIR
  --no-root         Never run sudo; print the commands that need root instead
  --no-start        Enable the daemon, but do not start it now
  --input-helper    Also enable the helper for keyboard shortcut and text actions
  --uninstall       Remove Linexus; your config in ~/.config/linexus stays
  -h, --help        Show this help

Environment: LINEXUS_VERSION works like --version.

Exit codes: 0 done, 1 error, 2 bad option,
            3 steps that need root are still open; they are printed. Run them,
              then run this script again.
USAGE
}

say() { printf '%s\n' "$*"; }
step() { printf '\n==> %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# True when sudo can ask for a password: there is a terminal to ask on.
has_tty() { { : </dev/tty; } 2>/dev/null; }

can_root() {
  [[ $use_root -eq 1 ]] && have sudo && { sudo -n true 2>/dev/null || has_tty; }
}

# Runs a command through sudo. When that is not possible, records it for the
# summary and returns 10.
as_root() {
  if can_root; then
    sudo "$@" </dev/null
  else
    pending_root+=("sudo $(printf '%q ' "$@")")
    return 10
  fi
}

user_systemd() {
  have systemctl && systemctl --user show-environment >/dev/null 2>&1
}

resolve_version() {
  version="${version#v}"
  if [[ $version != latest ]]; then
    return
  fi
  if [[ -n $from_dir ]]; then
    version="$(sed -n 's/.*  linexus-\([0-9][0-9.]*\)-[a-z0-9_]*-linux\.tar\.gz$/\1/p' "$from_dir/SHA256SUMS" | head -n1)"
    [[ -n $version ]] || die "cannot tell the version from $from_dir/SHA256SUMS; pass --version"
    return
  fi
  local url
  url="$(curl -fsSL -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest")" \
    || die "cannot reach github.com"
  version="${url##*/tag/v}"
  [[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]] || die "no published release found at https://github.com/$repo/releases"
}

fetch() {
  local name="$1"
  if [[ -n $from_dir ]]; then
    [[ -f "$from_dir/$name" ]] || die "$from_dir/$name not found"
    cp "$from_dir/$name" "$work/$name"
    return
  fi
  say "Downloading $name"
  curl -fsSL --retry 3 --proto '=https' -o "$work/$name" \
    "https://github.com/$repo/releases/download/v$version/$name" \
    || die "download failed: https://github.com/$repo/releases/download/v$version/$name"
}

# Prints the release file whose name matches the extended regex $1.
asset_matching() {
  sed -n 's/^[0-9a-f]\{64\}  //p' "$work/SHA256SUMS" | grep -E "^$1\$" | head -n1 || true
}

fetch_verified() {
  local name="$1"
  fetch "$name"
  (cd "$work" && awk -v n="$name" '$2 == n' SHA256SUMS | grep -q . \
    && awk -v n="$name" '$2 == n' SHA256SUMS | sha256sum --check --quiet --strict -) \
    || die "checksum mismatch for $name; delete $work and try again"
}

no_prebuilt() {
  if [[ $arch == x86_64 ]]; then
    die "release $version has no $1 for $arch; try --method arch or --method tarball"
  fi
  die "there are no prebuilt files for $arch; build from source: https://github.com/$repo/blob/master/docs/install.md"
}

# True when the root part of the tarball install (udev rule, root input
# helper) is already in place for these files.
root_part_current() {
  local dir="$1"
  cmp -s "$dir/packaging/udev/99-linexus.rules" /etc/udev/rules.d/99-linexus.rules \
    && cmp -s "$dir/bin/linexus-inputd" /usr/local/lib/linexus/linexus-inputd \
    && [[ -f /etc/systemd/system/linexus-inputd.service ]]
}

old_source_install() {
  [[ -x "$HOME/.local/bin/linexusd" ]]
}

setup_user_services() {
  local ctl="$1"
  if [[ -f "$config_path" ]]; then
    "$ctl" config upgrade --path "$config_path"
  else
    "$ctl" config init --path "$config_path"
  fi

  if ! user_systemd; then
    warn "no systemd user session found; start the daemon yourself with: linexusd"
    return
  fi
  systemctl --user daemon-reload
  if [[ $start -eq 1 ]]; then
    systemctl --user enable linexusd.service
    systemctl --user restart linexusd.service
  else
    systemctl --user enable linexusd.service
  fi
}

setup_input_helper() {
  local root_unit="$1"
  [[ $input_helper -eq 1 ]] || return 0
  step "Input helper"
  if [[ -r /dev/uinput && -w /dev/uinput ]] && user_systemd; then
    systemctl --user enable --now linexus-inputd.service
    say "Enabled the input helper as a user service."
  elif systemctl is-enabled --quiet "$root_unit" 2>/dev/null && systemctl is-active --quiet "$root_unit" 2>/dev/null; then
    say "The input helper already runs ($root_unit)."
  else
    as_root systemctl enable --now "$root_unit" || [[ $? -eq 10 ]] || die "could not enable $root_unit"
  fi
}

install_arch() {
  local pkg installed rc=0
  pkg="$(asset_matching "linexus-${version//./\\.}-[0-9]+-$arch\\.pkg\\.tar\\.zst")"
  [[ -n $pkg ]] || no_prebuilt "an Arch package"
  fetch_verified "$pkg"

  if old_source_install; then
    warn "found an install from source in ~/.local/bin. Its user service in ~/.config/systemd/user"
    warn "takes precedence over the package. Remove it with: packaging/scripts/install-dev.sh --uninstall"
  fi

  installed="$(pacman -Q linexus 2>/dev/null | cut -d' ' -f2 || true)"
  if [[ $installed == "$version"-* ]]; then
    say "linexus $installed is already installed."
  else
    step "Installing the package (needs root)"
    as_root pacman -U --noconfirm --needed "$work/$pkg" || rc=$?
    if [[ $rc -eq 10 ]]; then
      pending_after="Then run this installer again to set up the daemon for your user."
      return
    fi
    [[ $rc -eq 0 ]] || die "pacman could not install $pkg"
  fi

  /usr/bin/linexusctl --version >/dev/null 2>&1 \
    || die "the installed linexusctl does not run on this system; try: --uninstall, then --method tarball"

  step "Setting up the daemon for $(id -un)"
  setup_user_services /usr/bin/linexusctl
  setup_input_helper "linexus-inputd@$(id -u).service"
  ctl=/usr/bin/linexusctl
}

install_tarball() {
  local tarball dir script rc=0
  tarball="linexus-$version-$arch-linux.tar.gz"
  [[ -n "$(asset_matching "${tarball//./\\.}")" ]] || no_prebuilt "prebuilt binaries"
  fetch_verified "$tarball"
  tar -xzf "$work/$tarball" -C "$work"
  dir="$work/${tarball%.tar.gz}"
  script="$dir/packaging/scripts/install-dev.sh"
  "$dir/bin/linexusctl" --version >/dev/null 2>&1 \
    || die "the prebuilt binaries do not run on this system (they need glibc 2.35 or newer); build from source: https://github.com/$repo/blob/master/docs/install.md"

  local args=(--bin-dir "$dir/bin") root_done=0
  [[ $start -eq 1 ]] && args+=(--start)
  user_systemd || {
    warn "no systemd user session found; the user services are installed but not enabled"
    args+=(--no-enable)
  }
  if root_part_current "$dir"; then
    root_done=1
    args+=(--no-udev)
  elif ! can_root; then
    args+=(--no-udev)
  fi

  step "Installing to ~/.local"
  bash "$script" "${args[@]}" </dev/null || rc=$?
  [[ $rc -eq 0 ]] || die "install-dev.sh failed with exit code $rc"
  if [[ $root_done -eq 0 ]] && ! can_root; then
    pending_root+=("sudo bash $(printf '%q' "$script") --bin-dir $(printf '%q' "$dir/bin") --no-build --no-config --no-enable")
  fi

  setup_input_helper linexus-inputd.service
  ctl="$HOME/.local/bin/linexusctl"
  case ":$PATH:" in
    *":$HOME/.local/bin:"*) ;;
    *) warn "$HOME/.local/bin is not on your PATH; add it so that linexusctl and linexus-studio work in a terminal" ;;
  esac
}

uninstall() {
  local rc=0 script
  if have pacman && pacman -Q linexus >/dev/null 2>&1; then
    step "Removing the linexus package"
    if user_systemd; then
      systemctl --user disable --now linexusd.service linexus-inputd.service 2>/dev/null || true
    fi
    as_root systemctl disable --now "linexus-inputd@$(id -u).service" || [[ $? -eq 10 ]] || true
    as_root pacman -R --noconfirm linexus || rc=$?
    [[ $rc -eq 0 || $rc -eq 10 ]] || die "pacman could not remove linexus"
  fi

  if old_source_install; then
    step "Removing the install in ~/.local"
    local found=("$cache_root"/*/linexus-*-linux/packaging/scripts/install-dev.sh)
    script=""
    [[ ! -f ${found[0]} ]] || script="$(printf '%s\n' "${found[@]}" | sort -V | tail -n1)"
    if [[ -z $script ]]; then
      resolve_version
      work="$cache_root/$version"
      mkdir -p "$work"
      fetch SHA256SUMS
      local tarball="linexus-$version-$arch-linux.tar.gz"
      fetch_verified "$tarball"
      tar -xzf "$work/$tarball" -C "$work"
      script="$work/${tarball%.tar.gz}/packaging/scripts/install-dev.sh"
    fi
    if can_root; then
      bash "$script" --uninstall </dev/null
    else
      bash "$script" --uninstall --no-udev </dev/null
      pending_root+=("sudo bash $(printf '%q' "$script") --uninstall --no-build --no-config --no-enable")
    fi
  fi

  say ""
  say "Your config and state stay. To remove them too:"
  say "  rm -rf ~/.config/linexus ~/.local/state/linexus"
}

print_pending_root() {
  [[ ${#pending_root[@]} -gt 0 ]] || return 0
  step "Steps that need root"
  if [[ $use_root -eq 0 ]]; then
    say "--no-root was given, so these steps did not run."
  elif ! have sudo; then
    say "sudo is not installed, so these steps did not run. Run them as root."
  else
    say "sudo needs a password and there is no terminal to ask for it, so these steps did not run."
  fi
  say "Run them yourself, in this order:"
  local cmd
  for cmd in "${pending_root[@]}"; do
    if [[ $use_root -eq 1 ]] && ! have sudo; then
      # Run as root directly; SUDO_UID tells install-dev.sh whose input helper it sets up.
      say "  SUDO_UID=$(id -u) ${cmd#sudo }"
    else
      say "  $cmd"
    fi
  done
  [[ -z $pending_after ]] || say "$pending_after"
}

main() {
  version="${LINEXUS_VERSION:-latest}"
  method=auto
  from_dir=""
  use_root=1
  start=1
  input_helper=0
  do_uninstall=0
  pending_root=()
  pending_after=""
  ctl=""

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --version) [[ $# -ge 2 ]] || { usage >&2; exit 2; }; version="$2"; shift ;;
      --method) [[ $# -ge 2 ]] || { usage >&2; exit 2; }; method="$2"; shift ;;
      --from)
        [[ $# -ge 2 && -d $2 ]] || { printf -- '--from needs an existing directory\n' >&2; exit 2; }
        from_dir="$(cd "$2" && pwd)"
        shift
        ;;
      --no-root) use_root=0 ;;
      --no-start) start=0 ;;
      --input-helper) input_helper=1 ;;
      --uninstall) do_uninstall=1 ;;
      -h|--help) usage; exit 0 ;;
      *) printf 'unknown option: %s\n' "$1" >&2; usage >&2; exit 2 ;;
    esac
    shift
  done
  case "$method" in
    auto|arch|tarball) ;;
    *) printf 'unknown method: %s\n' "$method" >&2; usage >&2; exit 2 ;;
  esac

  [[ $(uname -s) == Linux ]] || die "Linexus runs on Linux only"
  [[ ${EUID:-$(id -u)} -ne 0 ]] || die "run this as your desktop user, not as root; it calls sudo itself"
  local tool
  for tool in curl tar sha256sum; do
    have "$tool" || die "$tool is missing; install it first"
  done

  arch="$(uname -m)"
  config_path="${XDG_CONFIG_HOME:-$HOME/.config}/linexus/config.json"
  cache_root="${XDG_CACHE_HOME:-$HOME/.cache}/linexus/installer"

  if [[ $do_uninstall -eq 1 ]]; then
    uninstall
    print_pending_root
    [[ ${#pending_root[@]} -eq 0 ]] || exit 3
    exit 0
  fi

  if [[ $method == auto ]]; then
    if have pacman; then method=arch; else method=tarball; fi
  fi

  resolve_version
  work="$cache_root/$version"
  mkdir -p "$work"
  step "Linexus $version ($method, $arch)"
  fetch SHA256SUMS

  if [[ $method == arch ]]; then
    install_arch
  else
    install_tarball
  fi

  if [[ -n $ctl && -x $ctl ]]; then
    step "Setup check"
    "$ctl" doctor || true
  fi

  print_pending_root
  [[ -z $pending_after ]] || exit 3
  step "Next"
  say "Unplug and replug the Nexus once, so the device rule applies."
  say "Then open \"Linexus Studio\" from your app menu, or run: linexus-studio"
  say "Studio runs at http://127.0.0.1:19777/ ; \"linexusctl doctor\" checks the setup."
  [[ ${#pending_root[@]} -eq 0 ]] || exit 3
}

main "$@"
