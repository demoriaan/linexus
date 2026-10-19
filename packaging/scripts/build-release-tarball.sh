#!/usr/bin/env bash
# Builds the prebuilt binary archive for distributions without a package:
#   <out-dir>/linexus-<version>-<arch>-linux.tar.gz
# The archive keeps the repository layout for packaging/ and docs/, so
# packaging/scripts/install-dev.sh --bin-dir bin works from inside it.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
out_dir="$(mkdir -p "${1:-$repo_root/dist}" && cd "${1:-$repo_root/dist}" && pwd)"
target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"

version="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$repo_root/Cargo.toml")"
[[ -n "$version" ]] || { echo "cannot read the workspace version from Cargo.toml" >&2; exit 1; }
name="linexus-$version-$(uname -m)-linux"

cd "$repo_root"
cargo build --release --locked -p nexusctl -p nexus-daemon -p nexus-inputd -p nexus-studio

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
dest="$stage/$name"

for bin in linexusctl linexusd linexus-inputd linexus-studio; do
  install -Dm755 "$target_dir/release/$bin" "$dest/bin/$bin"
done

install -Dm755 packaging/scripts/install-dev.sh "$dest/packaging/scripts/install-dev.sh"
install -Dm755 packaging/scripts/apply-udev-rule.sh "$dest/packaging/scripts/apply-udev-rule.sh"
install -Dm644 packaging/udev/99-linexus.rules -t "$dest/packaging/udev/"
install -Dm644 packaging/systemd/user/*.service -t "$dest/packaging/systemd/user/"
install -Dm644 packaging/systemd/system/*.service -t "$dest/packaging/systemd/system/"
install -Dm644 packaging/desktop/*.desktop -t "$dest/packaging/desktop/"
(cd packaging/icons/hicolor && find . -type f -exec install -Dm644 {} "$dest/packaging/icons/hicolor/{}" \;)

install -Dm644 docs/*.md -t "$dest/docs/"
install -Dm644 README.md CHANGELOG.md LICENSE THIRD-PARTY-NOTICES.md -t "$dest/"
install -Dm644 crates/nexus-render/assets/fonts/OFL.txt "$dest/licenses/OFL-Barlow.txt"
install -Dm644 crates/nexus-render/src/font_headline.OFL.txt "$dest/licenses/OFL-Terminus.txt"
install -Dm644 crates/nexus-render/assets/icons/LICENSE "$dest/licenses/ISC-Lucide.txt"
install -Dm644 apps/nexus-studio/fonts/LICENSE-Inter.txt "$dest/licenses/OFL-Inter.txt"
install -Dm644 apps/nexus-studio/vendor/LICENSES.md "$dest/licenses/web-libraries.md"

mtime="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct 2>/dev/null || date +%s)}"
tar -C "$stage" --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$mtime" \
  -czf "$out_dir/$name.tar.gz" "$name"

echo "$out_dir/$name.tar.gz"
