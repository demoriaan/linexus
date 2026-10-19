# Architecture

A map of the code for contributors: which crate owns what, how the daemon is put together, how a frame reaches the strip and how a touch becomes an action.

## Repository layout

| Path | Content |
|---|---|
| `crates/` | The Rust workspace |
| `apps/nexus-studio/` | Studio frontend: plain ES modules with Preact, htm and Preact Signals vendored in `vendor/`. No bundler and no build step. |
| `packaging/arch/` | `PKGBUILD`, install hooks and `build-local-package.sh` |
| `packaging/scripts/` | `install-dev.sh`, `apply-udev-rule.sh` |
| `packaging/udev/`, `packaging/systemd/`, `packaging/desktop/`, `packaging/icons/` | udev rule, systemd units, launcher, icons |
| `docs/` | This documentation |

## Crates

| Crate | Kind | Owns | Uses |
|---|---|---|---|
| `nexus-core` | library | USB ids, frame encoding (`panel_frame`, `image_reports`), control reports, touch report parser, the simple gesture recognizer used by `linexusctl touch`, HID enumeration, the udev rule text, the device lock | |
| `nexus-render` | library | The renderer: a page plus `LiveData` in, a 640 x 48 RGBA `Canvas` out. Widgets, theme palettes, fonts, Lucide icons, overlays, page transitions, PNG encoding. No file, device or network access. | config, core |
| `nexus-config` | library | Config schema 2: model, validation, defaults and page templates, schema 1 migration, atomic load and save, conversion of `ActionSpec` into desktop actions | actions |
| `nexus-sensors` | library | Everything the strip displays from outside: `/proc` and sysfs metrics, hwmon, GPU, volume, MPRIS, running and installed apps, batteries, weather, Home Assistant, RSS and Atom feeds, webhooks, desktop notifications, focus watchers, command output, `sensors.json`, MQTT and IFTTT | |
| `nexus-actions` | library | Plans and runs desktop actions through `xdg-open`, `gtk-launch`, `kioclient`, `wpctl`, `busctl`, `curl`, `loginctl` and the input helper; probes which of these exist; reads the graphical session environment and default applications | input |
| `nexus-input` | library | Input helper protocol: request and response types, socket paths, client | |
| `nexus-ipc` | library | Daemon API: `Command`, `Response`, `DaemonStatus`, socket path, client | |
| `nexus-events` | library | JSON Lines event log with rotation | |
| `nexus-pack` | library | `.nexuspack` export, inspect and import | config |
| `nexus-daemon` | binary `linexusd` | The daemon | core, render, sensors, config, actions, events, ipc |
| `nexus-studio` | binary `linexus-studio` | Studio's HTTP server, JSON API and embedded frontend | actions, config, core, events, ipc, pack, render, sensors |
| `nexus-inputd` | binary `linexus-inputd` | The input helper | input |
| `linexusctl` | binary `linexusctl` | Command line tool | core, render, sensors, actions, config, events, ipc, pack, input |

## The daemon

### Threads

| Thread | Code | Job |
|---|---|---|
| Main | `main.rs`, `hardware.rs` | Owns the `Engine`. Runs the hardware loop (or the emulator loop): answers API requests, renders and sends frames, reads touch reports. |
| IPC listener, plus one short-lived thread per connection | `ipc.rs` | Checks the peer user ID, parses the request, hands it to the main loop over a channel and waits up to 3 s for the reply |
| Metrics worker | `metrics_cache.rs` | Collects system metrics and news cards into a shared snapshot every second; paused while the strip is off |
| Provider worker | `live.rs` | Fetches slow or remote data, only for widgets the active profile contains |
| Action worker | `dispatch.rs` | Runs actions one after another from a queue of 64; reports failures back to the engine |
| Notification monitor | `nexus_sensors::NotificationMonitor` | Follows `dbus-monitor` for desktop notifications |
| Focus watcher | `nexus_sensors::FocusWatcher` | Reports the focused application (KWin, Hyprland, Sway or X11) |

Lock and sleep detection (`session.rs`) is no thread: the main loop polls it at most once per second.

### Modules

| Module | Responsibility |
|---|---|
| `main.rs` | Command line, wiring of all parts, device lock, restoring the last profile and page |
| `hardware.rs` | Hardware loop, emulator loop, one-shot rendering, opening the HID device, signal handling |
| `engine.rs` | The `Engine`: touches to interactions to actions, API requests, the periodic `tick`, frame pacing |
| `scene.rs` | Frame composition: page drags and transitions, press feedback, page indicator, overlays (toast, notification, volume, now playing) |
| `touch.rs` | `TouchMachine`, the gesture state machine |
| `runtime.rs` | `Runtime`: config, active profile and page, `go_back` history, brightness, blank flags, idle timers; `RuntimeState` in `runtime.json` |
| `live.rs` | `LiveBuilder` (metric history for graphs, assembly of `LiveData`) and the provider worker |
| `metrics_cache.rs` | Metrics worker and its snapshot |
| `dispatch.rs` | Action worker, the `--allow-shell-actions` gate, Home Assistant service calls |
| `ipc.rs` | Socket server |
| `session.rs` | Session lock and sleep detection through `loginctl` and `busctl` |
| `timers.rs` | Timer widget state: pomodoro phases, countdown, stopwatch |
| `news.rs` | Headline rotation, marquee pacing, taps and holds on headlines |
| `log.rs` | Event log helpers |

### The main loop

`hardware::run_hardware` opens the first HID interface with vendor `0x1b1c`, product `0x1b8e` and usage page `0x000c`. While that fails, it logs `hardware_unavailable`, keeps answering the API and ticking the engine, and retries every 2 seconds. Once connected, `drive` repeats:

1. Poll the session monitor; drain API requests; `engine.tick` (scheduled sequence steps, timers, notifications, volume and track overlays, focus changes, auto-return, idle timeout, provider wishes).
2. When a frame is due: send the brightness report if the effective brightness changed; send the blank report when the strip turns off; otherwise render a frame and send it if it changed or 2 seconds have passed.
3. Schedule the next frame with `engine.next_frame_in`: 20 ms while something animates, 250 ms while seconds or a running timer are visible, the marquee's interval while a headline scrolls, 250 ms while blanked, otherwise 500 ms, and never later than the next hold or double-tap deadline.
4. Wait for touch reports until then (at most `--touch-poll-ms`), handle up to 64 of them, and pull the next frame forward to within 8 ms after a touch.

On `SIGINT` or `SIGTERM`, the loop ends and the daemon hands the panel back to its built-in animation. `run_emulator` runs the same steps but writes a PNG instead (at most 10 frames per second).

## From config to pixels

```text
Runtime (config, profile, page) ─┐
MetricsSnapshot (every 1 s)      ├─> LiveBuilder::build ─> LiveData
Provided (weather, entities, …) ─┘                            │
                                                              v
Scene::compose ── nexus_render::render_page (+ transition, press, indicator, overlay)
                                                              │
                                                              v
                                              Canvas: 640 x 48 RGBA
                                                              │
               nexus_core::image_reports: panel_frame (BGRX, 4 x 4 dither), 121 reports
                                                              │
                                                              v
                                       hidapi write  (or encode_png in emulator mode)
```

Studio's previews use the same `nexus_render::render_page` over sample data, with live readings from the daemon laid over it, so the editor shows what the strip will show. The byte-level format is in [protocol.md](protocol.md).

## From touch to action

```text
input report 0x01 ─> nexus_core::TouchParser ─> TouchEvent { began | moved | ended, x }
   ─> Engine::touch_event      wake-only touches, closing overlays, widget under x
   ─> TouchMachine             Press, Release, Tap, DoubleTap, Hold, Slide, Drag, DragEnd
   ─> Engine::interact         the widget's action for the gesture, or its built-in behaviour
   ─> Engine::perform          pages, profiles, brightness, display_off, sequence: handled here
                               everything else: ActionSpec::to_external_action
   ─> Dispatcher (action worker) ─> nexus_actions::execute_action_watched
                                    or nexus_sensors::call_service (Home Assistant)
```

The `tap` and `touch` API commands, and so Studio's live mirror, enter at `Engine::touch_event` as well.

`TouchMachine` thresholds (`touch.rs`):

| Constant | Value | Meaning |
|---|---|---|
| `DRAG_START_PX` | 24 px | Travel that turns a press into a page drag |
| `COMMIT_PX` | 140 px | Drag distance that changes the page on release |
| `FLICK_PX`, `FLICK_WINDOW` | 60 px within 280 ms | A quick flick that changes the page |
| `DOUBLE_TAP_WINDOW`, `DOUBLE_TAP_DISTANCE` | 300 ms, 80 px | Second tap that makes a double tap |
| `REPEAT_INTERVAL` | 160 ms | Pause between repeated holds |
| `hold` | 600 ms | Hold threshold; the engine sets it from the headline settings in `sensors.json` |

Single taps fire on release unless the widget also has a double-tap meaning; only then are they held back for the double-tap window. Holds fire while the finger is still down. A volume widget captures the finger as a slider instead.

`nexus_core::GestureRecognizer` is a simpler recognizer used only by `linexusctl touch`.

## Providers and refresh intervals

| Data | Source | Refresh |
|---|---|---|
| CPU, memory, disk, network, GPU, volume, now playing | `/proc`, sysfs, `wpctl`, `busctl` | Every second, paused while the strip is off |
| AMD GPU via `rocm-smi` | `rocm-smi`, where sysfs has no values | At most every 5 s |
| hwmon sensors | `/sys/class/hwmon` | Cached 5 s |
| Running applications (button dots) | `/proc` | Cached 5 s |
| Installed applications | Desktop files | Cached 60 s |
| Batteries | `upower --dump` or `/sys/class/power_supply` | Cached 30 s |
| Feed headlines | RSS and Atom through `curl`, cached in `rss-cache/` | Per feed, `refresh_seconds` (default 15 min) |
| Webhook cards | `~/.local/state/linexus/webhooks/` | With every metrics collection |
| Weather | Open-Meteo through `curl` | 15 min per location; 60 s after a failure |
| Home Assistant entities | Home Assistant REST API through `curl` | 5 s, and shortly after a toggle |
| Command widget output | The configured program, 5 s timeout | The widget's interval, at least 2 s |
| Album art, application icons | Art URL from MPRIS, icon themes | When first needed |
| Desktop notifications | `dbus-monitor` | As they arrive |
| Focused application | KWin script and journal; `hyprctl`, `swaymsg` or `xprop` | KWin as it changes; others every 0.7 s |
| Session lock, sleep | `loginctl`, `busctl` | Every second |

The provider worker re-reads the active profile's "wants" every second, so switching profiles starts and stops fetches.

## Studio server

| File | Responsibility |
|---|---|
| `main.rs` | Command line, loopback check, binding, one thread per connection |
| `http.rs` | Minimal HTTP/1.1: one request per connection, `Content-Length` bodies only, size limits, Host check, write token check, security headers |
| `api/mod.rs` | Dispatch: webhook ingest first, then the Host and token gates, then the routes |
| `api/config.rs` | Load, validate and save the config, templates, themes, pack export and import |
| `api/preview.rs` | Draft page previews, the live frame, the metric catalog |
| `api/daemon.rs` | Daemon commands and `systemctl --user` for the daemon unit |
| `api/catalog.rs` | Icons, installed apps, app icons, geocoding, weather |
| `api/sensors.rs` | `sensors.json`, feeds, Home Assistant, webhooks, MQTT and IFTTT tests |
| `api/system.rs` | Setup checks, event log, version, session token |
| `assets.rs` | Serving the frontend, token injection, ETag |
| `state.rs` | Paths, port and the per-start token |

### Embedded frontend

`crates/nexus-studio/build.rs` walks `apps/nexus-studio/`, skips dot files and writes `$OUT_DIR/assets.rs`: a table of `{ path, mime, bytes }` sorted by path, with the bytes pulled in by `include_bytes!`. Whatever the frontend directory contains is served; there is no list of file names to maintain. Cargo reruns the script when anything in that directory changes.

The same script hashes every path and file (FNV-1a) into one `ASSETS_ETAG` of the form `"<version>-<hash>"`. Every file is served with `Cache-Control: no-cache` and that ETag, and a matching `If-None-Match` gets 304. Any changed file changes the ETag of all files, so a browser never mixes modules from two builds.

For frontend work without rebuilding, point Studio at the checkout; files are then read from disk on every request:

```bash
NEXUS_STUDIO_ASSETS=apps/nexus-studio cargo run -p nexus-studio -- --port 19778
```

## Running against the emulator

The daemon can run without the strip and write every frame to a PNG file instead. Together with separate socket and state paths, it runs next to an installed daemon without touching it:

```bash
mkdir -p /tmp/nexus-dev
cargo run -q -p nexusctl -- config init --path /tmp/nexus-dev/config.json
cargo run -q -p nexus-daemon -- \
  --emulator-output /tmp/nexus-dev/frame.png \
  --config /tmp/nexus-dev/config.json \
  --socket-path /tmp/nexus-dev/daemon.sock \
  --state-path /tmp/nexus-dev/runtime.json \
  --dry-run-actions
```

In a second terminal:

```bash
cargo run -q -p nexusctl -- daemon --socket-path /tmp/nexus-dev/daemon.sock status
cargo run -q -p nexusctl -- daemon --socket-path /tmp/nexus-dev/daemon.sock tap 300
cargo run -q -p nexus-studio -- --port 19778 \
  --config /tmp/nexus-dev/config.json --socket-path /tmp/nexus-dev/daemon.sock
```

Studio's live mode then mirrors the emulator, and clicks on it act as touches.

| Flag | Effect |
|---|---|
| `--emulator-output <PNG>` | Do not open the strip and take no device lock; rewrite this PNG (at most 10 frames per second). The focus watcher is off. |
| `--dry-run-actions` | Plan actions without running them; the event log records `action_dry_run` with the command that would run |
| `--socket-path <PATH>` | API socket |
| `--state-path <PATH>` | Where the last profile, page and brightness are kept |
| `--config <PATH>` | Config file |
| `--page <ID>` | Start on this page of the active profile |
| `--no-restore-state` | Start on the first page instead of the last one |
| `--once` | Render one frame (to the strip, or to `--emulator-output`) and exit |
| `--touch-poll-ms <MS>` | Longest wait for touch input between frames (default 50) |
| `--disable-session-blank` | Keep the strip on while the session is locked |
| `--allow-shell-actions` | Allow `shell_command` actions and command widgets |

Some paths are not flags. To keep test runs apart from your own data, set these environment variables:

| Variable | Default | Content |
|---|---|---|
| `XDG_STATE_HOME` | `~/.local/state` | Event log and feed state |
| `NEXUS_SENSOR_CONFIG` | `~/.config/linexus/sensors.json` | Feeds and connections |
| `NEXUS_SENSOR_WEBHOOK_DIR` | `~/.local/state/linexus/webhooks` | Received webhooks |
| `NEXUS_SENSOR_RSS_SEEN_PATH` | `~/.local/state/linexus/rss-seen.json` | Read and hidden headlines |

## Render previews

`linexusctl render-preview` draws a page or a test pattern to a PNG without the strip or the daemon:

```bash
linexusctl render-preview --out overview.png
linexusctl render-preview --template media --theme ember --scale 2 --out media.png
linexusctl render-preview --config ~/.config/linexus/config.json --page launcher --out launcher.png
linexusctl render-preview --pattern border-grid --out grid.png
```

| Option | Effect |
|---|---|
| `--template <NAME>` | Built-in page template: `overview` (default), `performance`, `media`, `launcher`, `news`, `focus`, `home`, `blank` |
| `--config <FILE>` | Render a page from a config file instead of a template |
| `--profile <ID>` | Profile in `--config`; the active profile by default |
| `--page <ID>` | Page in `--config`; the profile's first visible page by default |
| `--pattern <NAME>` | Test pattern instead of a page: `rgb-bars` or `border-grid` |
| `--theme <NAME>` | `midnight`, `aurora`, `ember`, `neon`, `mono` or `daylight` |
| `--locale <LANG>` | `en` or `de` |
| `--scale <N>` | Enlarge by an integer factor (nearest neighbour) |
| `--out <FILE>` | Output PNG (required) |

To capture what a running daemon shows, use `linexusctl daemon frame --out strip.png` instead.

## Tests and lints

The CI runs:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

It also checks that the workspace builds with the minimum Rust version, 1.88 (with rustup: `cargo +1.88 check --workspace --locked`), and that every frontend script parses:

```bash
for file in $(git ls-files 'apps/nexus-studio/*.js' 'apps/nexus-studio/*.mjs'); do
  node --check "$file"
done
```

The CI installs `libudev-dev` and `pkg-config` as system libraries.
