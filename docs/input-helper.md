# Input helper

The `keyboard_shortcut` and `text_input` actions ("Press keys" and "Type text" in Studio) need the input helper `linexus-inputd`. All other actions work without it.

## Why a helper

To press keys in another application, a program has to act like a keyboard. On Wayland, applications cannot send key events to other windows; the compositor accepts input only from input devices. The helper therefore creates a virtual keyboard through the kernel's uinput interface (`/dev/uinput`). This works the same on Wayland and X11.

Opening `/dev/uinput` usually needs root. Instead of giving the daemon that right, a small separate program owns the virtual keyboard and accepts only two narrow requests over a local socket: press a key combination, or type a short text.

## How it works

- At start, the helper opens `/dev/uinput` and creates a virtual keyboard named "iCUE Nexus Linux Input Helper".
- It listens on a Unix socket:
  - `$XDG_RUNTIME_DIR/linexus/inputd.sock` when `XDG_RUNTIME_DIR` is set (user service),
  - `/run/linexus/inputd.sock` otherwise (system service),
  - or the path given with `--socket-path`.
- Only one user ID may use it (`--allow-uid`, default: `SUDO_UID` or the user running the helper). The socket has mode `0600`; a helper running as root hands it to that user.
- For every connection it checks the caller's user ID through the kernel (`SO_PEERCRED`) and refuses all others.
- Before it presses anything, it checks with `loginctl` whether your session is locked. It refuses while the session is locked, and also when it cannot determine the lock state.
- After each request it releases all modifier keys, so no Ctrl or Shift stays pressed.

The daemon and `linexusctl` use `$XDG_RUNTIME_DIR/linexus/inputd.sock` when that socket exists, and `/run/linexus/inputd.sock` otherwise.

| Option | Effect |
|---|---|
| `--socket-path <PATH>` | Socket path |
| `--allow-uid <UID>` | User ID allowed to send requests. Defaults to `SUDO_UID` or the current user ID. |
| `--dry-run` | Do not open `/dev/uinput`; check requests and report them only |
| `--once` | Exit after one accepted connection (for tests) |
| `--allow-locked` | Also press keys while the session appears locked |

## Choose how to run it

First check whether your user can already write to `/dev/uinput`:

```bash
ls -l /dev/uinput
test -r /dev/uinput -a -w /dev/uinput && echo "user service will work"
```

If `/dev/uinput` does not exist, load the module: `sudo modprobe uinput`.

### User service (no root)

Use this when your user can read and write `/dev/uinput`.

```bash
systemctl --user enable --now linexus-inputd.service
```

The unit runs `linexus-inputd --socket-path %t/linexus/inputd.sock`, where `%t` is your `$XDG_RUNTIME_DIR`. It only starts when `/dev/uinput` exists. `install-dev.sh` enables it automatically when `/dev/uinput` is writable for you.

### System service (root)

Use this when only root can open `/dev/uinput`. The helper then runs as root, listens on `/run/linexus/inputd.sock` and serves only your user ID.

With the Arch package, the system unit is a template; the instance name is your user ID:

```bash
sudo systemctl enable --now "linexus-inputd@$(id -u).service"
```

With `install-dev.sh` or `linexusctl doctor --fix`, the helper is installed to `/usr/local/lib/linexus/linexus-inputd` and the unit to `/etc/systemd/system/linexus-inputd.service`, already set to your user ID:

```bash
sudo systemctl enable --now linexus-inputd.service
```

To review that unit, or to write it yourself:

```bash
linexusctl print-input-helper-service --allow-uid "$(id -u)"
linexusctl print-input-helper-service --binary-path /usr/lib/linexus/linexus-inputd
```

The system units restrict the helper with systemd sandboxing: `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome=read-only`, `PrivateTmp`, `DevicePolicy=closed` with only `/dev/uinput` allowed, `RestrictAddressFamilies=AF_UNIX`, `LockPersonality` and `MemoryDenyWriteExecute`.

Run only one of the two services.

## Check that it works

```bash
linexusctl input-helper socket-path
linexusctl input-helper ping
```

`ping` prints `pong` when the helper answers. It does not press any key.

`linexusctl doctor` reports "input helper running (shortcut and text actions work)" or a hint what to start. In Studio, **Device > Setup checks** shows "The input helper is running." together with the socket in use.

To try the helper without touching `/dev/uinput`, run a dry-run instance on a separate socket in one terminal:

```bash
linexus-inputd --dry-run --allow-locked --socket-path "$XDG_RUNTIME_DIR/inputd-test.sock"
```

and talk to it from another:

```bash
linexusctl input-helper ping --socket-path "$XDG_RUNTIME_DIR/inputd-test.sock"
```

To send real key presses from the command line, use `linexusctl action keyboard-shortcut ctrl alt t` or `linexusctl action text-input "hello"`. They type into the window that has focus. Add `--dry-run` to only print what would be sent.

## Key names

`keyboard_shortcut` takes one to six key names, pressed together and released in reverse order. Names are not case sensitive, and `_` counts as `-`.

| Keys | Names |
|---|---|
| Modifiers | `ctrl` (`control`), `shift`, `alt`, `super` (`meta`, `win`), `rightctrl`, `rightshift`, `rightalt` (`altgr`), `rightmeta` |
| Editing | `enter` (`return`), `esc` (`escape`), `tab`, `space`, `backspace`, `delete` (`del`) |
| Navigation | `home`, `end`, `pageup`, `pagedown`, `left`, `right`, `up`, `down` |
| Function keys | `f1` to `f12` |
| Single characters | letters, digits and the punctuation of a US keyboard, for example `t`, `5`, `/` |

A single character names its key: `"A"` is the same key as `"a"`. Add `shift` for the shifted character.

`text_input` accepts up to 256 characters per request: printable ASCII, newline and tab. Other characters, such as umlauts, are refused. The helper sends key positions of a US keyboard, so the typed result follows your active keyboard layout.

## Request format

The protocol mirrors the daemon's: one JSON line per connection, one JSON line back.

```json
{"command":{"command":"key_combo","keys":["ctrl","alt","t"]}}
{"command":{"command":"text","text":"Hello!"}}
{"command":{"command":"ping"}}
```

```json
{"ok":true,"message":"sent key combo: ctrl+alt+t"}
{"ok":false,"message":"unsupported key 'hyper'"}
```

## Security notes

- The helper has no network listener and no shell. It accepts only key combinations of known keys and short texts, never raw input events.
- Any program running under the allowed user ID can make it press keys. Run it for your own desktop user only.
- The lock check fails closed: when the helper cannot tell whether your session is locked, it refuses. `--allow-locked` turns the check off; avoid it outside of tests.
- The root variant runs with the sandboxing listed above and talks to exactly one user ID.
