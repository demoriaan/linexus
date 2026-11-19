# Configuration reference

Linexus Studio writes the configuration for you. This page describes the file format (schema version 2) for people who edit it by hand, generate it with scripts, or want to know what a `.nexuspack` contains.

## Files

| Path | Content | Edited in |
|---|---|---|
| `~/.config/linexus/config.json` | Profiles, pages, widgets, actions and device settings. The format on this page. | Studio: Pages, Device, Connections (weather, notifications) |
| `~/.config/linexus/sensors.json` | News feeds, headline movement, incoming webhook rules, the Home Assistant and OBS Studio connections, MQTT and IFTTT. | Studio: News, Connections |
| `~/.local/state/linexus/runtime.json` | Last active profile, page and brightness, restored when the daemon starts | Written by the daemon |
| `~/.local/state/linexus/events.jsonl` | Event log, see [local-api.md](local-api.md#event-log) | Written by the daemon |
| `~/.local/state/linexus/rss-cache/`, `rss-seen.json`, `webhooks/` | Feed caches, read and hidden headlines, received webhooks | Written by the daemon and Studio |
| `~/.local/state/linexus/alert-seen.json` | Breaking news already seen, so a restart shows nothing twice | Written by the daemon |

`~/.config` stands for `$XDG_CONFIG_HOME` when that variable is set and not empty, and `~/.local/state` for `$XDG_STATE_HOME` when it is set. `linexusctl config path` prints the config path in use.

When `config.json` does not exist, the daemon shows the built-in default pages without writing a file. The daemon, Studio and `linexusctl` accept `--config <file>` to use another file.

This page covers `config.json`, apart from the feed fields for [breaking news](#news-feeds-and-breaking-news). `sensors.json` is managed by Studio and is not part of `.nexuspack` files.

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
| `locale` | `"auto"`, `"en"`, `"de"`, `"fr"` | `"auto"` | Language of dates and the few words on the strip. `auto` follows `LC_ALL`, `LC_MESSAGES` or `LANG` of the daemon; German when it starts with `de`, French when it starts with `fr`, otherwise English. |
| `hour12` | boolean | `false` | 12-hour clock |
| `temperature_unit` | `"celsius"`, `"fahrenheit"` | `"celsius"` | Unit for every temperature on the strip: readings, text tokens and weather |
| `transition` | `"slide"`, `"fade"`, `"none"` | `"slide"` | Animation when the page changes |
| `idle` | object | see below | Dimming and turning off when nobody touches the strip |
| `weather` | location or `null` | `null` | Default place for weather widgets: `{"name": "Berlin", "latitude": 52.52, "longitude": 13.405}` |
| `notifications` | object | see below | Desktop notifications on the strip |
| `overlays` | object | see below | Short pop-ups for volume and track changes, and breaking-news banners |
| `discord` | object | see below | Discord's notifications and the microphone badge |
| `audio` | object | see below | Names, order and visibility of sound outputs and microphones in the sound widgets |
| `touch` | object | see below | Correction for a touch panel that reads touches beside the finger |
| `ping` | object | see below | Default host for [`ping`](#ping) widgets and the `net.ping` readings |

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
| `enabled` | boolean | `false` | Show desktop notifications on the strip. Needs `busctl` (systemd). |
| `duration_ms` | number | `6000` | How long a notification stays at most; shorter when the app asks for less, and at least 1500 ms |
| `muted_apps` | array of strings | `[]` | Applications that never show: the name the app sends, its menu name or its desktop file id; compared without regard to case |
| `private` | boolean | `false` | Show only the application's name and icon, not the text or the notification's picture |

The strip shows a notification when the desktop would pop it up too, following the desktop's own notification settings:

- **GNOME:** Do Not Disturb and an app's "pop-up banners" switch let only critical notifications through, an app with notifications turned off shows none, and low-urgency notifications never appear.
- **KDE Plasma:** Do Not Disturb lets through critical notifications (unless Plasma is set otherwise) and the apps allowed during it. An app whose popups are turned off shows none, and low-priority notifications appear only when Plasma shows them as popups.
- **Other desktops:** Do Not Disturb counts where the notification server reports it.

Notification text is cut to 1024 characters. A picture sent inline with more than 256 KiB of pixel data (about 256 x 256 pixels) is left out; the notification still shows.

`overlays`:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `volume` | boolean | `true` | Show a volume bar when the system volume changes, unless the page already has a volume widget |
| `privacy` | boolean | `true` | "Who is listening?": while another app records from a microphone or has a camera open, a red pill at the right edge of every page names the app with a microphone or camera icon. Five seconds after the apps last changed it shrinks to its icons, out of the way of sliders below it, and a change of apps names them again. A tap on the pill, named or shrunk, lists every app with its device ("Microphone, A50 Mic", "Camera, Droidcam") and a Close button; the list closes by itself after 20 seconds. See [How the privacy badge decides](#how-the-privacy-badge-decides). |
| `now_playing` | boolean | `true` | Show the new track when the playing track changes, unless the page already has a media widget |
| `alerts` | boolean | `true` | Show breaking news from alert feeds as a banner across the strip, see [News feeds and breaking news](#news-feeds-and-breaking-news). Off, breaking news only heads the news widget. |
| `alert_duration_ms` | number | `20000` | How long a banner stays, 5000 to 120 000 ms. A headline too long for the strip scrolls to its end first, which can make it stay up to 60 seconds. |

`touch`:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `left` | number | `0` | Pixels added to touches at the left edge of the strip; positive moves them to the right |
| `right` | number | `0` | The same at the right edge; in between the correction changes in proportion |

Both stay within ±100. The daemon corrects the strip's own touches only; taps from Studio's mirror and the emulator are exact.

Studio → Device → Calibrate measures it: the strip shows five marks across its width, twice, one after the other, and you touch the center of each. A touch more than 70 px from its mark does not count and the mark stays. A panel reading further off than that cannot be calibrated; set the values by hand (up to ±100). A line fitted through the ten touches gives both values, which Studio applies right away and offers to undo; values under 10 px at both edges are within normal finger accuracy and count as no correction. One touch more than 20 px off the fitted line is left out; a calibration fails when more of them do, when the touches stretch the strip by more than 25 %, or when the marks are missed for 30 seconds; with no touch for 30 seconds it is cancelled. By hand, `linexusctl touch` prints what the panel reports while `linexusctl demo --pattern border-grid` shows lines every 100 px (stop the daemon first).

`ping`:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `host` | string | `"1.1.1.1"` | Host name or IP address for ping widgets without a `host` of their own and for the `net.ping*` readings |
| `interval_ms` | number | `1000` | Time between two probes of a host, 250 to 60 000 ms |

The daemon only pings while the active profile shows a `ping` widget or a `net.ping*` reading (in a `metric` widget, its `secondary` reading or a text `{token}`), and not while the strip is off or unplugged. Each host gets one probe at a time, one per interval. A probe counts as lost when no answer comes within a second, or within its red threshold plus half a second when a `ping` widget for that host (or a `metric` widget showing `net.ping`) turns red later. A lost probe holds the next one back until that time ran out, so with a shorter interval a host that does not answer is probed once per timeout.

Probes are ICMP echo requests through an unprivileged ping socket, which follows network changes (a new address, another Wi-Fi, a VPN). Where the kernel does not allow those to your user (`net.ipv4.ping_group_range`), the daemon times a TCP connection to port 443 instead and the widget says `TCP`. Host names are looked up in the background, again every 5 minutes, and after 30 seconds when five probes in a row get lost. After five lost probes in a row the next address of the name gets a turn (IPv4 when IPv6 does not get through, for example).

`discord` (needs Discord connected in Studio, see [`discord`](#discord)):

| Field | Type | Default | Meaning |
|---|---|---|---|
| `notifications` | boolean | `true` | Show the message notifications Discord shows, with the author's picture; a tap opens the conversation in Discord. Discord's own notification settings decide which messages notify. Needs `notifications.enabled`. `notifications.duration_ms`, `notifications.private` (which also hides the author's picture) and `notifications.muted_apps` apply, and so does do not disturb on the desktop. While they arrive this way, a desktop notification repeating one of these messages is left out. |
| `scroll` | `"direct"`, `"all"`, `"never"` | `"direct"` | Which notifications too long for their line scroll to the end: they wait 2 seconds, scroll, stay 3 more seconds and go (at most 30 seconds in all). `direct`: direct and group messages only, server messages show their beginning. |
| `mute_indicator` | boolean | `true` | While you sit in a voice channel, a badge in the top right corner of every page shows your microphone: red while you are muted or deafened, white while it is on. Off outside a voice channel, and left out on pages that already show it (a mute, deafen or push-to-talk button, a `discord_call` widget or a Discord volume slider) |

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
| `discord_voice` | boolean | no | Switch to this profile while you sit in a Discord voice channel, and back to the previous one when you leave (unless you switched profiles in between). The first profile with it wins. |

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

`audio`:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `devices` | array | `[]` | Your names, order and visibility for sound devices, at most 64 entries |

Each entry of `devices`:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `node` | string | required | The device's PipeWire `node.name`, for example `alsa_output.usb-Astro_Gaming_Astro_A50-00.stereo-game`. `wpctl status -n` lists them, and Studio shows them under Connections → Sound. |
| `name` | string | not set | Shown instead of the name Linexus makes from PipeWire's description, at most 64 characters |
| `hidden` | boolean | not set | `true` leaves the device out of the sound widgets and of `audio_output` buttons without a device list, even while it is in use; `false` shows a device that is hidden by default |

The sound widgets show listed devices first, in the order listed, and the others after them as PipeWire ranks them. Virtual devices are hidden unless an entry says otherwise or they are in use: nodes marked `node.virtual`, null sinks, ALSA loopback devices (`snd_aloop`), loopback modules and the filters PipeWire runs (echo cancelling, noise filters: nodes with a `node.link-group` and no device). The output and the input in use always show, unless an entry hides them. After you switch from a virtual device to another one on the strip, it is offered again only when an entry shows it (switch it on in Studio). Studio → Connections → Sound edits the list: rename a device, switch it off for the strip or move it up.

```json
"audio": {
  "devices": [
    { "node": "alsa_output.pci-0000_11_00.1.hdmi-stereo-extra2", "name": "Monitor" },
    { "node": "alsa_input.usb-Astro_Gaming_Astro_A50-00.mono-chat", "name": "A50 Mic" },
    { "node": "alsa_output.pci-0000_13_00.4.iec958-stereo", "hidden": true }
  ]
}
```

### How the privacy badge decides

The daemon follows PipeWire with `pw-dump --monitor` while the badge is switched on or the profile uses a sound widget or an `audio_output` action, and not while the strip is off or unplugged. The badge counts:

- **Microphones:** a recording stream that is running (not corked) and linked to an input. For a virtual input (a noise filter such as rnnoise or DeepFilter, echo cancelling, a loopback, EasyEffects' source) the daemon follows PipeWire's links upstream, through the other nodes of a filter (those sharing its `node.link-group`): the recording counts when a real microphone feeds the chain and when nothing tells what feeds it, and does not count when only outputs' monitors feed it (a loopback of what an output plays) or for the capture side of the ALSA loopback card. EasyEffects shows as well while it records the microphone for its source. Recordings of an output (monitor captures, as screen recorders make them), volume applets' peak meters (`stream.monitor`, `pulse.peak-detect`) and PipeWire's own streams (program `pipewire` or `wireplumber`) do not count, and neither does the level meter of the [`microphone`](#microphone) widget, which carries the daemon's process id in its node name.
- **Cameras:** a PipeWire camera (`Video/Source`) that is running and linked to an app, named after that app; and any process of yours that holds `/dev/videoN` open (found in `/proc/<pid>/fd`), which also finds apps reading a v4l2loopback camera such as Droidcam directly. The program feeding a v4l2loopback camera (the Droidcam client, OBS's virtual camera) holds it open too and is named as well: the open files do not tell feeding from watching reliably. PipeWire's own processes are left out, since they open cameras for the apps they serve, and processes of other users cannot be looked into. The scan of open files runs when inotify sees a camera opened or closed, and every 30 seconds besides (every 3 seconds when a camera cannot be watched).

The app's name is PipeWire's `application.name`, or the program's name made readable (`obs` → `OBS`, `steamwebhelper` → `Steam`). The pill sits left of the Discord microphone badge while that shows. Only what it draws takes a touch, named or shrunk.

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
| `timezone` | not set | Accepted, but not applied yet; clocks show local time |

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
| `interface` | not set | Accepted, but not applied yet; the widget shows the sum of all interfaces except loopback |

### ping

Round trip to a host: the latest one in large digits, a graph of the last 60 probes with a red line for each lost one, and the jitter (mean difference between consecutive round trips) and packet loss over those probes. Wide widgets add the lowest, average and highest round trip; below 120 pixels only the round trip and the loss remain. The default gateway is called "Router". Until the first answer the widget says it is measuring; it also says when a name does not resolve or the host stopped answering.

| Field | Default | Meaning |
|---|---|---|
| `host` | not set | Host name or IP address; `settings.ping.host` when not set |
| `warn_ms` | `60` | Round trip from which the reading turns amber |
| `critical_ms` | `120` | Round trip from which the reading turns red; it is red as well while more than 2 % of the probes are lost. A probe waits for an answer up to this threshold plus half a second, but at least a second, so thresholds above half a second make probes wait longer, see [`ping`](#settings). |

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
| `step` | `5` | Accepted, but not used yet |
| `source` | `"system"` | `system` (the default output), `discord_input` (Discord's microphone level, 0 to 100 %) or `discord_output` (Discord's output level, 0 to 200 %) |

Built-in touch: slide along the bar to set the volume; tap the icon to mute or unmute (for `discord_output`: deafen). The system volume uses `wpctl`, or `pactl` when `wpctl` is missing.

### audio_output

Your sound outputs as tiles: a headset, headphones, a screen (HDMI or DisplayPort) or a speaker icon, the name and the device below it. The output in use has an accent frame. A tap sends the sound there: it becomes the default output and the apps playing on the previous one move along. Right of the tiles, a slider sets the volume of the output in use. When more outputs exist than fit, a swipe pages through them (dots show where you are) and a swipe on the last page changes the strip's page as usual. Names, order and hidden devices come from [`settings.audio`](#settings).

| Field | Default | Meaning |
|---|---|---|
| `show_volume` | `true` | The volume slider of the output in use |

```json
{ "id": "outputs", "x": 0, "w": 640, "type": "audio_output" }
```

Outputs change with `wpctl set-default`, and WirePlumber moves the streams that follow the default output by itself. A stream sent to the previous output by name (`target.object` in PipeWire's metadata or in its own properties, as `pavucontrol` or `pactl move-sink-input` leave it) is sent to the new one by its `object.serial`, as `pactl` does it, with `pw-metadata`; streams sent to other outputs stay where they are. The volume uses `wpctl set-volume`. While no output shows, the widget says so and takes no touches.

### mixer

One entry per app that plays sound: a letter in a colour of its own, the name, the volume (or "muted") and a slider. A tap on the letter mutes or unmutes the app, a slide sets its volume; both apply to all of the app's streams. Apps appear when they start playing and go about 5 seconds after their last stream ends or pauses. Up to four apps show side by side on the full strip, fewer on a narrower widget; a swipe pages through the rest.

| Field | Default | Meaning |
|---|---|---|
| `pinned` | `[]` | App names that come first while they play, in this order (at most 16; compared without regard to case) |

```json
{ "id": "apps", "x": 0, "w": 640, "type": "mixer", "pinned": ["Spotify", "Discord"] }
```

### microphone

The microphone in use, its live level, mute for all apps and the input volume. A tap on the device tile takes the next input the strip shows (hidden inputs, and virtual ones not in use, are skipped) and makes it the default. While no input shows (none is in use, or the one in use is hidden), the widget says so and takes no touches. The level shows as 25 segments: green, amber from the 17th, red from the 22nd; below it the state ("Voice coming through", "silent" or "muted for all apps"). The mute tile mutes the default input for every app (`wpctl set-mute`), and the slider sets its volume.

| Field | Default | Meaning |
|---|---|---|
| `show_level` | `true` | The level meter |
| `show_volume` | `true` | The input volume slider |

```json
{ "id": "mic", "x": 0, "w": 640, "type": "microphone" }
```

The level meter records the microphone: only while a page with this widget shows the input, the input is not muted and the strip is on and plugged in, the daemon runs `pw-record` on the default input at 16 kHz mono and turns the peak of every 64 ms into a level (−60 to 0 dBFS as 0 to 100 %). Nothing is stored or sent. The recorder stops when you leave the page, mute or hide the input, or the strip goes off or is unplugged, and follows a change of input. It is marked as a peak meter (`stream.monitor`), so WirePlumber does not switch a Bluetooth headset to its call profile for it. While it runs, your desktop's microphone indicator lights up (KDE Plasma shows one in the panel); the [privacy badge](#how-the-privacy-badge-decides) leaves this recording out. Switch `show_level` off to never record.

### button

| Field | Default | Meaning |
|---|---|---|
| `label` | `""` | Text |
| `icon` | not set | `lucide:<name>` (Lucide icon), `app:<desktop id>` (the application's icon, for example `app:org.kde.dolphin.desktop`) or a `data:image/...` URL |
| `style` | `"tile"` | `tile` (icon above the label), `inline` (icon and label side by side) or `icon` (icon only) |
| `show_label` | `true` | Show the label |
| `running_indicator` | `true` | For buttons whose tap action is `launch_app`: show a dot while that application runs |

A button whose tap action is [`discord`](#discord) follows Discord: mute and deafen light up red while on (a `mic` or `headphones` icon is crossed out), a join button lights up while you are in its channel, a leave button fades while you are not in voice, push to talk lights up green while the microphone is open, volume buttons show the level in place of the label, and the voice mode and audio processing toggles light up while on. All of them fade while Discord is not connected.

A button whose tap action is [`obs`](#obs) shows what OBS is doing: a recording or stream lights up red with its running time in place of the label (amber while a recording is paused or a stream reconnects), the scene on air and an active replay buffer or virtual camera light up in the accent colour, and a muted source lights up red (a `mic` or `volume` icon is crossed out). While OBS is not connected, and where the button has nothing to act on (pause without a recording, a replay clip without a replay buffer), it is faded.

### news

Two rows of headlines from the feeds and webhooks configured in Studio.

| Field | Default | Meaning |
|---|---|---|
| `font` | `"pixel"` | `pixel` (crisp bitmap font) or `smooth` |
| `feeds` | `[]` | Feed ids. Accepted, but not applied yet; the widget shows all enabled sources. |

Built-in touch: tap the left half to open the top headline, the right half for the bottom one; double tap left or right to step back or ahead a pair; hold to hide a headline.

Breaking news comes first for an hour, labelled BREAKING (EILMELDUNG, ALERTE) in the theme's `critical` colour; see [News feeds and breaking news](#news-feeds-and-breaking-news).

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

### discord_voice

Who sits in the voice channels of a Discord server, with their profile pictures: one tile per channel, the channel you are in first and lit up, a green ring around whoever speaks and a red badge on muted or deafened members. A tile is as wide as its pictures, at least 56 px; longer channel names are cut. When the channels do not all fit, they take turns a page at a time every 5 seconds, with dots below; the turning waits while the strip was touched in the last 10 seconds. Needs Discord set up in Studio under Connections (see [`discord`](#discord)).

| Field | Default | Meaning |
|---|---|---|
| `guild_id` | not set | Server id. Without one, the widget follows the server of the voice channel you are in. |
| `guild_name` | not set | The server's name when it was picked in Studio; only for display in Studio |
| `show_empty` | `false` | Also list channels nobody is in |

Built-in tap: joins the voice channel you tap; on the channel you are already in, says so.

### discord_call

The voice channel you are in, or the direct or group call (named after the others in it when it has no name): its name and the voice connection's ping on the left (green signal under 100 ms, amber under 250 ms, red above), then everyone in it with their picture and name, a green ring around whoever speaks, and badges for muted, deafened and muted-for-you members. Says so when you are in no voice channel. On the left, a button for your microphone stays visible whatever the row shows, also outside a call: a solid red tile while you are muted or deafened, a green microphone otherwise.

| Field | Default | Meaning |
|---|---|---|
| `show_names` | `true` | Names under the pictures |
| `show_ping` | `true` | Ping and connection quality under the channel name |

Built-in tap: the button on the left mutes or unmutes your microphone; while you are deafened it undeafens you. Tap someone to open their controls: mute them for you, quieter or louder in steps of 10 % (0 to 200 %), close. The controls close on their own after 8 seconds without a touch. These settings are yours only, as in Discord's own menu. Tapping yourself opens your microphone's controls instead (mute, quieter, louder, 0 to 100 %), since Discord sets no volume of yours for you.

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
| `text` | `"Hello"` | The text. `{metric id}` tokens are replaced with live values, for example `CPU {cpu.temp}`. `{obs.scene}` is OBS's scene on air, `{obs.record}` and `{obs.stream}` how long the recording and the stream run. A token without a value shows `–`. |
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

Page and profile actions, `brightness`, `display_off` and `sequence` are handled by the daemon itself. OBS commands go to the daemon's OBS connection. Everything else, Home Assistant calls included, runs one after another on a background worker, so a slow action never stalls the strip. When an action fails, the strip shows a short message and the event log records `action_failed`.

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

Changes the default output with `wpctl`, or `pactl` when `wpctl` is missing. `command` is `up`, `down`, `set` or `toggle_mute`. `step` (default 5) applies to `up` and `down`; `percent` (0 to 100) is required for `set`. Volume never goes above 100 %.

```json
{ "type": "volume", "command": "up", "step": 5 }
```

```json
{ "type": "volume", "command": "set", "percent": 40 }
```

### microphone_mute

Toggles the mute state of the default microphone with `wpctl`, or `pactl` when `wpctl` is missing.

```json
{ "type": "microphone_mute" }
```

### audio_output

Switches the sound output, like a tap on an [`audio_output`](#audio_output) widget's tile: the device becomes the default output and the apps playing on the previous one move along. `devices` lists PipeWire node names (at most 16). With one, a tap switches to it; with several, each tap takes the next one after the output in use; without any, each tap takes the next output the strip shows. Devices that are not connected are skipped.

```json
{ "type": "audio_output", "devices": ["alsa_output.usb-Astro_Gaming_Astro_A50-00.stereo-game", "alsa_output.pci-0000_11_00.1.hdmi-stereo-extra2"] }
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

### obs

Controls OBS Studio through its WebSocket server (OBS 28 or newer), using the connection saved in Studio under Connections. The daemon connects only while the active profile has OBS buttons or `{obs.*}` text.

| `command` | Does |
|---|---|
| `toggle_record`, `start_record`, `stop_record` | Recording |
| `pause_record` | Pauses a running recording, or resumes a paused one |
| `toggle_stream`, `start_stream`, `stop_stream` | Streaming |
| `set_scene` | Switches the program output to the scene named in `target` |
| `toggle_mute` | Mutes or unmutes the audio source named in `target` |
| `toggle_replay_buffer`, `save_replay` | Starts or stops the replay buffer; saves its contents as a clip |
| `toggle_virtual_cam` | Starts or stops the virtual camera |

`set_scene` and `toggle_mute` need `target`, the name as OBS shows it.

```json
{ "type": "obs", "command": "set_scene", "target": "Gameplay" }
```

### discord

Controls the Discord desktop app through its local RPC socket. Only a socket whose other end runs as your own user gets your token. The daemon connects while the active profile has Discord buttons, a [`discord_voice`](#discord_voice) or [`discord_call`](#discord_call) widget or a Discord volume slider, while a profile has `discord_voice`, or while `settings.discord` shows notifications or the microphone badge.

| `command` | Does |
|---|---|
| `toggle_mute` | Mutes or unmutes your microphone |
| `toggle_deafen` | Deafens or undeafens you |
| `push_to_talk` | Opens your microphone while the finger stays on the button and mutes it again on release; does nothing while you are not muted |
| `join_voice` | Joins the voice channel whose id is in `channel`, leaving the one you are in |
| `leave_voice` | Leaves voice |
| `input_volume_up`, `input_volume_down` | Microphone level up or down by `step` percent (default 10), 0 to 100 % |
| `output_volume_up`, `output_volume_down` | Discord's output level up or down by `step` percent (default 10), 0 to 200 % |
| `toggle_voice_mode` | Switches between voice activity and push to talk |
| `toggle_noise_suppression` | Turns noise suppression on or off |
| `toggle_echo_cancellation` | Turns echo cancellation on or off |
| `toggle_automatic_gain` | Turns automatic gain control on or off |

```json
{ "type": "discord", "command": "join_voice", "channel": "123456789012345678" }
```

Discord opens this socket only to applications it approved, and to up to 50 testers of others. Linexus therefore uses an application you create yourself in the [Discord developer portal](https://discord.com/developers/applications): add `http://localhost` under OAuth2 → Redirects (Discord refuses applications without a redirect, even though the local flow never opens it), paste the Client ID and Client Secret in Studio under Connections, click Connect and accept the prompt Discord shows. The secret and the tokens stay in `sensors.json` (mode 0600); the tokens never reach the browser, and the daemon renews them before they expire.

Notifications need the `rpc.notifications.read` permission, which Linexus asks for since it shows them: connections made before need Connect clicked once more. Until then, Studio says so next to the switch, and the strip says so once when the daemon connects. Should Discord leave the permission out when asked, Linexus connects for voice control alone and Studio says that instead, without asking to connect again. A tapped notification opens `discord://-/channels/…` with the desktop's handler for Discord links.

Only the official Discord app (stable, PTB, Canary, also as Flatpak or Snap) offers voice control. Vesktop and other clients that use arRPC only share what you play; Studio says so when only such a client answers.

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
| `gpu.fan` | rpm or % | GPU fan; NVIDIA reports percent of the maximum |
| `gpu.vram` | % | VRAM in use |
| `ram.usage` | % | Memory in use |
| `ram.used` | bytes | Memory in use |
| `ram.total` | bytes | Installed memory |
| `disk.usage` | % | Root file system in use |
| `disk.read` | bytes/s | Disk reads |
| `disk.write` | bytes/s | Disk writes |
| `net.down` | bytes/s | Received, all interfaces except loopback |
| `net.up` | bytes/s | Sent, all interfaces except loopback |
| `net.ping` | ms | Latest round trip to `settings.ping.host`; left out while the host does not answer |
| `net.ping.jitter` | ms | Mean difference between consecutive round trips over the last 60 probes |
| `net.ping.loss` | % | Lost probes among the last 60 |
| `volume.level` | % | Output volume |
| `hwmon:<chip>/<label>` | °C, rpm, W or V | One per hardware sensor input |
| `battery:<kind>/<key>` | % | One per battery |

The `gpu.*` readings come from an AMD GPU (sysfs and hwmon) or an NVIDIA GPU on the proprietary driver (`nvidia-smi`). With both, they follow the one with more video memory, which is usually the graphics card rather than the processor's integrated GPU. An NVIDIA GPU in a laptop that sleeps while unused is not asked while it sleeps: `gpu.*` then shows the integrated AMD GPU where there is one, and is left out otherwise.

The `net.ping*` readings exist only while the active profile uses one of them or shows a `ping` widget for `settings.ping.host`, see [`ping`](#settings). The `net.ping` graph has a point per probe, so it keeps moving while the host does not answer; a lost probe repeats the round trip before it; the graphs start over when `settings.ping.host` changes.

`hwmon:<chip>/<label>`: `<chip>` is the hwmon driver name, `<label>` the driver's label for the input or the input name (`temp1`, `fan2`). Both are lower-cased, and characters other than letters, digits, `-` and `.` become `_`. When two inputs end up with the same id, the later ones get `-2`, `-3` and so on. Examples: `hwmon:k10temp/tctl`, `hwmon:amdgpu/edge`, `hwmon:nct6798/fan2`.

`battery:<kind>/<key>`: read from `upower --dump` when UPower is installed, otherwise from `/sys/class/power_supply`. `<kind>` is what the battery powers (`battery` for the computer itself; with UPower also `mouse`, `keyboard`, `headset`, `gaming-input` and others; without UPower `device` for peripherals). `<key>` is the device's name in the kernel, for example `battery:mouse/hidpp_battery_0`.

## News feeds and breaking news

Studio → News edits the feeds in `sensors.json`. Besides the fields Studio has always written (`id`, `url`, `limit`, `refresh_seconds`, `show_mode`, `priority`, `max_age_seconds`, `tap_behavior`), a feed has these:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `format` | `"rss"`, `"tagesschau"` | `"rss"` | `rss` reads RSS and Atom. `tagesschau` reads the tagesschau API (`https://www.tagesschau.de/api2u/homepage/`) and keeps only the items it marks as breaking news. An unknown value reads as `rss`. |
| `alert` | boolean | `true` for `tagesschau`, else `false` | New items show as breaking news |
| `alert_keywords` | array of strings | `[]` | When not empty, only items whose title contains one of them count as breaking news. At most 20, each up to 60 characters. |
| `refresh_seconds` | number | `120` for `tagesschau`, else `900` | Seconds between fetches, at least 60. An alert feed is fetched at least every 15 minutes: a longer interval counts as 900. |

```json
{
  "rss": [
    {"id": "tagesschau", "url": "https://www.tagesschau.de/api2u/homepage/", "format": "tagesschau", "alert": true, "refresh_seconds": 120},
    {"id": "kicker", "url": "https://newsfeed.kicker.de/news/aktuell", "alert": true, "alert_keywords": ["Bayern*", "Abpfiff"]}
  ]
}
```

A keyword matches whole words, regardless of case (`KÖLN` matches "Köln", `strasse` matches "Straße", `istanbul` matches "İstanbul"); runs of spaces count as one, so a keyword can be several words. Headline and keyword are compared in the same Unicode form (NFC), so an umlaut that a feed sends as a letter followed by a combining mark matches as well. The characters before and after the match must not be letters, digits or combining marks: `Tor` matches "Tor in der 90. Minute" and "Last-Minute-Tor" but not "Torwart", and `Ko` does not match "Köln". A `*` at the end also allows longer words: `Bayern*` matches "Bayerns". Otherwise accents and umlauts must be written as in the headline; `ue` does not match `ü`.

An item of an alert feed is breaking news when all of these hold:

- **The source is not new.** The first successful fetch of a feed, after the daemon starts with no record of it or after its address changed, only records what is there; nothing from it shows. A fetch succeeds only when the answer is a feed: an RSS, Atom or RSS 1.0 document, with or without items, or the tagesschau API's answer. Anything else, such as a network's login page, is not stored and counts as a failed fetch; the headlines from before stay up.
- **The item is new.** An item counts as seen when the same title with the same link, or the same `guid` (`id` in Atom), came before, so a corrected title under the same `guid` does not show again. What was seen is kept in `alert-seen.json` for 14 days and survives a restart: up to 2000 entries per feed, one per item and one more for an item with a `guid`, so at least the last 1000 items.
- **The item is recent.** Its date (`pubDate`, `dc:date`, `published` or `updated`; `date` in the tagesschau API) is at most 30 minutes old. When the fetch is at most 30 minutes old and the feed's previous successful fetch came at most an hour before it, an item up to 6 hours old counts too if its date is after that previous fetch or after the newest item it brought, with 5 minutes to spare. That covers a feed checked less often than every 30 minutes (one that backs off after errors), dates without a time zone and a server clock that is behind; after a longer gap (the computer was off, the feed was down) only the 30 minutes count. An item without a date counts only in that case. The tagesschau API marks items as breaking news after they appeared, so there the moment that counts is when an item first shows up marked: in that case it counts up to 6 hours old.
- **A keyword matches**, when the feed has keywords.
- **It is not a repeat.** A story that showed in the last hour does not show again. Links are compared ignoring `http` or `https`, `www.`, `utm_` parameters and a trailing slash. From the same feed a repeat is an item with the same link, such as a title corrected under it; entries of a live blog that differ only after the `#`, and two goals both called "Tor für Bayern" with links of their own, each show. From another feed it is the same link, also ignoring what follows a `#`, or the same title (letters and digits only, compared as keywords are).

Breaking news shows as a banner across the strip: the label BREAKING (EILMELDUNG in German, ALERTE in French), the source and the headline, which scrolls when it is too long, in the theme's `critical` colour. It stays for `overlays.alert_duration_ms` and the strip then returns to where it was. A tap opens the item unless the feed's `tap_behavior` is `none`, and closes the banner; holding as long as the news widget's hold time closes it without opening. Either marks the headline read.

Banners rank above everything else:

- A banner covers notifications. A notification that is showing when a banner arrives waits and shows again in full afterwards; notifications and toasts that arrive during a banner wait for it too. Volume and track pop-ups are left out while a banner shows.
- Several banners show one after the other, oldest first. At most 5 wait; with more, the oldest is dropped. A banner that waited 30 minutes, while the strip was off or a calibration ran, is dropped.

For an hour after it appears, an item heads the news widget with the label; holding it there hides it as usual. The daemon keeps this hour in memory: after a restart the item takes its normal place again (it does not show as a banner twice). Both times are wall-clock time: the time a suspended computer slept counts. A feed switched off (`show_mode` `hidden`, the switch in Studio → News) is not fetched at all: it brings no headlines, no breaking news and no banners. With `overlays.alerts` off there are no banners, and breaking news only heads the news widget.

Feed requests send `User-Agent: Linexus/<version> (+https://github.com/demoriaan/linexus)` and nothing about you: no cookies, no account. They ask for a compressed answer, which brings the tagesschau API down from about 650 KB to about 100 KB, and give up after 20 seconds. Each fetch waits up to a tenth of the interval longer (at most 30 seconds), so feeds of several computers do not line up. A failed fetch, including an HTTP 429 or 5xx answer, is retried after 30 seconds, then after twice as long each time, at most 10 minutes. The tagesschau API allows 60 requests an hour; the default of two minutes stays well below that.

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
- `overlays.alert_duration_ms` is between 5000 and 120 000.
- `shell_command` actions and `command` widgets name a program without `/`.
- `join_voice` Discord actions name a channel id (digits only), and a `discord_voice` widget's `guild_id`, when set, is digits only.
- `settings.audio.devices` has at most 64 entries, each `node` listed once, not empty and at most 256 characters, and each `name`, when set, not blank and at most 64 characters. A mixer's `pinned` and an `audio_output` action's `devices` have at most 16 entries, none blank.
- `http_request` URLs start with `http://` or `https://`, and `content_type` contains no line break.
- Sequences nest at most one level deep, have at most 16 steps and wait at most 60 000 ms between steps.
- Background `dim_percent` is at most 90.
- `settings.ping.host` and a `ping` widget's `host` are an IP address or a host name of at most 253 characters: dot-separated labels of letters, digits and inner hyphens, each 1 to 63 characters, the last one not all digits. Unspecified, multicast and broadcast addresses are refused, and so are IPv6 link-local ones (`fe80::/10`, which need an interface); an IPv4-mapped IPv6 address (`::ffff:192.0.2.1`) counts as the IPv4 address it carries.
- `settings.ping.interval_ms` is 250 to 60 000; a `ping` widget's `warn_ms` and `critical_ms` are 1 to 10 000 and `warn_ms` is below `critical_ms`.
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

Images are embedded in the config as data URLs, so there are no other files. A pack holds `config.json` only: feeds, the Home Assistant token, the OBS password and other entries of `sensors.json` are not included. URLs and request bodies in your actions are included as written.

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
