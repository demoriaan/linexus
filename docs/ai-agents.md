# Installing Linexus with an AI agent

This page is for coding agents that install Linexus on someone's computer. A person can follow it too. Every command is non-interactive, except the root steps, which the agent hands to the person.

Linexus drives the Corsair iCUE Nexus, a 640 × 48 touch strip (USB `1b1c:1b8e`), on Linux. It consists of a daemon (`linexusd`, a systemd user service), a web configurator (`linexus-studio`, on `http://127.0.0.1:19777/`), a command line tool (`linexusctl`) and an optional input helper (`linexus-inputd`).

## 1. Check the system

Run these as the person's desktop user:

```sh
uname -sm                                  # expect: Linux x86_64
systemctl --user show-environment >/dev/null && echo "user session ok"
for pm in pacman apt-get dnf zypper; do command -v "$pm"; done   # which package manager the installer uses
```

- **Architecture.** Prebuilt files exist for x86_64 only. On other architectures, build from source as described in [install.md](install.md#option-2-from-source-with-install-devsh).
- **systemd user session.** Without it, the installer still installs the programs, but the daemon has to be started by hand.
- **The strip itself.** `lsusb -d 1b1c:1b8e` shows whether it is plugged in. Installing works without it.
- **glibc.** `getconf GNU_LIBC_VERSION` must show 2.34 or newer. The installer checks this too and stops with a message on older systems.
- **Ask first.** Installing adds system files: the `linexus` package, or a udev rule in `/etc/udev/rules.d/`. Ask the person before you start.

## 2. Install

```sh
curl -fsSL https://github.com/demoriaan/linexus/releases/latest/download/install.sh -o linexus-install.sh \
  && bash linexus-install.sh --no-root
echo "exit code: $?"
```

Download the script first, as above, so you can read it and see its exit code. Do not run it with `sudo` or as root: it refuses, because the user services must belong to the desktop user.

Use `--no-root`. Without it, the installer calls `sudo` itself. That is right for a person at a terminal, but in an agent's pseudo-terminal `sudo` can wait for a password nobody types. With `--no-root` it does everything that needs no root, prints the rest, and exits with `3`.

| Exit code | Meaning | What to do |
|---|---|---|
| `0` | Installed and set up | Go to step 3 |
| `1` | Error, message on stderr | Fix the cause and run it again; it is safe to repeat |
| `2` | Unknown option | Check the options below |
| `3` | Steps that need root are still open; they are printed | See "Root steps" |

Options, passed after the script name:

| Option | Effect |
|---|---|
| `--version X.Y.Z` | Install a given release instead of the latest |
| `--method arch\|deb\|rpm\|tarball` | Force a package format, or the `~/.local` install. `auto` picks the package for the package manager it finds (pacman, apt-get, dnf or zypper), and keeps updating an existing `~/.local` install. |
| `--no-root` | Never call `sudo`; print the root commands instead. Recommended for agents. |
| `--no-start` | Enable the daemon, but do not start it |
| `--input-helper` | Also enable the helper for keyboard shortcut and text actions |
| `--from DIR` | Use release files already downloaded to `DIR` (offline install) |
| `--uninstall` | Remove Linexus; the config stays |

### Root steps

With exit code `3`, the installer has printed the commands that need root under `==> Steps that need root`.

1. Pass the exact commands to the person and ask them to run them. If your tool lets the person run a command in the conversation, suggest that, so that the output lands where you can read it. Do not try to get around `sudo`.
2. Run `bash linexus-install.sh --no-root` again. When the root steps are in place, it exits with `0`.

With a package (pacman, apt-get, dnf or zypper), the root step is the package installation itself, so nothing is installed before the person runs it. The second run then sets up the daemon for the user. With the `~/.local` install, the first run already installed the programs in `~/.local/bin`; the root step adds the device rule.

## 3. Verify

```sh
linexusctl doctor
linexusctl daemon status
```

- `doctor` prints one line per check, starting with `ok`, `warn` or `FAIL`, with a hint under each problem. It ends with `Everything looks good.` or with `N problem(s), M warning(s).` It always exits with `0`, so read the output instead of the exit code.
- `daemon status` prints `device: Connected` when the daemon drives the strip.

If `~/.local/bin` is not on `PATH` (tarball install), call `~/.local/bin/linexusctl`.

| Symptom | Cause and fix |
|---|---|
| `Nexus at ... cannot be opened by this user`, or `device: Unavailable` with the strip plugged in | The udev rule is new. Ask the person to unplug and replug the strip once. |
| `udev rule not installed` | The root step was skipped. Run the printed command, or `linexusctl print-udev-rule \| sudo tee /etc/udev/rules.d/99-linexus.rules`. |
| `OpenLinkHub is running (pid ...)` | OpenLinkHub also claims the strip. Ask the person before you stop it, because it may drive their other Corsair devices. |
| `daemon (linexusd) is not running` | `systemctl --user enable --now linexusd.service`, then check `journalctl --user -u linexusd -n 50`. |
| `GLIBC_2.xx not found` after the Arch package | The derivative lags behind Arch. Run `--uninstall`, then install with `--method tarball`. |
| `wpctl or pactl missing` | Volume actions need PipeWire's `wpctl` (package wireplumber) or PulseAudio's `pactl` (package pulseaudio-utils). |

## 4. Hand over

Tell the person:

- Open **Linexus Studio** from the app menu, or run `linexus-studio`. It opens `http://127.0.0.1:19777/` in the browser.
- Keyboard shortcut and text actions need the input helper. Install it with `--input-helper`, or see [input-helper.md](input-helper.md).

## Changing the pages for the person

The pages live in `~/.config/linexus/config.json`, described in [configuration.md](configuration.md). You can also change them, in this order:

1. Edit the config.
2. Check it with `linexusctl config validate`.
3. Render the page to an image with `linexusctl render-preview --config ~/.config/linexus/config.json --page <id> --scale 2 --out page.png`, and look at the image.
4. Apply it with `linexusctl daemon reload-config`.

`linexusctl daemon tap <x>` simulates a touch, and `linexusctl daemon frame --out now.png` saves what the strip shows. [local-api.md](local-api.md) lists everything.

## Do not

- Do not run the installer, `linexusd` or `linexus-studio` as root.
- Do not `chmod` the `/dev/hidraw*` devices. The udev rule grants access to the logged-in user.
- Do not install Corsair iCUE or run it through Wine. Linexus needs no Corsair software.
- Do not make Studio listen on other addresses. It accepts only loopback, on purpose.

## Uninstall

```sh
bash linexus-install.sh --uninstall --no-root
```

Hand the printed root steps to the person, as during the install. This removes the package or the `~/.local` install, the services and the udev rule. The config stays in `~/.config/linexus` and `~/.local/state/linexus`; delete those only when the person asks for it.
