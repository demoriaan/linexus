#!/usr/bin/env bash
# Builds the Arch package from the committed state of this checkout.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
pkgbuild_dir="$repo_root/packaging/arch"
pkgver="$(sed -n 's/^pkgver=//p' "$pkgbuild_dir/PKGBUILD")"
pkgname="$(sed -n 's/^pkgname=//p' "$pkgbuild_dir/PKGBUILD")"

command -v makepkg >/dev/null 2>&1 || {
  echo "makepkg not found. Install base-devel first." >&2
  exit 1
}

if [[ -n "$(git -C "$repo_root" status --porcelain --untracked-files=no)" ]]; then
  echo "Note: uncommitted changes are not part of the package; it is built from HEAD." >&2
fi

git -C "$repo_root" archive --format=tar.gz --prefix="${pkgname}-${pkgver}/" \
  -o "$pkgbuild_dir/${pkgname}-${pkgver}.tar.gz" HEAD
(
  cd "$pkgbuild_dir"
  rm -rf src pkg
  makepkg -f
)

echo "Package(s) written to $pkgbuild_dir"
