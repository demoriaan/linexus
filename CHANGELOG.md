# Changelog

All notable changes to this project are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [semantic versioning](https://semver.org/).

## [1.2.0] - 2026-10-20

### Added

- Desktop notifications on the strip also come from GNOME apps (GApplication notifications) and from Flatpak apps through the notification portal, not only from apps that use libnotify.
- Notifications on the strip show the app's icon, or the picture the notification carries (a contact photo, for example), instead of a bell.

### Changed

- Notifications on the strip follow the desktop's notification settings: Do Not Disturb (only critical notifications, and on KDE Plasma the apps allowed during it), apps whose popups are turned off, and low-urgency notifications (never on GNOME, as set on Plasma). A notification that asks for less time than the configured duration leaves sooner, and an update of a notification replaces the one on the strip instead of queueing behind it.
- The daemon follows the session bus with `busctl` (systemd), which every supported system has, instead of `dbus-monitor`, and only while notifications are turned on.
- **Never show from** also matches an app's menu name and desktop file id.
- **Hide message text** also hides the notification's picture; the app's icon stays.
- A full queue of overlays drops its oldest entry instead of the newest, and a notification that keeps updating stays up at most twice the configured time.
- A failing command widget shows `! ` instead of `⚠ ` before the error, since the strip's font has no warning sign.

### Fixed

- Emoji the strip's font lacks are left out instead of being drawn as boxes, common symbols get ASCII stand-ins (`→` as `->`, `E♭` as `Eb`, `★` as `*`), and missing spaces (thin or narrow no-break spaces) are drawn as spaces. Letters the font lacks still show a box, and a letter written with a separate accent mark (as in macOS file names) is drawn as the composed letter.
- SVG images on the strip (app icons, notification icons, images in pages) never read files they point to, so an icon cannot make the daemon read a pipe or a device.

## [1.1.0] - 2026-10-20

### Added

- `.deb` package for Debian, Ubuntu and their derivatives, and `.rpm` package for Fedora, RHEL and openSUSE, in every release. Like the Arch package, they install the programs, the device rule and the services system-wide, so there is a single root step.
- The one-line installer uses them: it installs the package through `apt-get`, `dnf` or `zypper` where one of them exists (`--method deb` or `--method rpm` to choose), keeps updating an existing `~/.local` install in place, and removes the package again with `--uninstall`.
- Volume, the volume widget and microphone mute also work through PulseAudio's `pactl` when PipeWire's `wpctl` is missing, for example on Ubuntu 22.04 and Linux Mint 21.
- App-aware profiles on GNOME (GNOME Shell 40 and newer), through a small GNOME Shell extension that reports the focused app. `linexusctl gnome-extension install|remove|status` manages it; the installer sets it up on GNOME, and `linexusctl doctor` and Studio's setup checks report it.

### Changed

- The installer installs to `~/.local` on image-based systems with a read-only `/usr` (Fedora Silverblue, Kinoite and Bazzite, openSUSE Aeon and MicroOS, SteamOS), where a package cannot be installed the usual way.
- `linexusd.service` is also started by `graphical-session.target`, so it comes back at the next login when the systemd user manager stayed running in between. Run the installer, or `systemctl --user reenable linexusd.service`, to apply this to an existing install.
- The installer checks the glibc version before it downloads the programs and names the problem on systems older than glibc 2.34. It no longer needs `awk`.
- Removing a package (Arch, deb or rpm) also turns off the root input helper, and upgrading one restarts it.
- The Arch package build uses the same staging script as the `.deb` and `.rpm` (`packaging/scripts/stage-root.sh`); its contents are unchanged.
- [docs/ai-agents.md](docs/ai-agents.md) no longer names a specific agent tool.

### Fixed

- App-aware profiles did not switch after a fresh login: the daemon starts before the desktop announces itself and looked for the focus source only once. It now keeps looking until it finds one, and waits for KWin to accept its script.
- The overlay that appears when the track changes showed the track that had just ended instead of the new one. It also shows the new cover as soon as it has loaded.

## [1.0.1] - 2026-10-19

### Added

- One-line installer: `curl -fsSL https://github.com/demoriaan/linexus/releases/latest/download/install.sh | bash`. It installs the Arch package where `pacman` exists and the prebuilt binaries in `~/.local/bin` elsewhere, checks the downloads against the checksum list of the release, and prints the steps that need root when it cannot ask for a password or runs with `--no-root`.
- Every release carries an Arch Linux package, a tarball with x86_64 binaries for other distributions (glibc 2.35 or newer), the installer and `SHA256SUMS`, built by GitHub Actions.
- [docs/ai-agents.md](docs/ai-agents.md): installing Linexus with a coding agent.
- `install-dev.sh --bin-dir DIR` installs prebuilt binaries instead of building them.

### Changed

- `install-dev.sh` no longer stops when `udevadm` cannot reach udev, for example in a container.

## [1.0.0] - 2026-10-19

First public release.

### Strip

- New renderer with Barlow and Terminus fonts, Lucide icons, six colour themes and a custom accent per profile, page backgrounds, slide and fade transitions, and the correct pixel order and dithering for the panel.
- 15 widget types: clock, reading (number, bar, graph, ring), network, now playing, volume slider, button, headlines, weather, Home Assistant entity, timer, text with live values, image, command output and spacer.
- Gestures per widget: tap, double tap, hold and repeat while held. Pages follow the finger when swiped.
- Actions for apps, links, folders, media, volume, microphone, keyboard shortcuts, typed text, pages, profiles, brightness, display off, screen lock, Home Assistant, HTTP requests, programs (opt-in) and sequences.
- Overlays for desktop notifications, volume and track changes.
- App-aware profiles for KDE Plasma, Hyprland, Sway and X11; hidden pages with automatic return.
- Idle dimming, a display timeout that wakes on the first touch, and blanking while the session is locked.
- Readings for CPU, AMD GPUs, memory, disks, network, every hwmon sensor and the batteries of wireless peripherals.

### Linexus Studio

- Rebuilt editor: strip preview from the same renderer, drag and resize on an 8 px grid, widget library, inspector, page filmstrip with templates, undo and redo, autosave, live device mirror, command palette, English and German, light and dark mode.
- Sections for news feeds, connections (Home Assistant, weather location), device status and behaviour, setup checks with fixes, and `.nexuspack` import and export.
- The UI is embedded in the binary; browsers revalidate it after updates.

### Configuration

- Schema version 2 with profiles, pages and widgets. Version 1 configs are upgraded on first start, and the original is kept as `config.json.v1.bak`.
