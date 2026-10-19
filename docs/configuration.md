# Configuration reference

Linexus Studio writes the configuration for you. This page describes the file format (schema version 2) for people who edit it by hand, generate it with scripts, or want to know what a `.nexuspack` contains.

## Files

| Path | Content | Edited in |
|---|---|---|
| `~/.config/linexus/config.json` | Profiles, pages, widgets, actions and device settings. The format on this page. | Studio: Pages, Device, Connections (weather, notifications) |
| `~/.config/linexus/sensors.json` | News feeds, headline movement, incoming webhook rules, the Home Assistant connection, MQTT and IFTTT. | Studio: News, Connections |
| `~/.local/state/linexus/runtime.json` | Last active profile, page and brightness, restored when the daemon starts | Written by the daemon |
| `~/.local/state/linexus/events.jsonl` | Event log, see [local-api.md](local-api.md#event-log) | Written by the daemon |
| `~/.local/state/linexus/rss-cache/`, `rss-seen.json`, `webhooks/` | Feed caches, read and hidden headlines, received webhooks | Written by the daemon and Studio |

`~/.config` stands for `$XDG_CONFIG_HOME` when that variable is set and not empty, and `~/.local/state` for `$XDG_STATE_HOME` when it is set. `linexusctl config path` prints the config path in use.

When `config.json` does not exist, the daemon shows the built-in default pages without writing a file. The daemon, Studio and `linexusctl` accept `--config <file>` to use another file.

This page covers `config.json` only. `sensors.json` is managed by Studio and is not part of `.nexuspack` files.

## Working with the file

| Command | Effect |
|---|---|
| `linexusctl config path` | Print the default config path |
| `linexusctl config init [--path FILE] [--force]` | Write the built-in default config; `--force` replaces an existing file |
| `linexusctl config validate [--path FILE]` | Check a file and summarize it |
| `linexusctl config show [--path FILE]` | Print the config as formatted JSON; older schemas are shown upgraded |
| `linexusctl config upgrade [--path FILE] [--dry-run]` | Rewrite an older schema in the current one and keep a backup |
| `linexusctl config hit-test --page PAGE --x X [--profile PROFILE] [--path FILE]` | Show which widget and actions sit at position X |
| `linexusctl daemon reload-config` | Make the running daemon read the file again |

```console
$ linexusctl config validate
Config OK: /home/you/.config/linexus/config.json
  1 profile(s), 5 page(s), 17 widget(s); active profile 'Default' (default)
$ linexusctl config hit-test --page launcher --x 120
x=120 on page 'launcher' (profile 'default'): widget 'files' (button, x 107..214)
  tap: {"type":"launch_app","target":"default:files"}
  double tap: -
  hold: -
```

Every program writes the file atomically (temporary file, then rename) and only after it passes validation. Studio saves on every change and asks the daemon to reload. After editing the file by hand, run `linexusctl config validate` and then `linexusctl daemon reload-config`. When the file is invalid, the reload fails and the daemon keeps showing the previous config; at start-up, an invalid file stops the daemon with an error.

## Top level

```json
{
  "schema_version": 2,
  "active_profile": "default",
  "settings": { },
  "profiles": [ ]
}
```

| Field | Type | Required | Meaning |
|---|---|---|---|
| `schema_version` | number | yes | Must be `2`. Version 1 files are upgraded, see [Upgrading from schema 1](#upgrading-from-schema-1). |
| `active_profile` | string | yes | Id of the profile shown on the strip |
| `settings` | object | no | Device-wide settings; every field has a default |
| `profiles` | array | yes | The profiles |

## Settings

All fields are optional.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `locale` | `"auto"`, `"en"`, `"de"` | `"auto"` | Language of dates and the few words on the strip. `auto` follows `LC_ALL`, `LC_MESSAGES` or `LANG` of the daemon; German when it starts with `de`, otherwise English. |
| `hour12` | boolean | `false` | 12-hour clock |
| `temperature_unit` | `"celsius"`, `"fahrenheit"` | `"celsius"` | Unit for every temperature on the strip: readings, text tokens and weather |
| `transition` | `"slide"`, `"fade"`, `"none"` | `"slide"` | Animation when the page changes |
| `idle` | object | see below | Dimming and turning off when nobody touches the strip |
| `weather` | location or `null` | `null` | Default place for weather widgets: `{"name": "Berlin", "latitude": 52.52, "longitude": 13.405}` |
| `notifications` | object | see below | Desktop notifications on the strip |
| `overlays` | object | see below | Short pop-ups for volume and track changes |

`idle`:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `dim_after_minutes` | number | `0` | Dim after this many minutes without a touch; `0` never dims |
| `dim_percent` | number | `30` | Brightness while dimmed, as a percentage of the profile brightness |
| `wake_touch_only` | boolean | `true` | While dimmed, the first touch only wakes the strip and does not trigger the widget under the finger |
| `blank_when_locked` | boolean | `true` | Turn the strip off while the session is locked or the system goes to sleep |
| `off_after_minutes` | number | `0` | Turn the strip off after this many minutes without a touch; `0` keeps it on. The next touch, or unlocking the session, turns it back on. |

Lock detection asks logind for the `LockedHint` of the session in the daemon's `XDG_SESSION_ID` environment variable; sleep is detected through logind's `PreparingForSleep`. The daemon flag `--disable-session-blank` turns this off regardless of the setting.

`notifications`:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `enabled` | boolean | `false` | Show desktop notifications on the strip. Needs `dbus-monitor`. |
| `duration_ms` | number | `6000` | How long a notification stays; at least 1500 ms are used |
| `muted_apps` | array of strings | `[]` | Application names that never show; compared without regard to case |
| `private` | boolean | `false` | Show only the application name, not the text |

`overlays`:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `volume` | boolean | `true` | Show a volume bar when the system volume changes, unless the page already has a volume widget |
| `now_playing` | boolean | `true` | Show the new track when the playing track changes, unless the page already has a media widget |

## Profiles

A profile is a set of pages with its own look and brightness.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `id` | string | yes | Unique id |
| `name` | string | yes | Display name |
| `brightness` | number | yes | 0 to 100. Applied when the profile becomes active. |
| `theme` | object | no | `{"preset": "midnight"}` by default |
| `pages` | array | yes | At least one page must not be hidden |
| `apps` | array of strings | no | Applications that switch to this profile while they have focus |

`theme`:

| Field | Type | Meaning |
|---|---|---|
| `preset` | string | `midnight` (default), `aurora`, `ember`, `neon`, `mono` or `daylight`. Unknown names fall back to `midnight`. |
| `accent` | `"#rrggbb"` | Replaces the preset's accent colour. The short form `"#rgb"` is accepted too. |

Switching to a profile sets the brightness to the profile's value and shows its first page that is not hidden.

`apps` entries are compared without regard to case against the focused window's application id (desktop file name or Wayland app id) and its window class. A trailing `.desktop` is ignored, and the last dot-separated part counts on its own, so `dolphin` matches `org.kde.dolphin`. The first profile whose `apps` match wins. When focus moves to an application that no profile lists, the daemon returns to the profile that was active before. See [studio.md](studio.md#app-aware-profiles) for the supported desktops.

## Pages

| Field | Type | Required | Meaning |
|---|---|---|---|
| `id` | string | yes | Unique within the profile |
| `name` | string | yes | Display name |
| `hidden` | boolean | no | Hidden pages are skipped by swipes and opened only with a `go_to_page` action, like folders |
| `auto_return_seconds` | number | no | For hidden pages: go back to the page it was opened from after this many seconds without a touch |
| `background` | object | no | Page background; the theme gradient when absent |
| `widgets` | array | yes | The widgets, see below |

Swipes and the `next_page` and `previous_page` actions cycle through the pages that are not hidden, in list order, and wrap around at the ends.

`background`:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `image` | data URL | none | `data:image/...;base64,...`, at most 3 MiB |
| `color` | `"#rrggbb"` | none | Solid colour, drawn under the image |
| `fit` | `"fill"`, `"fit"`, `"stretch"` | `"fill"` | How the image covers the strip: crop to fill, fit inside, or stretch |
| `dim_percent` | number | `0` | Darkens the page so widgets stay readable, 0 to 90 |

## Widgets

A page is one row of widgets along the 640-pixel strip. The touch panel reports only the horizontal position, so every widget owns its full height and widgets never overlap.

A widget is a flat object: the common fields below plus the fields of its `type`.

```json
{ "id": "tools", "x": 540, "w": 100, "type": "button", "label": "Tools", "icon": "lucide:wrench",
  "tap": { "type": "go_to_page", "page_id": "tools" } }
```

| Field | Type | Required | Meaning |
|---|---|---|---|
| `id` | string | yes | Unique on the page |
| `x` | number | yes | Left edge in pixels, 0 to 639 |
| `w` | number | yes | Width in pixels, at least 24; `x + w` must not exceed 640 |
| `type` | string | yes | One of the types below |
| `accent` | `"#rrggbb"` | no | Colour for this widget instead of the theme accent |
| `tap` | action | no | Runs on a short touch |
| `double_tap` | action | no | Runs on two taps within 300 ms. A widget with a double-tap meaning delays its single tap by that window. |
| `hold` | action | no | Runs when the finger rests on the widget |
| `repeat` | boolean | no | While the finger rests, repeat the `hold` action (or the `tap` action when there is no `hold`) every 160 ms, like a held key |

Several widget types have built-in touch behaviour that applies while the matching gesture has no action. Setting an action replaces it.

### clock

| Field | Default | Meaning |
|---|---|---|
| `show_date` | `true` | Show the date below the time |
| `show_seconds` | `false` | Show seconds |
| `hour12` | not set | Overrides `settings.hour12` for this clock |
| `label` | not set | Small caption, for example a city name |
| `timezone` | not set | Accepted, but not applied in 1.0.0; clocks show local time |

### metric

One reading as a number, bar, graph or ring.

| Field | Default | Meaning |
|---|---|---|
| `source` | `"cpu.usage"` | Metric id, see [Metric ids](#metric-ids) |
| `style` | `"sparkline"` | `number`, `bar`, `sparkline` (graph of the last minute) or `ring` |
| `label` | not set | Caption instead of the metric's own label |
| `secondary` | not set | Second, smaller reading in the corner, for example `cpu.temp` |
| `warn` | not set | Value (in the metric's unit) from which the reading turns amber |
| `critical` | not set | Value from which the reading turns red |

### network

Download and upload speed (`net.down` and `net.up`).

| Field | Default | Meaning |
|---|---|---|
| `interface` | not set | Accepted, but not applied in 1.0.0; the widget shows the sum of all interfaces except loopback |

### media

Album art, title and playback buttons for the MPRIS player the desktop reports.

| Field | Default | Meaning |
|---|---|---|
| `show_art` | `true` | Album art |
| `show_progress` | `true` | Progress bar |
| `show_controls` | `true` | Previous, play/pause and next buttons |
| `tint` | `true` | Tint the background with the album art's main colour |
| `player` | not set | Player for the buttons when no player reports playback. Use the full bus name, for example `org.mpris.MediaPlayer2.spotify`. |

Built-in tap: the buttons skip back, play or pause, and skip forward; tapping the title plays or pauses.

### volume

| Field | Default | Meaning |
|---|---|---|
| `show_value` | `true` | Show the percentage |
| `step` | `5` | Accepted, but not used in 1.0.0 |

Built-in touch: slide along the bar to set the volume; tap the speaker icon to mute or unmute. Uses `wpctl`.

### button

| Field | Default | Meaning |
|---|---|---|
| `label` | `""` | Text |
| `icon` | not set | `lucide:<name>` (Lucide icon), `app:<desktop id>` (the application's icon, for example `app:org.kde.dolphin.desktop`) or a `data:image/...` URL |
| `style` | `"tile"` | `tile` (icon above the label), `inline` (icon and label side by side) or `icon` (icon only) |
| `show_label` | `true` | Show the label |
| `running_indicator` | `true` | For buttons whose tap action is `launch_app`: show a dot while that application runs |

### news

Two rows of headlines from the feeds and webhooks configured in Studio.

| Field | Default | Meaning |
|---|---|---|
| `font` | `"pixel"` | `pixel` (crisp bitmap font) or `smooth` |
| `feeds` | `[]` | Feed ids. Accepted, but not applied in 1.0.0; the widget shows all enabled sources. |

Built-in touch: tap the left half to open the top headline, the right half for the bottom one; double tap left or right to step back or ahead a pair; hold to hide a headline.

### weather

Current conditions from Open-Meteo.

| Field | Default | Meaning |
|---|---|---|
| `location` | not set | `{"name", "latitude", "longitude"}`; `settings.weather` when not set |
| `show_range` | `false` | Show today's high and low |

### entity

A Home Assistant light or switch. Needs the Home Assistant connection set up in Studio under Connections.

| Field | Default | Meaning |
|---|---|---|
| `entity_id` | `""` | For example `light.desk` |
| `label` | not set | Caption |
| `icon` | not set | Lucide icon, for example `lucide:lamp-desk`; an icon matching the entity's domain when not set |

Built-in tap: toggles the entity.

### timer

| Field | Default | Meaning |
|---|---|---|
| `mode` | `"pomodoro"` | `pomodoro`, `countdown` or `stopwatch` |
| `minutes` | `25` | Countdown length, or the focus phase of a pomodoro |
| `break_minutes` | `5` | Pomodoro break |
| `long_break_minutes` | `15` | Pomodoro long break |
| `rounds` | `4` | Focus phases before a long break |
| `label` | not set | Caption |

Built-in touch: tap starts or pauses, double tap skips to the next phase, hold resets.

### text

| Field | Default | Meaning |
|---|---|---|
| `text` | `"Hello"` | The text. `{metric id}` tokens are replaced with live values, for example `CPU {cpu.temp}`. |
| `size` | `"medium"` | `small`, `medium` or `large` |
| `align` | `"center"` | `left`, `center` or `right` |
| `caption` | not set | Small caption above the text |

### image

| Field | Default | Meaning |
|---|---|---|
| `src` | `""` | `data:image/...;base64,...`, at most 3 MiB |
| `fit` | `"fill"` | `fill`, `fit` or `stretch` |

### command

Shows the first line a program prints, refreshed on an interval. Runs only when the daemon is started with `--allow-shell-actions`.

| Field | Default | Meaning |
|---|---|---|
| `program` | `""` | Program name on `PATH`, without `/`. No shell is involved. |
| `args` | `[]` | Arguments |
| `interval_seconds` | `30` | Refresh interval; at least 2 seconds are used |
| `label` | not set | Caption |

### spacer

Empty space. No fields of its own. A spacer can still carry actions.

## Actions

An action is an object with a `type`. It goes into a widget's `tap`, `double_tap` or `hold`.

Page and profile actions, `brightness`, `display_off` and `sequence` are handled by the daemon itself. Everything else, Home Assistant calls included, runs one after another on a background worker, so a slow action never stalls the strip. When an action fails, the strip shows a short message and the event log records `action_failed`.

### open_url

Opens an `http://` or `https://` address. When the default browser is a known Chromium or Firefox variant, the address opens in a new window of it; otherwise through `xdg-open` (or `kioclient`).

```json
{ "type": "open_url", "url": "https://example.com" }
```

### open_path

Opens a file or folder with its default application. The path must be absolute or start with `~/`.

```json
{ "type": "open_path", "path": "~/Downloads" }
```

### launch_app

`target` is one of:

- a desktop file id ending in `.desktop`, started with `gtk-launch` (or `xdg-open`)
- the name of a program on `PATH`
- `default:browser`, `default:files`, `default:terminal` or `default:mail` for the desktop's default application

Paths (anything containing `/`) are refused.

```json
{ "type": "launch_app", "target": "org.kde.dolphin.desktop" }
```

### media

Sends an MPRIS command with `busctl`. `command` is `play_pause`, `next`, `previous` or `stop`. `player` is optional and must be a full bus name starting with `org.mpris.MediaPlayer2.`; without it the first player found is used. The older type name `media_key` is accepted.

```json
{ "type": "media", "command": "play_pause", "player": "org.mpris.MediaPlayer2.spotify" }
```

### volume

Changes the default output with `wpctl`. `command` is `up`, `down`, `set` or `toggle_mute`. `step` (default 5) applies to `up` and `down`; `percent` (0 to 100) is required for `set`. Volume never goes above 100 %.

```json
{ "type": "volume", "command": "up", "step": 5 }
```

```json
{ "type": "volume", "command": "set", "percent": 40 }
```

### microphone_mute

Toggles the mute state of the default microphone with `wpctl`.

```json
{ "type": "microphone_mute" }
```

### keyboard_shortcut

Presses up to six keys together through the input helper. See [input-helper.md](input-helper.md#key-names) for key names.

```json
{ "type": "keyboard_shortcut", "keys": ["ctrl", "alt", "t"] }
```

### text_input

Types text through the input helper: up to 256 characters per action. Printable ASCII, newline and tab are supported; other characters make the action fail. The keys are sent as positions on a US keyboard, so the result follows your active keyboard layout.

```json
{ "type": "text_input", "text": "Hello!" }
```

### shell_command

Runs a program on `PATH` directly, without a shell, and waits up to 10 seconds. `program` must not contain `/`. This action only runs when the daemon is started with `--allow-shell-actions`; otherwise the strip shows "shell actions are off".

```json
{ "type": "shell_command", "program": "notify-send", "args": ["Nexus", "Button pressed"] }
```

To allow it for the user service, add the flag with an override:

```bash
systemctl --user edit linexusd.service
```

```ini
[Service]
ExecStart=
ExecStart=/usr/bin/linexusd --allow-shell-actions
```

That path fits the Arch package. For `install-dev.sh`, use `ExecStart=%h/.local/bin/linexusd --allow-shell-actions` (`%h` is your home directory). Then run `systemctl --user restart linexusd.service`. The same flag enables command widgets.

### http_request

Sends a request with `curl` and discards the response. `method` is `GET`, `POST` or `PUT`; the URL must start with `http://` or `https://`. With a `body`, `content_type` defaults to `application/json`. The request fails on HTTP errors and after 10 seconds.

```json
{
  "type": "http_request",
  "method": "POST",
  "url": "https://hooks.example.com/nexus",
  "body": "{\"event\":\"pressed\"}",
  "content_type": "application/json"
}
```

### home_assistant

Calls a Home Assistant service through the connection saved in Studio. `service` is `domain.service`; without a dot, the domain `homeassistant` is used. `entity_id` is optional.

```json
{ "type": "home_assistant", "service": "light.toggle", "entity_id": "light.desk" }
```

### next_page, previous_page

Same as swiping. The older names `screen_next` and `screen_previous` are accepted.

```json
{ "type": "next_page" }
```

### go_to_page

Opens a page of the same profile by id, hidden pages included. The page must exist.

```json
{ "type": "go_to_page", "page_id": "tools" }
```

### go_back

Returns to the page that a `go_to_page` action came from, or to the first page that is not hidden.

```json
{ "type": "go_back" }
```

### switch_profile

Switches to another profile. The profile must exist. The older name `profile_switch` is accepted.

```json
{ "type": "switch_profile", "profile_id": "game" }
```

### next_profile, previous_profile

Cycle through all profiles in list order and wrap around.

```json
{ "type": "next_profile" }
```

### brightness

`command` is `up` (+10), `down` (-10, not below 5), `set` (needs `percent`) or `cycle` (25, 50, 75, 100, then 25 again). The new brightness is kept across daemon restarts.

```json
{ "type": "brightness", "command": "cycle" }
```

```json
{ "type": "brightness", "command": "set", "percent": 50 }
```

### display_off

Turns the strip off until the next touch. That touch only wakes the strip.

```json
{ "type": "display_off" }
```

### lock_screen

Locks the session with `loginctl lock-session`.

```json
{ "type": "lock_screen" }
```

### sequence

Runs `steps` in order. The first step runs at once, each further step `delay_ms` after the previous one (default 0, at most 60 000). A sequence has at most 16 steps and may contain one further level of sequences. Steps still waiting are dropped when the config is reloaded or the profile changes.

```json
{
  "type": "sequence",
  "delay_ms": 300,
  "steps": [
    { "type": "launch_app", "target": "default:terminal" },
    { "type": "keyboard_shortcut", "keys": ["super", "up"] }
  ]
}
```

## Metric ids

`metric` widgets, the `secondary` field and `{tokens}` in text widgets refer to readings by id. Readings this machine cannot provide (no GPU, no PipeWire) are left out. To list the ids available on your machine:

```bash
linexusctl daemon metrics
linexusctl daemon metrics temp
```

| Id | Unit | Reading |
|---|---|---|
| `cpu.usage` | % | CPU load |
| `cpu.temp` | °C | CPU temperature |
| `cpu.clock` | MHz | Average core clock |
| `gpu.usage` | % | GPU load |
| `gpu.temp` | °C | GPU temperature |
| `gpu.mem_temp` | °C | VRAM temperature |
| `gpu.power` | W | GPU power |
| `gpu.fan` | rpm | GPU fan |
| `gpu.vram` | % | VRAM in use |
| `ram.usage` | % | Memory in use |
| `ram.used` | bytes | Memory in use |
| `ram.total` | bytes | Installed memory |
| `disk.usage` | % | Root file system in use |
| `disk.read` | bytes/s | Disk reads |
| `disk.write` | bytes/s | Disk writes |
| `net.down` | bytes/s | Received, all interfaces except loopback |
| `net.up` | bytes/s | Sent, all interfaces except loopback |
| `volume.level` | % | Output volume |
| `hwmon:<chip>/<label>` | °C, rpm, W or V | One per hardware sensor input |
| `battery:<kind>/<key>` | % | One per battery |

`hwmon:<chip>/<label>`: `<chip>` is the hwmon driver name, `<label>` the driver's label for the input or the input name (`temp1`, `fan2`). Both are lower-cased, and characters other than letters, digits, `-` and `.` become `_`. When two inputs end up with the same id, the later ones get `-2`, `-3` and so on. Examples: `hwmon:k10temp/tctl`, `hwmon:amdgpu/edge`, `hwmon:nct6798/fan2`.

`battery:<kind>/<key>`: read from `upower --dump` when UPower is installed, otherwise from `/sys/class/power_supply`. `<kind>` is what the battery powers (`battery` for the computer itself; with UPower also `mouse`, `keyboard`, `headset`, `gaming-input` and others; without UPower `device` for peripherals). `<key>` is the device's name in the kernel, for example `battery:mouse/hidpp_battery_0`.

## Validation rules

A config is refused when any of these fails:

- `schema_version` is `2`.
- Profile, page and widget ids are not empty.
- Profile ids are unique and `active_profile` names one of them.
- Every profile has a brightness of at most 100 and at least one page that is not hidden.
- Page ids are unique within their profile; widget ids are unique within their page.
- Every widget is at least 24 pixels wide and ends within the 640-pixel strip.
- No two widgets on a page overlap.
- `switch_profile` names an existing profile; `go_to_page` names a page of the same profile.
- `volume` and `brightness` actions with `"command": "set"` have a `percent` of at most 100.
- `idle.dim_percent` is at most 100, and hidden pages with `auto_return_seconds` wait at least 1 second.
- `shell_command` actions and `command` widgets name a program without `/`.
- `http_request` URLs start with `http://` or `https://`, and `content_type` contains no line break.
- Sequences nest at most one level deep, have at most 16 steps and wait at most 60 000 ms between steps.
- Background `dim_percent` is at most 90.
- Embedded images (backgrounds, image widgets, button icons given as data URLs) start with `data:image/`, are base64 encoded and are at most 3 MiB long.

Studio checks the same rules while you edit and shows what blocks saving.

## Upgrading from schema 1

Configs written by versions before 1.0.0 use schema 1. They are upgraded when:

- the daemon starts or reloads,
- you run `linexusctl config upgrade` (add `--dry-run` to preview),
- you run `linexusctl doctor --fix`, or
- `install-dev.sh` runs.

The original is kept next to the file as `config.json.v1.bak`. An existing backup is never overwritten. Studio shows an old file upgraded but does not write a backup itself.

What changes:

- Fixed page kinds become pages built from widgets: `status` becomes the Overview template, `performance` Performance, `sensors` News and `media` Media.
- Tappable elements become buttons with the same action. On pages that become a template, they are placed into free space next to the template's widgets. Buttons that only switched pages are dropped, since swiping works on every page. Buttons that overlapped a neighbour or found no free space are dropped.
- Themes map to presets: `light` to `daylight`, `cyberpunk` to `neon`, `aurora` stays, everything else becomes `midnight`.
- Background images keep their fit and darkening.
- Buttons that pointed at a missing profile lose their action.

The upgrade prints a note for everything it dropped.

## .nexuspack files

A `.nexuspack` is a backup or a shareable set of profiles. It is a zip archive with two entries:

| Entry | Content |
|---|---|
| `manifest.json` | Summary of the pack, at most 1 MiB |
| `config.json` | The complete `config.json`, at most 64 MiB |

Images are embedded in the config as data URLs, so there are no other files. A pack holds `config.json` only: feeds, the Home Assistant token and other entries of `sensors.json` are not included. URLs and request bodies in your actions are included as written.

`manifest.json`:

```json
{
  "format_version": 2,
  "created_by": "linexusctl",
  "created_at_unix_ms": 1792376421693,
  "config_schema_version": 2,
  "active_profile": "default",
  "profile_count": 1,
  "profiles": [
    { "id": "default", "name": "Default", "page_count": 5 }
  ],
  "embedded_images": 0
}
```

| Field | Meaning |
|---|---|
| `format_version` | `2`. Packs with format 1 still import. |
| `created_by` | `linexusctl` or `linexus-studio` |
| `created_at_unix_ms` | Creation time |
| `config_schema_version` | Schema of the stored config, 1 or 2. Schema 1 configs are upgraded on import. |
| `active_profile`, `profile_count`, `profiles` | Must match the stored config |
| `embedded_images` | Number of `data:image/` URLs in the config; must match for format 2 |

Import and inspect check the manifest against the config and validate the config before anything is written.

```bash
linexusctl pack export --out backup.nexuspack
linexusctl pack inspect backup.nexuspack
linexusctl pack import backup.nexuspack --dry-run
linexusctl pack import backup.nexuspack --force
linexusctl daemon reload-config
```

A pack is a configuration someone else may have written, and buttons can run programs, type text, send web requests or open files. `pack inspect` and `pack import` therefore list every button that acts outside the strip, with where it sits and what it does, and `pack import` asks before writing (`--yes` skips the question). It refuses to replace an existing config without `--force`, keeps the replaced file as `config.json.bak`, and does not notify the daemon; reload it afterwards. Packs whose config is larger than 15 MiB are refused, since Studio could not save them again.

In Studio, **Settings > Backup > Import backup** shows the same list in a dialog and only replaces the configuration after you confirm. The previous file is kept as `config.json.bak` and the daemon reloads.

## Example

One profile with a main page and a hidden "Tools" page that a button opens. It returns to the main page after 30 seconds without a touch.

```json
{
  "schema_version": 2,
  "active_profile": "desk",
  "settings": {
    "locale": "en",
    "transition": "slide",
    "idle": { "dim_after_minutes": 10, "dim_percent": 30, "off_after_minutes": 60 },
    "weather": { "name": "Berlin", "latitude": 52.52, "longitude": 13.405 }
  },
  "profiles": [
    {
      "id": "desk",
      "name": "Desk",
      "brightness": 80,
      "theme": { "preset": "midnight", "accent": "#5aa9ff" },
      "pages": [
        {
          "id": "main",
          "name": "Main",
          "widgets": [
            { "id": "clock", "x": 0, "w": 120, "type": "clock", "show_date": true },
            {
              "id": "cpu", "x": 120, "w": 140, "type": "metric",
              "source": "cpu.usage", "secondary": "cpu.temp", "style": "sparkline",
              "warn": 70, "critical": 90
            },
            { "id": "weather", "x": 260, "w": 160, "type": "weather", "show_range": true },
            { "id": "volume", "x": 420, "w": 120, "type": "volume" },
            {
              "id": "tools", "x": 540, "w": 100, "type": "button",
              "label": "Tools", "icon": "lucide:wrench",
              "tap": { "type": "go_to_page", "page_id": "tools" }
            }
          ]
        },
        {
          "id": "tools",
          "name": "Tools",
          "hidden": true,
          "auto_return_seconds": 30,
          "widgets": [
            {
              "id": "terminal", "x": 0, "w": 160, "type": "button",
              "label": "Terminal", "icon": "lucide:square-terminal",
              "tap": { "type": "launch_app", "target": "default:terminal" }
            },
            {
              "id": "mute", "x": 160, "w": 160, "type": "button",
              "label": "Mic", "icon": "lucide:mic-off",
              "tap": { "type": "microphone_mute" }
            },
            {
              "id": "lock", "x": 320, "w": 160, "type": "button",
              "label": "Lock", "icon": "lucide:lock",
              "tap": { "type": "lock_screen" },
              "hold": { "type": "display_off" }
            },
            {
              "id": "back", "x": 480, "w": 160, "type": "button",
              "label": "Back", "icon": "lucide:arrow-left",
              "tap": { "type": "go_back" }
            }
          ]
        }
      ]
    }
  ]
}
```

Check it and render the main page without the strip:

```bash
linexusctl config validate --path example.json
linexusctl render-preview --config example.json --page main --scale 2 --out main.png
```
