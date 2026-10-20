#!/usr/bin/env bash
# Installs Linexus, the Linux driver and Studio for the Corsair iCUE Nexus,
# from the prebuilt files of a GitHub release.
#
#   curl -fsSL https://github.com/demoriaan/linexus/releases/latest/download/install.sh | bash
#
# Arch Linux, Debian, Ubuntu, Fedora, openSUSE and their derivatives get the
# linexus package through their package manager; other distributions get the
# prebuilt binaries in ~/.local/bin. Run it as your desktop user; it calls
# sudo itself for the parts that need root.
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
  --method METHOD   auto (default), arch (pacman), deb (apt), rpm (dnf or
                    zypper) or tarball (prebuilt binaries in ~/.local)
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
  local name="$1" sum file entry=""
  fetch "$name"
  while read -r sum file; do
    [[ $file == "$name" ]] && entry="$sum  $file"
  done < "$work/SHA256SUMS"
  [[ -n $entry ]] || die "$name is not listed in SHA256SUMS"
  (cd "$work" && printf '%s\n' "$entry" | sha256sum --check --quiet --strict -) \
    || die "checksum mismatch for $name; delete $work and try again"
}

no_prebuilt() {
  if [[ $arch == x86_64 ]]; then
    die "release $version has no $1 for $arch; try --method tarball"
  fi
  die "there are no prebuilt files for $arch; build from source: https://github.com/$repo/blob/master/docs/install.md"
}

# The prebuilt binaries need glibc 2.34 or newer. When the version cannot be
# read, the package dependencies or the test run of a binary catch it later.
check_glibc() {
  local found
  found="$(getconf GNU_LIBC_VERSION 2>/dev/null || true)"
  found="${found#glibc }"
  [[ $found =~ ^[0-9]+\.[0-9]+ ]] || return 0
  if [[ "$(printf '%s\n2.34\n' "$found" | sort -V | head -n1)" != 2.34 ]]; then
    die "the prebuilt files need glibc 2.34 or newer, this system has $found; build from source: https://github.com/$repo/blob/master/docs/install.md"
  fi
}

# Arch Linux or a derivative (Manjaro, EndeavourOS, Garuda, SteamOS, ...).
# Debian and Ubuntu ship a game and a package manager called pacman, so the
# command alone is not enough.
arch_based() {
  have pacman || return 1
  (
    # shellcheck disable=SC1091
    . /etc/os-release 2>/dev/null || exit 1
    [[ " ${ID:-} ${ID_LIKE:-} " == *" arch "* ]]
  )
}

# The package format of this system's package manager, or nothing.
native_format() {
  if arch_based; then
    echo arch
  elif have apt-get && have dpkg; then
    echo deb
  elif have rpm && { have dnf || have zypper; }; then
    echo rpm
  fi
}

# Image-based systems keep /usr read-only: Fedora Silverblue, Kinoite and
# Bazzite (rpm-ostree), openSUSE Aeon and MicroOS (transactional-update),
# SteamOS. Their package manager cannot take a package the usual way; the
# ~/.local install works there.
immutable_system() {
  [[ -e /run/ostree-booted ]] && return 0
  have transactional-update && return 0
  have findmnt && findmnt -no OPTIONS -T /usr 2>/dev/null | tr ',' '\n' | grep -qx ro
}

# The installed version of the linexus package in format $1, or nothing.
installed_version() {
  case "$1" in
    arch) pacman -Q linexus 2>/dev/null | cut -d' ' -f2 || true ;;
    deb) dpkg-query -W -f='${Status}|${Version}' linexus 2>/dev/null | sed -n 's/^[a-z]* ok installed|//p' || true ;;
    # rpm -q prints "package linexus is not installed" on stdout.
    rpm) rpm -q --qf '%{VERSION}-%{RELEASE}\n' linexus 2>/dev/null | grep -v ' ' || true ;;
  esac
}

# Sets pkg_cmd to the package manager command for $1 (install or remove) in
# format $2. The package file follows for install.
set_pkg_cmd() {
  case "$2" in
    arch)
      if [[ $1 == install ]]; then pkg_cmd=(pacman -U --noconfirm --needed); else pkg_cmd=(pacman -R --noconfirm linexus); fi ;;
    deb)
      if [[ $1 == install ]]; then pkg_cmd=(apt-get install -y --allow-downgrades); else pkg_cmd=(apt-get remove -y linexus); fi ;;
    rpm)
      if have dnf; then
        if [[ $1 == install ]]; then pkg_cmd=(dnf install -y); else pkg_cmd=(dnf remove -y linexus); fi
      elif [[ $1 == install ]]; then
        pkg_cmd=(zypper --non-interactive install --allow-unsigned-rpm)
      else
        pkg_cmd=(zypper --non-interactive remove linexus)
      fi ;;
  esac
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

# On GNOME, app-aware profiles need a small shell extension; linexusctl
# installs it for this user. It also decides whether this is a GNOME Shell
# session, the same way the daemon does.
setup_gnome_focus() {
  local ctl="$1" status
  # Read all of it first: grep -q would close the pipe early and, with
  # pipefail, turn linexusctl's broken-pipe exit into a "no".
  status="$("$ctl" gnome-extension status 2>/dev/null || true)"
  grep -qx 'GNOME Shell session: yes' <<<"$status" || return 0
  step "GNOME focus extension"
  "$ctl" gnome-extension install \
    || warn "could not set up the GNOME focus extension; profiles will not follow the focused app. Retry with: linexusctl gnome-extension install"
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

install_package() {
  local format="$1" pattern pkg installed rc=0 v="${version//./\\.}"
  case "$format" in
    arch) have pacman || die "--method arch needs pacman"
          pattern="linexus-$v-[0-9]+-$arch\\.pkg\\.tar\\.zst" ;;
    deb)  have apt-get || die "--method deb needs apt-get"
          pattern="linexus_$v-[0-9]+_$deb_arch\\.deb" ;;
    rpm)  have dnf || have zypper || die "--method rpm needs dnf or zypper"
          pattern="linexus-$v-[0-9]+\\.$arch\\.rpm" ;;
  esac
  pkg="$(asset_matching "$pattern")"
  if [[ -z $pkg && $auto_method -eq 1 ]]; then
    say "Release $version has no $format package; installing the prebuilt binaries to ~/.local instead."
    install_tarball
    return
  fi
  [[ -n $pkg ]] || no_prebuilt "$format package"
  [[ $format == arch ]] || check_glibc
  fetch_verified "$pkg"

  if old_source_install; then
    warn "found an install in ~/.local/bin. Its user service in ~/.config/systemd/user"
    warn "takes precedence over the package. Remove it with: packaging/scripts/install-dev.sh --uninstall"
  fi

  installed="$(installed_version "$format")"
  if [[ $installed == "$version"-* ]]; then
    say "linexus $installed is already installed."
  else
    step "Installing the package (needs root)"
    set_pkg_cmd install "$format"
    as_root "${pkg_cmd[@]}" "$work/$pkg" || rc=$?
    if [[ $rc -eq 10 ]]; then
      pending_after="Then run this installer again to set up the daemon for your user."
      return
    fi
    # zypper also ends with informational codes (106, 107) after a good
    # install, so the result counts, not the exit code.
    installed="$(installed_version "$format")"
    if [[ $installed != "$version"-* ]]; then
      if [[ $rc -ne 0 ]]; then
        die "${pkg_cmd[0]} could not install $pkg (exit code $rc); see its output above, or try --method tarball"
      fi
      die "${pkg_cmd[0]} kept linexus ${installed:-uninstalled} instead of $version; run with --uninstall first, then install again"
    fi
    [[ $rc -eq 0 ]] || warn "${pkg_cmd[0]} ended with exit code $rc, but linexus $installed is installed"
  fi

  /usr/bin/linexusctl --version >/dev/null 2>&1 \
    || die "the installed linexusctl does not run on this system; try: --uninstall, then --method tarball"

  step "Setting up the daemon for $(id -un)"
  setup_user_services /usr/bin/linexusctl
  setup_gnome_focus /usr/bin/linexusctl
  setup_input_helper "linexus-inputd@$(id -u).service"
  ctl=/usr/bin/linexusctl
}

install_tarball() {
  local tarball dir script rc=0
  tarball="linexus-$version-$arch-linux.tar.gz"
  [[ -n "$(asset_matching "${tarball//./\\.}")" ]] || no_prebuilt "prebuilt binaries"
  check_glibc
  fetch_verified "$tarball"
  tar -xzf "$work/$tarball" -C "$work"
  dir="$work/${tarball%.tar.gz}"
  script="$dir/packaging/scripts/install-dev.sh"
  "$dir/bin/linexusctl" --version >/dev/null 2>&1 \
    || die "the prebuilt binaries do not run on this system; build from source: https://github.com/$repo/blob/master/docs/install.md"

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

  setup_gnome_focus "$HOME/.local/bin/linexusctl"
  setup_input_helper linexus-inputd.service
  ctl="$HOME/.local/bin/linexusctl"
  case ":$PATH:" in
    *":$HOME/.local/bin:"*) ;;
    *) warn "$HOME/.local/bin is not on your PATH; add it so that linexusctl and linexus-studio work in a terminal" ;;
  esac
}

uninstall() {
  local rc=0 script format helper tool
  helper="linexus-inputd@$(id -u).service"
  format="$(native_format)"
  for tool in /usr/bin/linexusctl "$HOME/.local/bin/linexusctl"; do
    if [[ -x $tool && -d "${XDG_DATA_HOME:-$HOME/.local/share}/gnome-shell/extensions/linexus-focus@demoriaan.github.io" ]]; then
      step "Removing the GNOME focus extension"
      "$tool" gnome-extension remove || warn "could not remove the GNOME focus extension"
      break
    fi
  done
  if [[ -n $format && -n "$(installed_version "$format")" ]]; then
    step "Removing the linexus package"
    if user_systemd; then
      systemctl --user disable --now linexusd.service linexus-inputd.service 2>/dev/null || true
    fi
    if systemctl is-enabled --quiet "$helper" 2>/dev/null; then
      as_root systemctl disable --now "$helper" || [[ $? -eq 10 ]] || true
    fi
    set_pkg_cmd remove "$format"
    as_root "${pkg_cmd[@]}" || rc=$?
    [[ $rc -eq 0 || $rc -eq 10 ]] || die "${pkg_cmd[0]} could not remove linexus"
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
    auto|arch|deb|rpm|tarball) ;;
    *) printf 'unknown method: %s\n' "$method" >&2; usage >&2; exit 2 ;;
  esac

  [[ $(uname -s) == Linux ]] || die "Linexus runs on Linux only"
  [[ ${EUID:-$(id -u)} -ne 0 ]] || die "run this as your desktop user, not as root; it calls sudo itself"
  local tool
  for tool in curl tar sha256sum; do
    have "$tool" || die "$tool is missing; install it first"
  done

  arch="$(uname -m)"
  case "$arch" in
    x86_64) deb_arch=amd64 ;;
    aarch64) deb_arch=arm64 ;;
    *) deb_arch="$arch" ;;
  esac
  config_path="${XDG_CONFIG_HOME:-$HOME/.config}/linexus/config.json"
  cache_root="${XDG_CACHE_HOME:-$HOME/.cache}/linexus/installer"

  if [[ $do_uninstall -eq 1 ]]; then
    uninstall
    print_pending_root
    [[ ${#pending_root[@]} -eq 0 ]] || exit 3
    exit 0
  fi

  auto_method=0
  if [[ $method == auto ]]; then
    auto_method=1
    method="$(native_format)"
    if [[ -z $method ]]; then
      method=tarball
    elif immutable_system; then
      say "This system keeps /usr read-only; installing the prebuilt binaries to ~/.local."
      method=tarball
    elif old_source_install && [[ -z "$(installed_version "$method")" ]]; then
      say "Found an install in ~/.local; updating it in place."
      say "To switch to the $method package instead: run with --uninstall, then without."
      method=tarball
    fi
  fi

  resolve_version
  work="$cache_root/$version"
  mkdir -p "$work"
  step "Linexus $version ($method, $arch)"
  fetch SHA256SUMS

  if [[ $method == tarball ]]; then
    install_tarball
  else
    install_package "$method"
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
