#!/usr/bin/env bash
# Installs the packaged files into DESTDIR with the system layout that the
# Arch, Debian and RPM packages share:
#   stage-root.sh <bin-dir> <destdir>
# <bin-dir> holds linexusctl, linexusd, linexus-studio and linexus-inputd.
set -euo pipefail

[[ $# -eq 2 ]] || { echo "usage: $0 <bin-dir> <destdir>" >&2; exit 2; }
bin_dir="$(cd "$1" && pwd)"
dest="$2"
mkdir -p "$dest"
dest="$(cd "$dest" && pwd)"
cd "$(dirname "${BASH_SOURCE[0]}")/../.."

install -Dm755 "$bin_dir/linexusctl" "$dest/usr/bin/linexusctl"
install -Dm755 "$bin_dir/linexusd" "$dest/usr/bin/linexusd"
install -Dm755 "$bin_dir/linexus-studio" "$dest/usr/bin/linexus-studio"
install -Dm755 "$bin_dir/linexus-inputd" "$dest/usr/lib/linexus/linexus-inputd"

install -Dm644 packaging/udev/99-linexus.rules "$dest/usr/lib/udev/rules.d/99-linexus.rules"
install -Dm644 packaging/systemd/user/linexusd.service "$dest/usr/lib/systemd/user/linexusd.service"
install -Dm644 packaging/systemd/user/linexus-inputd.service "$dest/usr/lib/systemd/user/linexus-inputd.service"
install -Dm644 packaging/systemd/system/linexus-inputd.service "$dest/usr/lib/systemd/system/linexus-inputd@.service"
sed -i 's#%h/.local/bin/linexusd#/usr/bin/linexusd#' "$dest/usr/lib/systemd/user/linexusd.service"
sed -i 's#%h/.local/bin/linexus-inputd#/usr/lib/linexus/linexus-inputd#; s#file:%h/.local/share/doc/linexus/#file:/usr/share/doc/linexus/#' \
  "$dest/usr/lib/systemd/user/linexus-inputd.service"
sed -i 's#/usr/local/lib/linexus/linexus-inputd#/usr/lib/linexus/linexus-inputd#; s/--allow-uid 1000/--allow-uid %i/' \
  "$dest/usr/lib/systemd/system/linexus-inputd@.service"

install -Dm644 packaging/desktop/io.github.demoriaan.linexus.desktop "$dest/usr/share/applications/io.github.demoriaan.linexus.desktop"
(cd packaging/icons/hicolor && find . -type f -exec install -Dm644 {} "$dest/usr/share/icons/hicolor/{}" \;)

install -Dm644 README.md "$dest/usr/share/doc/linexus/README.md"
install -Dm644 docs/*.md -t "$dest/usr/share/doc/linexus/"
licenses="$dest/usr/share/licenses/linexus"
install -Dm644 LICENSE "$licenses/LICENSE"
install -Dm644 THIRD-PARTY-NOTICES.md "$licenses/THIRD-PARTY-NOTICES.md"
install -Dm644 crates/nexus-render/assets/fonts/OFL.txt "$licenses/OFL-Barlow.txt"
install -Dm644 crates/nexus-render/src/font_headline.OFL.txt "$licenses/OFL-Terminus.txt"
install -Dm644 crates/nexus-render/assets/icons/LICENSE "$licenses/ISC-Lucide.txt"
install -Dm644 apps/nexus-studio/fonts/LICENSE-Inter.txt "$licenses/OFL-Inter.txt"
install -Dm644 apps/nexus-studio/vendor/LICENSES.md "$licenses/web-libraries.md"
