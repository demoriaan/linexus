#!/usr/bin/env bash
# Builds the .deb and .rpm packages from already built binaries with nfpm
# (https://nfpm.goreleaser.com):
#   build-linux-packages.sh <bin-dir> <out-dir>
# Writes linexus_<version>-1_<arch>.deb and linexus-<version>-1.<arch>.rpm.
# Set NFPM to the nfpm binary when it is not on PATH.
set -euo pipefail

[[ $# -eq 2 ]] || { echo "usage: $0 <bin-dir> <out-dir>" >&2; exit 2; }
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
bin_dir="$(cd "$1" && pwd)"
out_dir="$(mkdir -p "$2" && cd "$2" && pwd)"
nfpm="${NFPM:-nfpm}"

version="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$repo_root/Cargo.toml")"
[[ -n "$version" ]] || { echo "cannot read the workspace version from Cargo.toml" >&2; exit 1; }
case "$(uname -m)" in
  x86_64) arch=amd64 ;;
  aarch64) arch=arm64 ;;
  *) echo "unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac

# The binaries need these glibc symbol versions at most; the package
# dependencies below must not promise less.
min_glibc=2.34
newest="$(cd "$bin_dir" && objdump -T linexusctl linexusd linexus-inputd linexus-studio | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -V | tail -n1)"
if [[ "$(printf '%s\n%s\n' "$min_glibc" "$newest" | sort -V | tail -n1)" != "$min_glibc" ]]; then
  echo "the binaries need glibc $newest, but the packages declare $min_glibc" >&2
  exit 1
fi

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
root="$stage/root"
"$repo_root/packaging/scripts/stage-root.sh" "$bin_dir" "$root"
{
  echo "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/"
  echo "Upstream-Name: Linexus"
  echo "Source: https://github.com/demoriaan/linexus"
  echo "Comment: Third-party licenses are in /usr/share/licenses/linexus/."
  echo
  echo "Files: *"
  echo "License: MIT"
  sed 's/^$/./; s/^/ /' "$repo_root/LICENSE"
} > "$stage/copyright"

# Every file is listed on its own: owning /usr/bin or /usr/lib would clash
# with the permissions the distribution gives those directories.
{
  cat <<EOF
name: linexus
arch: $arch
platform: linux
version: $version
release: 1
section: utils
priority: optional
maintainer: donno <donno@donno.de>
homepage: https://github.com/demoriaan/linexus
license: MIT
description: |-
  Linux driver and page editor for the Corsair iCUE Nexus touch strip
  Linexus runs the 640 x 48 touch display of the Corsair iCUE Nexus on Linux
  over USB HID: a daemon renders pages with system readings, media controls,
  app launchers, timers, feeds and Home Assistant switches, reads touches and
  runs actions. Linexus Studio, a local web app, designs the pages.
  Unofficial project, not affiliated with Corsair.
scripts:
  postinstall: $repo_root/packaging/deb-rpm/postinstall.sh
  preremove: $repo_root/packaging/deb-rpm/preremove.sh
  postremove: $repo_root/packaging/deb-rpm/postremove.sh
overrides:
  deb:
    depends:
      - libc6 (>= $min_glibc)
      - libgcc-s1
      - libudev1
    recommends:
      - wireplumber | pulseaudio-utils
      - curl
      - xdg-utils
    suggests:
      - upower
  rpm:
    depends:
      - libc.so.6(GLIBC_$min_glibc)(64bit)
      - libgcc_s.so.1()(64bit)
      - libudev.so.1()(64bit)
    recommends:
      - (wireplumber or pulseaudio-utils)
      - curl
      - xdg-utils
contents:
  - dst: /usr/lib/linexus
    type: dir
  - dst: /usr/share/doc/linexus
    type: dir
  - dst: /usr/share/licenses/linexus
    type: dir
  - src: $stage/copyright
    dst: /usr/share/doc/linexus/copyright
    packager: deb
    file_info:
      mode: 0644
EOF
  (cd "$root" && find . -type f | sort) | while read -r file; do
    mode=0644
    [[ -x "$root/$file" ]] && mode=0755
    printf '  - src: %s\n    dst: %s\n    file_info:\n      mode: %s\n' \
      "$root/${file#./}" "/${file#./}" "$mode"
  done
} > "$stage/nfpm.yaml"

for packager in deb rpm; do
  "$nfpm" package --config "$stage/nfpm.yaml" --packager "$packager" --target "$out_dir/"
done
ls -1 "$out_dir"/linexus*"$version"*.{deb,rpm}
