# Local API

The daemon `linexusd` accepts commands on a Unix socket. `linexusctl daemon` and Linexus Studio use it, and so can your own scripts. The API is local only; the daemon opens no network port.

## Socket

Default path:

```text
$XDG_RUNTIME_DIR/linexus/daemon.sock
```

When `XDG_RUNTIME_DIR` is not set, the daemon uses `linexus-<uid>/linexus/daemon.sock` below the system temporary directory, but only if that directory belongs to you and nobody else can write to it; otherwise it falls back to `~/.cache/linexus-runtime/linexus/daemon.sock`. `linexusctl daemon socket-path` prints the default for your session.

All three programs take `--socket-path` to use another path, for example to run a second daemon in emulator mode next to the real one:

```bash
linexusd --emulator-output /tmp/nexus-test/frame.png \
  --socket-path /tmp/nexus-test/daemon.sock --state-path /tmp/nexus-test/runtime.json
linexusctl daemon --socket-path /tmp/nexus-test/daemon.sock status
linexus-studio --port 19778 --socket-path /tmp/nexus-test/daemon.sock
```

Access control:

- The daemon creates the socket with mode `0600`, and its directory with mode `0700` when the directory does not exist yet.
- For every connection it reads the caller's user ID from the kernel (`SO_PEERCRED`). Callers running as another user get `{"ok":false,"message":"peer uid ... is not allowed"}`.
- At start, a leftover socket from an earlier run is replaced. Any other kind of file at that path makes the daemon refuse to start. The socket is removed when the daemon exits.

## Framing

One request per connection:

1. Connect.
2. Send one JSON object followed by a newline.
3. Read one JSON line back. The daemon then closes the connection.

The daemon reads the request within 2 seconds and answers between two frames, at most 3 seconds later; otherwise the reply is `{"ok":false,"message":"daemon did not answer in time"}`.

The request object has one field, `command`, which holds the command object. The command object names the command in its own `command` field:

```json
{"command":{"command":"go_to_page","page_id":"media"}}
```

## Commands

| Command | Fields | Effect | Reply carries |
|---|---|---|---|
| `ping` | none | Checks that the daemon answers | `message: "pong"` |
| `status` | none | Reports mode, device, profile, page and brightness | `status` |
| `tap` | `x` (0 to 639) | A short touch at `x`, as if a finger tapped the strip | `status` |
| `touch` | `phase` (`began`, `moved`, `ended`), `x` | One raw touch event. Studio's live mirror sends these, so swipes and holds behave as on the panel. | `message` only |
| `page_next` | none | Next page of the active profile | `status` |
| `page_previous` | none | Previous page | `status` |
| `go_to_page` | `page_id` | Opens a page of the active profile, hidden pages included | `status` |
| `switch_profile` | `profile_id` | Makes another profile active and saves the choice | `status` |
| `set_brightness` | `percent` (0 to 100) | Sets the brightness and saves it | `status` |
| `blank` | none | Turns the strip off | `status` |
| `restore` | none | Turns the strip back on | `status` |
| `reload_config` | `path` (optional) | Reads the config file again, or another file | `status` |
| `frame` | none | Returns the frame currently on the strip | `frame` |
| `notify` | `text`, `icon` (optional) | Shows a message over the current page for 5 seconds. `icon` is a Lucide icon name such as `bell` or `check`; unknown names show `bell`. | `message` only |
| `metrics` | none | Current value of every metric | `metrics` |

Examples:

```json
{"command":{"command":"status"}}
{"command":{"command":"tap","x":320}}
{"command":{"command":"touch","phase":"began","x":500}}
{"command":{"command":"set_brightness","percent":60}}
{"command":{"command":"reload_config","path":"/home/you/nexus-test.json"}}
{"command":{"command":"notify","text":"Build done","icon":"check"}}
```

`switch_profile` also ends an automatic profile switch, see [studio.md](studio.md#app-aware-profiles). `go_to_page` remembers where it came from, so a later `go_back` action returns there.

## Replies

| Field | Type | When present |
|---|---|---|
| `ok` | boolean | Always. `false` when the command failed. |
| `message` | string | Always. Short result, or the error. |
| `status` | object | After `status` and the commands marked above, when they succeed |
| `frame` | string | After `frame`: the current frame as a base64-encoded PNG, 640 x 48 pixels |
| `metrics` | array | After `metrics` |

`status`:

```json
{
  "mode": "hardware",
  "device": { "state": "connected", "path": "/dev/hidraw5" },
  "profile_id": "default",
  "profile_name": "Default",
  "page_id": "media",
  "page_name": "Media",
  "page_index": 1,
  "page_count": 5,
  "brightness_percent": 80,
  "blanked": false,
  "manual_blanked": false,
  "session_blanked": false
}
```

| Field | Meaning |
|---|---|
| `mode` | `hardware`, or `emulator` when the daemon runs with `--emulator-output` |
| `device.state` | `connected`, `unavailable` (with `error`) or `emulator` |
| `device.path` | HID device path while connected |
| `device.error` | Why the strip is unavailable |
| `profile_id`, `profile_name` | Active profile |
| `page_id`, `page_name` | Page on the strip |
| `page_index`, `page_count` | Position among the pages that are not hidden, counting from 0. On a hidden page, `page_index` is the position of the page it was opened from. |
| `brightness_percent` | Brightness set for the strip. Idle dimming lowers the panel brightness without changing this value. |
| `blanked` | The strip is off for any reason |
| `manual_blanked` | Turned off with `blank` |
| `session_blanked` | Turned off because the session is locked or the system goes to sleep |

`metrics` entries:

```json
{ "id": "cpu.temp", "label": "CPU temp", "value": 81.8, "unit": "celsius", "max": 100.0 }
```

`unit` is one of `percent`, `celsius`, `watts`, `rpm`, `bytes`, `bytes_per_sec`, `megahertz` or `plain`. `max` is the full scale used for bars and rings. The ids are listed in [configuration.md](configuration.md#metric-ids).

Errors come back with `ok: false`:

```json
{"ok":false,"message":"profile 'nope' not found"}
{"ok":false,"message":"invalid request: invalid type: string \"ping\", expected internally tagged enum Command at line 1 column 17"}
```

## linexusctl daemon

`linexusctl daemon` covers every command. Without `--json` it prints the message and a readable status; with `--json` it prints the whole reply. It exits with an error when the reply has `ok: false`.

| Subcommand | Command sent |
|---|---|
| `linexusctl daemon socket-path` | none; prints the default socket path |
| `linexusctl daemon ping` | `ping` |
| `linexusctl daemon status [--json]` | `status` |
| `linexusctl daemon tap <X> [--json]` | `tap` |
| `linexusctl daemon page-next [--json]` | `page_next` |
| `linexusctl daemon page-previous [--json]` | `page_previous` |
| `linexusctl daemon go-to-page <PAGE_ID> [--json]` | `go_to_page` |
| `linexusctl daemon switch-profile <PROFILE_ID> [--json]` | `switch_profile` |
| `linexusctl daemon brightness <PERCENT> [--json]` | `set_brightness` |
| `linexusctl daemon blank [--json]` | `blank` |
| `linexusctl daemon restore [--json]` | `restore` |
| `linexusctl daemon reload-config [--config FILE] [--json]` | `reload_config` |
| `linexusctl daemon frame --out FILE [--scale N]` | `frame`; saves a PNG, enlarged by N (1 to 16) |
| `linexusctl daemon push <TEXT>... [--icon NAME] [--json]` | `notify`; checks the icon name and suggests similar ones |
| `linexusctl daemon metrics [FILTER] [--json]` | `metrics`; shows a table, filtered by id or label |

```console
$ linexusctl daemon status
status
mode: Hardware
device: Connected
device path: /dev/hidraw5
profile: Default (default)
page: Overview (overview) [1/5]
brightness: 80%
blanked: false
manual blanked: false
session blanked: false
$ linexusctl daemon push Build done --icon check
shown
$ linexusctl daemon metrics cpu
ID                    LABEL           VALUE  UNIT
cpu.clock             CPU clock        4050  MHz
cpu.temp              CPU temp         81.8  °C
cpu.usage             CPU                22  %
hwmon:nct6798/cputin  nct6798 CPUTIN   54.5  °C
$ linexusctl daemon frame --out strip.png --scale 2
Frame written: strip.png (1280x96)
```

## Raw access

With `socat`:

```bash
printf '%s\n' '{"command":{"command":"go_to_page","page_id":"media"}}' \
  | socat - "UNIX-CONNECT:$XDG_RUNTIME_DIR/linexus/daemon.sock"
```

```json
{"ok":true,"message":"page changed","status":{"mode":"hardware","device":{"state":"connected","path":"/dev/hidraw5"},"profile_id":"default","profile_name":"Default","page_id":"media","page_name":"Media","page_index":1,"page_count":5,"brightness_percent":80,"blanked":false,"manual_blanked":false,"session_blanked":false}}
```

With Python, standard library only:

```python
import json
import os
import socket


def nexus_request(command, path=None):
    """Send one command to linexusd and return its JSON reply."""
    if path is None:
        path = os.path.join(os.environ["XDG_RUNTIME_DIR"], "linexus", "daemon.sock")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
        sock.settimeout(3)
        sock.connect(path)
        sock.sendall(json.dumps({"command": command}).encode() + b"\n")
        sock.shutdown(socket.SHUT_WR)
        data = b""
        while not data.endswith(b"\n"):
            chunk = sock.recv(65536)
            if not chunk:
                break
            data += chunk
    return json.loads(data)


reply = nexus_request({"command": "set_brightness", "percent": 60})
print(reply["ok"], reply["status"]["brightness_percent"])
```

## Event log

The daemon records what happens in a JSON Lines file:

```text
$XDG_STATE_HOME/linexus/events.jsonl     (default: ~/.local/state/linexus/events.jsonl)
```

Each line is one record:

```json
{"ts_ms":1792376515483,"level":"info","source":"linexusd","kind":"action_dry_run","message":"action executed","fields":{"command":"/usr/bin/konsole","widget":"terminal"}}
```

| Field | Meaning |
|---|---|
| `ts_ms` | Unix time in milliseconds |
| `level` | `debug`, `info`, `warn` or `error` |
| `source` | Program that wrote the record, `linexusd` |
| `kind` | Event type, see below |
| `message` | Short description |
| `fields` | Extra string values; absent when empty |

Kinds the daemon writes:

| Kind | Fields |
|---|---|
| `daemon_start`, `daemon_stop` | none |
| `ipc_socket_ready` | `path` |
| `config_upgraded` | `from_version`, `notes` |
| `config_reloaded` | `profile` |
| `hardware_connected` | `path` |
| `hardware_unavailable` | `error`; repeated at most every 10 minutes while the error stays the same |
| `hardware_lost` | `error` |
| `session_blank`, `session_restore` | none |
| `page_changed` | `page`, `cause` (`swipe`, `api`, `auto_return` or the id of the widget) |
| `profile_auto` | `profile`, `app` |
| `focus_watch` | `backend` (`kwin`, `hyprland`, `sway` or `x11`) |
| `action_executed`, `action_dry_run` | `widget`, `command` |
| `action_failed` | `widget`, `error` |
| `news_opened` | `source`, `title`, `url` |
| `news_dismissed` | `source`, `title` |

When the file reaches 8 MiB, it is renamed to `events.jsonl.1` (replacing an older one) and a new file starts.

```bash
linexusctl events path
linexusctl events list --lines 20
linexusctl events list --json
linexusctl events clear
```

`events clear` empties the log and removes `events.jsonl.1`. Studio shows the same records under **Device > Recent events**.

## Studio's HTTP API

Linexus Studio serves its own JSON endpoints under `http://127.0.0.1:19777/api/`. They exist for the Studio interface, can change between releases, and are not a stable public API. Use the daemon socket for scripts.

The one exception is the webhook endpoint, which is meant for other programs. See [studio.md](studio.md#incoming-webhooks).
