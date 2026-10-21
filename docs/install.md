# Install Linexus

Linexus consists of four programs:

| Program | What it does |
|---|---|
| `linexusd` | The daemon. Draws the pages on the strip, reads touches and runs actions. Runs as a systemd user service. |
| `linexus-studio` | Linexus Studio, the local web configurator at `http://127.0.0.1:19777/`. |
| `linexusctl` | Command line tool for setup checks, diagnostics, previews and controlling the daemon. |
| `linexus-inputd` | Optional helper that types keyboard shortcuts and text. See [input-helper.md](input-helper.md). |

The strip itself is tested on Arch Linux with KDE Plasma (Wayland). The installer and the packages are tested in containers on Ubuntu 22.04 and 24.04, Debian 12 and 13, Fedora 44, Rocky Linux 9, openSUSE Tumbleweed and Arch Linux, and the GNOME focus extension with GNOME Shell 42, 46 and 48.

## Requirements

To run:

- Linux with a systemd user session. The daemon is meant to run as a systemd user service.
- The Corsair iCUE Nexus on USB (`1b1c:1b8e`). Studio and the emulator mode also work without it.

To build:

- Rust 1.88 or newer (`rust-version` in `Cargo.toml`).
- A C toolchain.
- libudev headers and `pkg-config`.

| Distribution | Build dependencies |
|---|---|
| Arch Linux | `sudo pacman -S --needed base-devel rust git` (`git` and `makepkg` are needed for the package build) |
| Debian, Ubuntu | `sudo apt install build-essential libudev-dev pkg-config`, plus Rust 1.88 or newer, for example from rustup |

Optional desktop tools. Without them only the matching features stop working; `linexusctl doctor` lists what is missing.

| Tool | Usually packaged as | Used for |
|---|---|---|
| `xdg-open` | xdg-utils | Opening links and files, launching apps |
| `wpctl` or `pactl` | wireplumber (PipeWire), or pulseaudio-utils (PulseAudio) | Volume, volume widget, microphone mute. `wpctl` is used when both exist. |
| `busctl` | systemd | Media controls (MPRIS), sleep detection, desktop notifications on the strip |
| `curl` | curl | Feeds, weather, Home Assistant, web request actions |
| `loginctl` | systemd | Lock screen action, lock detection |
| `upower` | upower | Battery levels of wireless mice, keyboards and headsets |
| `rocm-smi` | rocm-smi-lib | AMD GPU readings where sysfs does not provide them |
| `nvidia-smi` | nvidia-utils, usually installed with the NVIDIA driver (Fedora: xorg-x11-drv-nvidia-cuda from RPM Fusion; openSUSE: nvidia-compute-utils-G06) | NVIDIA GPU readings |

## Quick install

On x86_64 Linux, as your desktop user:

```bash
curl -fsSL https://github.com/demoriaan/linexus/releases/latest/download/install.sh | bash
```

The script fetches the files of the latest [release](https://github.com/demoriaan/linexus/releases) into `~/.cache/linexus/installer/<version>/` and checks them against the release's `SHA256SUMS`, which catches broken downloads. Then:

- **With a package manager it knows** it installs the matching package from Option 1 through `sudo`: `pacman` on Arch Linux, `apt-get` on Debian and Ubuntu, `dnf` on Fedora and RHEL, `zypper` on openSUSE, and their derivatives. After that it writes or upgrades your config and enables and starts `linexusd.service` for your user.
- **Otherwise** it unpacks the prebuilt binaries and runs Option 2's `install-dev.sh --bin-dir` from inside them. That puts the programs in `~/.local/bin` and installs the udev rule through `sudo`. No Rust toolchain is needed.
- **When `~/.local/bin` already holds an install** and no package is installed, it updates that install in place instead of adding a package next to it. To switch to the package, run it with `--uninstall` first.

It ends with `linexusctl doctor`. Running it again is safe, for example to update to a new release. To pass options, download it first, or use `... | bash -s -- --help`:

| Option | Effect |
|---|---|
| `--version X.Y.Z` | Install this release instead of the latest one |
| `--method auto\|arch\|deb\|rpm\|tarball` | `auto` picks the package for this system's package manager, as described above |
| `--from DIR` | Use release files already downloaded to `DIR` |
| `--no-root` | Never call `sudo`; print the commands that need root instead |
| `--no-start` | Enable the daemon, but do not start it now |
| `--input-helper` | Also enable the input helper for keyboard shortcut and text actions |
| `--uninstall` | Remove Linexus; your config stays |

Exit codes: `0` done, `1` error, `2` unknown option, `3` root steps still open. With `3` it prints the commands that need root: with `--no-root`, or when `sudo` needs a password and there is no terminal to ask on. Run them, then run the installer again. With a package nothing is installed until then, because the root step is the package itself. [ai-agents.md](ai-agents.md) describes the same flow for coding agents.

The binaries in the `.deb`, the `.rpm` and the tarball are built on Ubuntu 22.04 and need glibc 2.34 or newer: Ubuntu 22.04, Debian 12, Fedora 35, RHEL 9 and their derivatives, or newer releases. On older systems the installer stops with a message; build from source there. On Arch derivatives whose glibc lags behind Arch, use `--method tarball`.

## Option 1: package

Download the package for your distribution from the [latest release](https://github.com/demoriaan/linexus/releases/latest) and install it:

| Distribution | Release file | Install command |
|---|---|---|
| Arch Linux, Manjaro, EndeavourOS | `linexus-<version>-1-x86_64.pkg.tar.zst` | `sudo pacman -U ./linexus-*-x86_64.pkg.tar.zst` |
| Debian, Ubuntu, Linux Mint, Pop!_OS | `linexus_<version>-1_amd64.deb` | `sudo apt install ./linexus_*_amd64.deb` |
| Fedora, RHEL, Rocky, AlmaLinux | `linexus-<version>-1.x86_64.rpm` | `sudo dnf install ./linexus-*.x86_64.rpm` |
| openSUSE | `linexus-<version>-1.x86_64.rpm` | `sudo zypper install --allow-unsigned-rpm ./linexus-*.x86_64.rpm` |

The packages are not signed; check them against `SHA256SUMS` from the same release with `sha256sum -c SHA256SUMS --ignore-missing`. To update, install the package of the new release the same way.

To build the `.deb` and `.rpm` yourself, build the binaries, then pack them with [nfpm](https://nfpm.goreleaser.com):

```bash
cargo build --release --locked -p nexusctl -p nexus-daemon -p nexus-inputd -p nexus-studio
packaging/scripts/build-linux-packages.sh target/release dist
```

The Arch package builds from a checkout of this repository:

```bash
./packaging/arch/build-local-package.sh
sudo pacman -U packaging/arch/linexus-*-x86_64.pkg.tar.zst
```

`build-local-package.sh` packs the committed state of the checkout (`git archive HEAD`) as a source tarball and runs `makepkg -f` with `packaging/arch/PKGBUILD`; uncommitted changes are not included. The build runs `cargo build --release --locked` and the full test suite (`cargo test --release --locked --workspace`). The package lands in `packaging/arch/`.

Every package is called `linexus` and installs:

| Path | Content |
|---|---|
| `/usr/bin/linexusd`, `/usr/bin/linexus-studio`, `/usr/bin/linexusctl` | Programs |
| `/usr/lib/linexus/linexus-inputd` | Input helper |
| `/usr/lib/udev/rules.d/99-linexus.rules` | Device access rule |
| `/usr/lib/systemd/user/linexusd.service` | Daemon user service |
| `/usr/lib/systemd/user/linexus-inputd.service` | Input helper user service |
| `/usr/lib/systemd/system/linexus-inputd@.service` | Input helper system service, one instance per user ID |
| `/usr/share/applications/io.github.demoriaan.linexus.desktop` | "Linexus Studio" launcher |
| `/usr/share/icons/hicolor/*/apps/io.github.demoriaan.linexus.*` | App icon |
| `/usr/share/doc/linexus/` | This documentation |

Installing reloads the udev rules, and upgrading restarts a running root input helper. Then, as your desktop user:

1. Unplug and replug the strip.
2. Start the daemon now and at every login:

   ```bash
   systemctl --user enable --now linexusd.service
   ```

3. Open Linexus Studio from the app launcher, or run `linexus-studio`.
4. Optional, for keyboard shortcut and text actions: set up the input helper as described in [input-helper.md](input-helper.md).

## Option 2: from source with install-dev.sh

`packaging/scripts/install-dev.sh` builds the release binaries and installs them into your home directory. Run it as your normal user, not with `sudo`; it calls `sudo` itself for the parts that need root.

```bash
./packaging/scripts/install-dev.sh --start
```

By default the script:

1. Runs `cargo build --release -p nexusctl -p nexus-daemon -p nexus-inputd -p nexus-studio`.
2. Installs `linexusctl`, `linexusd`, `linexus-inputd` and `linexus-studio` to `~/.local/bin`.
3. Installs `docs/input-helper.md` to `~/.local/share/doc/linexus/`.
4. Installs the "Linexus Studio" launcher to `~/.local/share/applications/` and the icons to `~/.local/share/icons/hicolor/`.
5. Writes the default config with `linexusctl config init` when `~/.config/linexus/config.json` does not exist. When it exists, it runs `linexusctl config upgrade` instead, which only changes files that use the old schema.
6. Installs `linexusd.service` and `linexus-inputd.service` to `~/.config/systemd/user/` and enables `linexusd.service`. The input helper user service is enabled only when your user can read and write `/dev/uinput`.
7. Runs itself through `sudo` to install the udev rule to `/etc/udev/rules.d/99-linexus.rules`, reload udev, and install the root variant of the input helper (`/usr/local/lib/linexus/linexus-inputd` and `/etc/systemd/system/linexus-inputd.service` for your user ID). The root input helper is installed but not enabled.

Without `--start`, the services are enabled but not started.

| Option | Effect |
|---|---|
| `--no-udev` | Skip everything that needs `sudo` (udev rule, root input helper) |
| `--no-build` | Reuse the binaries already in `target/release` |
| `--bin-dir DIR` | Install prebuilt binaries from `DIR` instead of building; implies `--no-build`. The release tarball uses `--bin-dir bin`. |
| `--no-config` | Do not create or upgrade the config |
| `--no-enable` | Do not enable the user services |
| `--start` | Start or restart the user services after installing |
| `--uninstall` | Remove what the script installed; keeps your config |
| `--purge-config` | With `--uninstall`: also remove `~/.config/linexus/config.json` |
| `-h`, `--help` | Show the help |

| Environment variable | Default | Effect |
|---|---|---|
| `NEXUS_INSTALL_PREFIX` | `~/.local` | Install prefix for binaries |
| `NEXUS_SYSTEMD_USER_DIR` | `~/.config/systemd/user` | Where the user units go. When set to anything else, the script does not run `systemctl --user enable`. |
| `XDG_CONFIG_HOME` | `~/.config` | Base directory of the config file |

Make sure `~/.local/bin` is on your `PATH`, so `linexusctl` and `linexus-studio` work in a terminal.

## Device access

Linux exposes the strip as a hidraw device that only root can open by default. The udev rule `packaging/udev/99-linexus.rules` gives your user access:

```text
KERNEL=="hidraw*", SUBSYSTEM=="hidraw", ATTRS{idVendor}=="1b1c", ATTRS{idProduct}=="1b8e", MODE="0660", GROUP="plugdev", TAG+="uaccess", TAG+="Corsair_iCUE_Nexus"
```

- `TAG+="uaccess"` lets the user logged in at the local seat open the device. This is the main mechanism on systemd desktops.
- `MODE="0660", GROUP="plugdev"` is a fallback for systems that drop the seat permissions after suspend and resume. It only helps when a `plugdev` group exists and your user is a member.

Both install options put the rule in place. To install or refresh it by hand:

```bash
linexusctl print-udev-rule | sudo tee /etc/udev/rules.d/99-linexus.rules >/dev/null
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=hidraw
```

Then unplug and replug the strip.

## Start the daemon

```bash
systemctl --user enable --now linexusd.service
```

Check that it runs and found the strip:

```bash
linexusctl daemon status
journalctl --user -u linexusd
```

The daemon can start before the strip is plugged in. It retries every 2 seconds and reports `device: Unavailable` in the meantime.

Only one program can drive the strip at a time. The daemon holds a lock file at `$XDG_RUNTIME_DIR/linexus/device.lock`. The `linexusctl` commands that talk to the strip directly (`info`, `brightness`, `blank`, `demo`, `image`, `touch`) take the same lock, so stop the daemon before you use them:

```bash
systemctl --user stop linexusd.service
linexusctl demo --pattern rgb-bars
systemctl --user start linexusd.service
```

To control a running daemon, use `linexusctl daemon ...` instead. See [local-api.md](local-api.md).

## Open Linexus Studio

Start "Linexus Studio" from your app launcher, or run:

```bash
linexus-studio
```

Studio listens on `http://127.0.0.1:19777/` and opens that address in your default browser with `xdg-open`. It keeps running after you close the tab; starting it again while it runs only opens the browser.

| Option | Default | Effect |
|---|---|---|
| `--port <PORT>` | `19777` | TCP port |
| `--host <HOST>` | `127.0.0.1` | Loopback address to bind. Only `127.0.0.1`, `::1` and `localhost` are accepted. |
| `--no-open` | off | Do not open a browser |
| `--section <SECTION>` | none | Open a section, for example `news` |
| `--config <CONFIG>` | `~/.config/linexus/config.json` | Config file to edit |
| `--socket-path <SOCKET_PATH>` | `$XDG_RUNTIME_DIR/linexus/daemon.sock` | Daemon socket |

See [studio.md](studio.md) for a tour.

## Check your setup

```bash
linexusctl doctor
```

`doctor` prints one line per check, grouped into Device, Services, Config and Desktop tools, with a hint under each problem:

- whether the strip is connected and your user can open it
- whether the installed udev rule matches the current one
- whether OpenLinkHub is running
- whether the daemon answers, and whether it drives the strip or runs in emulator mode
- whether the input helper answers
- whether the config file is valid and uses the current schema
- which desktop tools are missing

`linexusctl doctor --verbose` also lists the HID interfaces, binaries, service states, socket paths, the event log path, `/dev/uinput` permissions, your groups, hwmon sensors and MPRIS players.

`linexusctl doctor --fix` repairs what it can, then runs the checks:

- Creates the default config if there is none, or upgrades an old one (keeping a backup).
- Writes `~/.config/systemd/user/linexusd.service` pointing at the `linexusd` next to `linexusctl` (or on `PATH`) and enables it.
- Installs the root input helper to `/usr/local/lib/linexus/linexus-inputd` with `/etc/systemd/system/linexus-inputd.service` and enables it.
- Installs the udev rule to `/etc/udev/rules.d/99-linexus.rules`.

The root steps run directly when you are root, through `sudo -n` when it works without a password, and otherwise `--fix` prints the commands to run. Add `--dry-run` to see what `--fix` would change without changing anything:

```bash
linexusctl doctor --fix --dry-run
```

Studio shows the same kind of checks under **Device > Setup checks**, each with a command to copy.

## Other software that uses the strip

- **Corsair iCUE** does not run on Linux. Linexus talks to the strip on its own and needs no Corsair software.
- **OpenLinkHub** can also claim the Nexus. When both run, they fight over the strip. `linexusctl doctor` and Studio warn when a process named OpenLinkHub runs. Stop it while you use Linexus, for example with `sudo systemctl stop OpenLinkHub.service`, or turn off its Nexus support.
- **A second `linexusd`** cannot start while another one holds the device lock. The error names the lock file.

## Uninstall

Installed with the quick install script, either way:

```bash
curl -fsSL https://github.com/demoriaan/linexus/releases/latest/download/install.sh | bash -s -- --uninstall
```

Installed with `install-dev.sh`:

```bash
./packaging/scripts/install-dev.sh --uninstall
```

This stops and disables the user services and removes the binaries, user units, launcher, icons and installed docs. Through `sudo` it also removes the udev rule and the root input helper. Add `--no-udev` to skip the `sudo` part. Your config stays unless you add `--purge-config`, which removes `~/.config/linexus/config.json`.

Installed as a package:

```bash
systemctl --user disable --now linexusd.service linexus-inputd.service
sudo pacman -R linexus     # Arch
sudo apt remove linexus    # Debian, Ubuntu
sudo dnf remove linexus    # Fedora, RHEL
sudo zypper remove linexus # openSUSE
```

Removing the package also turns off the root input helper (`linexus-inputd@<uid>.service`).

Neither method removes all per-user data. To remove it as well:

```bash
rm -rf ~/.config/linexus ~/.local/state/linexus
```

`~/.config/linexus` holds `config.json` (pages and profiles) and `sensors.json` (feeds and connections, including the Home Assistant token). `~/.local/state/linexus` holds the event log, the last active page, feed caches and received webhooks.
