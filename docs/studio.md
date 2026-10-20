# Linexus Studio

Linexus Studio is the configurator for the strip. It runs on your computer as a small local web server and opens in your browser at `http://127.0.0.1:19777/`. Start it from the app launcher ("Linexus Studio") or with `linexus-studio`; see [install.md](install.md#open-linexus-studio) for its options.

Studio edits `~/.config/linexus/config.json` and, for feeds and connections, `~/.config/linexus/sensors.json`. Every change is saved automatically and the daemon reloads it, so the strip shows your edits right away. Studio also works while the daemon is stopped or the strip is unplugged; only the live view of the strip is missing then.

The interface is available in English and German.

## Layout

The window has a top bar, a navigation rail on the left and the current section.

| Section | What you do there |
|---|---|
| **Pages** | Design pages: the strip on stage, page thumbnails, widget library, inspector |
| **News** | Feeds for the Headlines widget, and how headlines move |
| **Connections** | Home Assistant, the default weather place, desktop notifications, webhooks, MQTT, IFTTT |
| **Device** | Strip and daemon status, behaviour settings, setup checks, event log |
| **Settings** | Studio language and appearance, backups, about |

The top bar holds, from left to right:

- **Profile menu**: the profile you are editing. A dot marks the profile shown on the strip. The menu switches between profiles and offers "Show ... on the strip", Rename, Duplicate and Delete (when there is more than one profile). Delete asks for a second click.
- **Save state**: "All changes saved", "Unsaved changes", "Saving…" or "Could not save" (hover for the reason).
- **Undo** and **Redo**.
- **Search or run a command**: the command palette, also on <kbd>Ctrl</kbd>+<kbd>K</kbd>.
- **Device pill**: what the strip shows right now, "Daemon not running", "Strip turned off", "Waiting for the Nexus" or "Emulator". Click it to open Device.

When a setup check reports an error, the Pages section shows a banner with a link to the fix.

## Pages

### Stage

The stage shows the strip large, with a ruler marking every 80 pixels. It has two modes.

**Edit** shows the page you are editing, drawn by the same renderer as the daemon. When the daemon runs, metric widgets show live readings.

- Click a widget to select it. Click empty space to clear the selection.
- Drag a widget to move it. Drag its left or right edge to resize it.
- Positions snap to 8 pixels. Widgets stop at their neighbours and are at least 24 pixels wide, because widgets on the strip never overlap.
- A **+** in a free gap adds a button there.

**Live** shows what the strip shows right now, refreshed several times per second. Clicking works like touching the panel: click to tap, press and keep the mouse button down to hold, drag sideways to swipe. Live mode needs the running daemon.

While you edit a page of the profile that is on the strip, the strip switches to that page, so you can check every change on the hardware.

### Pages strip

Below the stage, thumbnails show all pages of the profile. The number is the page's position when swiping; hidden pages ("folders") show a folder icon instead.

- Click a thumbnail to edit that page.
- Drag thumbnails to change the order.
- **Add page** opens the template gallery. The new page is inserted after the current one.

| Template | Content |
|---|---|
| Overview | Clock, CPU, GPU, memory and network at a glance |
| Performance | Five ring gauges for load, memory and disk |
| Media | Album art, track, playback controls and volume |
| Launcher | Six buttons for apps and quick actions: browser, files, terminal, mail, microphone, lock |
| News | Two headlines from your feeds; tap to read |
| Focus | Pomodoro timer, weather and a large clock |
| Home | Weather and Home Assistant switches |
| Empty page | Nothing yet |

### Widget library

Drag a widget onto the strip, or click it to place it in the largest free gap. When a page has no free gap, narrow a widget first.

| Widget | Shows |
|---|---|
| Button | Icon and label; launches an app, opens a link or runs any action |
| Clock | Time and date, optionally with seconds |
| Reading | One sensor as number, bar, graph or ring |
| Network | Download and upload speed |
| Now playing | Album art, title and playback buttons |
| Volume | A slider you move along the strip |
| Headlines | Two rows from your news feeds and webhooks |
| Weather | Current conditions, high and low |
| Home Assistant | A light or switch you can toggle |
| Timer | Pomodoro, countdown or stopwatch |
| Text | Your own words, with live values like `{cpu.temp}` |
| Image | A logo or photo |
| Command output | What a program prints, refreshed regularly (needs `--allow-shell-actions`) |
| Spacer | Empty space |

[configuration.md](configuration.md#widgets) lists every field of every widget.

### Inspector

The inspector on the right edits what is selected.

**A widget**: its own fields, position and width in pixels, and a colour. Duplicate copies it into a free gap; Delete asks for a second click. The **Touch** panel sets what happens on tap, double tap and hold; see [Gestures](#gestures).

**No widget** (the page and its profile):

- **Page**: name, duplicate, delete, and "Open only from a button", which turns the page into a folder that swipes skip. A folder can return to the page it was opened from after a number of seconds without a touch ("Return after").
- **Background**: an image, how it fits, a colour, and how much to darken it. "Use theme background" removes it.
- **Look** of the profile: theme, accent colour, the apps that switch to this profile (see [App-aware profiles](#app-aware-profiles)) and the brightness. Moving the brightness slider also changes the strip right away.

## Gestures

The panel senses only where along the strip you touch, so each widget owns its full height. In the inspector, each gesture has an action picker. "Nothing (built-in behaviour)" keeps the widget's own behaviour, described under the picker.

| Gesture | On the strip |
|---|---|
| Tap | A short touch. It fires when the finger lifts. On widgets that also react to a double tap, the single tap waits 300 ms for a possible second tap. |
| Double tap | Two taps within 300 ms, at most 80 pixels apart, on the same widget |
| Hold | The finger rests on the widget for about 0.6 seconds. It fires while the finger is still down; lifting it afterwards does nothing more. The "Hold to hide" setting under News changes this time. |
| Repeat while held | After the first hold, the action repeats every 0.16 seconds until the finger lifts, like a held key. It repeats the hold action, or the tap action when there is no hold action. |
| Swipe | Move the finger 24 pixels or more and the page follows it. Lift after 140 pixels, or flick at least 60 pixels within 0.28 seconds, to change the page. Swiping left shows the next page. |

Built-in behaviour, used while the gesture has no action of its own:

| Widget | Tap | Double tap | Hold |
|---|---|---|---|
| Now playing | Buttons skip back, play or pause, and skip forward; the title plays or pauses | | |
| Volume | Slide along the bar to set the volume; tap the speaker to mute | | |
| Headlines | Left half opens the top headline, right half the bottom one | Left or right half jumps back or ahead a pair | Hides the headline |
| Timer | Start or pause | Skip to the next phase | Reset |
| Home Assistant | Toggle the entity | | |

Some touches do not reach widgets:

- While the strip is dimmed with "First touch only wakes" switched on, or turned off by the idle timer or a "Turn the strip off" action, the first touch only wakes it.
- While a notification, volume or track pop-up is shown, a touch closes it.

### Actions

The action picker groups the actions:

| Group | Actions |
|---|---|
| Open | Launch an app, Open a website, Open a file or folder |
| Sound and media | Media control, Volume, Mute microphone |
| Keyboard | Press keys, Type text (need the [input helper](input-helper.md)) |
| The strip | Next page, Previous page, Go to page, Go back, Switch profile, Next profile, Previous profile, Brightness, Turn the strip off |
| System and services | Lock the screen, Home Assistant service, Web request, Run a program, Several actions |

- **Launch an app** picks from your installed applications, or "Web browser", "File manager", "Terminal" and "Mail", which follow your desktop's default applications.
- **Press keys** records a shortcut: click the field, then press the keys.
- **Run a program** only runs when the daemon is started with `--allow-shell-actions`.
- **Several actions** runs a list of actions in order, with an optional delay between steps.

[configuration.md](configuration.md#actions) describes every action and its fields.

## Undo, redo and autosave

Studio saves about half a second after your last change. Before saving, the server checks the layout; when something blocks saving (for example two widgets that overlap), a banner names the problem and the last valid state stays on disk.

Edits that are not saved yet are also kept in the browser. If you close the tab before a save succeeded, Studio offers to restore them the next time it opens, and it warns you when you leave the page with unsaved changes.

When the configuration file changed since Studio loaded it (another Studio tab, `linexusctl` or a text editor), Studio does not overwrite it silently: it asks whether to reload that version or overwrite it with your changes.

Undo and redo go back and forth through up to 200 steps. Typing in a field or dragging a widget counts as one step. The history lives in the browser tab and starts empty when you reload Studio.

## Keyboard shortcuts

| Keys | Where | Effect |
|---|---|---|
| <kbd>Ctrl</kbd>+<kbd>K</kbd> | everywhere | Open or close the command palette |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> | Pages | Undo |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Z</kbd> or <kbd>Ctrl</kbd>+<kbd>Y</kbd> | Pages | Redo |
| <kbd>Delete</kbd> or <kbd>Backspace</kbd> | Pages, widget selected | Delete the widget |
| <kbd>Esc</kbd> | Pages, widget selected | Clear the selection |
| <kbd>←</kbd> <kbd>→</kbd> | Pages, widget selected | Move the widget by 1 pixel |
| <kbd>Shift</kbd>+<kbd>←</kbd> <kbd>→</kbd> | Pages, widget selected | Move by 8 pixels |
| <kbd>Alt</kbd>+<kbd>←</kbd> <kbd>→</kbd> | Pages, widget selected | Make the widget narrower or wider (with <kbd>Shift</kbd>: by 8 pixels) |
| <kbd>Enter</kbd> | on a focused widget on the stage | Select it |
| <kbd>↑</kbd> <kbd>↓</kbd>, <kbd>Enter</kbd>, <kbd>Esc</kbd> | command palette | Move, run, close |
| <kbd>Esc</kbd> | dialogs and pop-ups | Close |

The shortcuts of the Pages section are off while you type in a field or a dialog is open.

## Command palette

<kbd>Ctrl</kbd>+<kbd>K</kbd> opens a search box. Type part of a name and press <kbd>Enter</kbd>. It finds:

- sections, pages and profiles to edit,
- "Show ... on the strip" for every page and profile,
- "Add ..." for every widget type, and "Add a page",
- strip commands: turn off, turn on, brightness 25 to 100 %, restart the daemon, show a test message,
- undo and redo, dark or light appearance, switching the language, and exporting a backup.

## App-aware profiles

A profile can take over the strip while a certain application has focus, for example a media profile while your video editor is in front.

1. Select the profile in the profile menu.
2. With no widget selected, find **Switch on for apps** in the **Look** panel.
3. Click **Add app** and pick the application.

While one of these applications has focus, the strip shows the profile. When focus moves to an application that no profile lists, the strip returns to the profile it showed before. Choosing a profile yourself ends this automatic return. Automatic switches are not remembered across daemon restarts. The matching rules are in [configuration.md](configuration.md#profiles).

The daemon needs a way to learn which window has focus. It picks one when it starts:

| Desktop | Detected through | How the focus is read | Needs |
|---|---|---|---|
| KDE Plasma | `XDG_CURRENT_DESKTOP` contains `KDE` | A small KWin script reports focus changes to the user journal, which the daemon follows. No polling. | `dbus-send`, `journalctl` |
| Hyprland | `HYPRLAND_INSTANCE_SIGNATURE` is set | `hyprctl activewindow -j`, every 0.7 seconds | `hyprctl` |
| Sway | `SWAYSOCK` is set | `swaymsg -t get_tree`, every 0.7 seconds | `swaymsg` |
| GNOME | `XDG_CURRENT_DESKTOP` contains `GNOME` and GNOME Shell is on the session bus (on X11 only when the extension is installed) | The Linexus focus extension writes the focused app to `$XDG_RUNTIME_DIR/linexus/gnome-focus.json`; the daemon reads it every 0.7 seconds | GNOME Shell 40 or newer, the extension |
| Other X11 desktops | `XDG_SESSION_TYPE` is `x11` and `DISPLAY` is set | `xprop` on the root window, every 0.7 seconds | `xprop` |

GNOME tells only shell extensions which window has focus. The installer sets up the small Linexus focus extension on GNOME; to do it yourself:

```bash
linexusctl gnome-extension install
```

It writes the extension to `~/.local/share/gnome-shell/extensions/linexus-focus@demoriaan.github.io/`, in the format for your GNOME Shell version, and adds it to `enabled-extensions`. GNOME Shell loads new extensions at login, so log out and back in once. `linexusctl gnome-extension status` shows whether it runs; `linexusctl gnome-extension remove` takes it out again. After a GNOME upgrade to a version the extension does not list yet, `linexusctl doctor --fix` installs it again.

The daemon usually starts before the desktop has announced itself. Until it finds a desktop it knows, it checks again in the background, every 5 seconds for two minutes and every 30 seconds after that.

`XDG_CURRENT_DESKTOP`, `XDG_SESSION_TYPE`, `DISPLAY`, `HYPRLAND_INSTANCE_SIGNATURE` and `SWAYSOCK` are read from the systemd user manager (`systemctl --user show-environment`), or else from the daemon's own environment. If your Hyprland or Sway startup does not hand its variables to the user manager, add for example `systemctl --user import-environment HYPRLAND_INSTANCE_SIGNATURE` to it; the daemon picks them up at its next check. The event log records the chosen method as `focus_watch`, and on GNOME a `focus_watch_problem` when the extension cannot run:

```bash
linexusctl events list | grep focus_watch
```

The focus watcher does not run in emulator mode.

## News

The News section manages the sources of the Headlines widget. Changes save on their own.

- **Add a source**: type a website or feed address and choose **Find feed**. Studio looks for the feed and lists more feeds the page links. A list of suggestions is offered below the field.
- **Your sources**: each source has an on/off switch, **Latest headlines** to check it, the number of headlines it contributes, how often it updates (5 minutes to 4 hours), its order (First, Normal, Last) and what a tap does (open the article, or nothing). Arrows reorder sources.
- **Movement**: scroll speed for long headlines, pauses before and after scrolling, the minimum time a pair of headlines stays, and the hold time that hides a headline. **Reset** restores the defaults.
- **Read headlines**: mark the current headlines as read, or all as unread. Unread headlines come first.

When `sensors.json` exists but cannot be read, Studio shows the error and does not change the file.

## Connections

### Home Assistant

Enter the address of your Home Assistant (for example `http://homeassistant.local:8123`) and a long-lived access token, which you create in Home Assistant under your profile, Security. **Test connection** reports how many entities it found; **Save** stores both in `sensors.json`. Home Assistant widgets and the "Home Assistant service" action use this connection, and the widget's entity field then offers a list to pick from.

### Weather

Search a place to set the default location for weather widgets. Weather data comes from Open-Meteo and needs no account or key.

### Messages on the strip

- **Show desktop notifications** mirrors notifications onto the strip, with the app's icon or the picture the notification carries. It follows the desktop's notification settings: during Do Not Disturb only urgent notifications appear (on KDE Plasma also apps allowed during it), and apps whose popups are turned off in the desktop settings stay off the strip too. An update of a notification (a new status, more progress) replaces the one on the strip. Touch the strip to dismiss one.
- **Hide message text** shows only the application's name and icon, without the text or the notification's picture.
- **Show for up to** sets how long a message stays (2 to 15 seconds). A notification that asks for less time, such as a short status message, leaves sooner.
- **Never show from** takes application names, separated by commas: the name shown on the strip, the app's menu name or its desktop file id.
- **Show volume changes** and **Show new tracks** control the short pop-ups for volume and track changes.
- **Show a test message** sends a message to the strip.

### Incoming webhooks

Any program that can send a web request can put a card into the Headlines widget. Send a `POST` or `PUT` request to:

```text
http://127.0.0.1:19777/api/sensors/webhook/<id>
```

```bash
curl -X POST http://127.0.0.1:19777/api/sensors/webhook/build \
  -H 'Content-Type: application/json' \
  -d '{"message":"Build passed","url":"https://ci.example"}'
```

- Studio receives webhooks, so it has to be running.
- Add the webhook name under **Connections** first; requests for names that are not set up are refused (`404`).
- Only programs on this computer can deliver: requests from web pages (with an `Origin` header or `Sec-Fetch-Site: cross-site`) are refused, and a body may be at most 64 KiB.
- Each `<id>` is one card; a new request replaces it. Letters are lower-cased; characters other than letters, digits, `-` and `_` become `-`.
- The body can be JSON, XML or plain text. For JSON, the card shows the first of the fields `message`, `status` and `text`; other JSON bodies appear as raw JSON. Tapping the card opens the `url`, `link` or `href` field.
- Hold the card on the strip to delete it, or delete it in Studio.

### MQTT and IFTTT

Both lists hold targets that receive the current headline card: an MQTT broker (optionally with Home Assistant discovery) or an IFTTT Maker webhook. Add an entry, fill in the fields, **Save**, then **Test**. In this release a card is sent only when you press **Test**.

## Device

- **Status**: connection state, device path, profile and page on the strip, a brightness slider, and **Turn off** or **Turn on**. **Start daemon**, **Restart** and **Stop** run `systemctl --user` for `linexusd.service`; **Start with my session** enables or disables it.
- **Behaviour**: page change animation (Slide, Fade, Instant), language on the strip, 24 or 12 hour clock, °C or °F, dimming after an idle time and the dimmed brightness, "First touch only wakes", turning off after an idle time, and "Off while the screen is locked". These are the `settings` fields in [configuration.md](configuration.md#settings).
- **Setup checks**: the strip, the udev rule, the daemon, autostart, the input helper, the desktop tools (`xdg-open`, `wpctl` or `pactl`, `busctl`, `curl`), the GNOME focus extension on GNOME, and OpenLinkHub. Each problem comes with a command you can copy.
- **Recent events**: the daemon's event log.

The brightness slider here changes the strip immediately and is kept across daemon restarts, but it does not change the profile's brightness. Use the slider in the profile's **Look** panel for that.

## Settings

- **Studio**: interface language (English or Deutsch) and appearance (System, Dark, Light). Both are stored in the browser.
- **Backup**: **Export backup** downloads `linexus-backup.nexuspack` with all profiles, pages and images. When buttons send web requests or pass arguments to programs, a notice reminds you that keys or tokens in them travel with the file. **Import backup** first shows what the backup contains and lists every button that acts outside the strip (runs programs, types text, sends web requests, opens apps, files or links, calls Home Assistant); only **Replace configuration** applies it. The previous configuration is kept as `config.json.bak`. Feeds and connections are not part of a backup. The section also shows the path of the configuration file.
- **Reset all pages** replaces the whole configuration, profiles and behaviour settings included, with the built-in default after you confirm it in a dialog. The previous configuration is kept as `config.json.bak`.
- **About**: version, credits and the trademark note.

## Security model

Studio can change your configuration and start programs through actions, so it accepts requests only from its own page on this computer:

- **Loopback only.** Studio binds to `127.0.0.1` by default and refuses to start on an address that is not a loopback address.
- **Host check.** Every request must name `127.0.0.1`, `localhost` or `[::1]` with Studio's port in its `Host` header; anything else gets status 421. A web page whose domain resolves to `127.0.0.1` still sends its own name and is refused.
- **Write token.** Each start of Studio creates a random token and places it in the Studio page. Every `POST`, `PUT` and `DELETE` must carry the headers `X-Nexus-Studio: 1` and `X-Nexus-Studio-Token`; otherwise Studio answers 403. Other websites can neither read the token nor add these headers, because Studio never allows cross-origin requests.
- **Content Security Policy.** Every response carries `default-src 'self'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; script-src 'self'; connect-src 'self'; font-src 'self'` and `X-Content-Type-Options: nosniff`. The interface is built into the binary and loads nothing from other hosts.
- **Limits.** One request per connection and at most 32 connections at a time (more get status 503); a request must arrive completely within 30 seconds; headers up to 64 KiB, bodies up to 16 MiB (webhook deliveries 64 KiB); a saved configuration up to 15 MiB.
- **No lost updates.** A save names the revision of the configuration it started from; if the file changed since, Studio answers 409 and the page asks what to do.

The webhook endpoint is the one exception to the token rule: `POST` and `PUT` to `/api/sensors/webhook/<id>` work without the token, so other local programs can deliver cards. They still pass the Host check, are refused when they come from a web page (an `Origin` header without the token, or `Sec-Fetch-Site: cross-site` or `same-site`), are accepted only for webhook names set up under **Connections**, and may carry at most 64 KiB. Names are matched the way they are stored, so `/webhook/Build` reaches the webhook `build`. A delivery only stores the card; deleting cards needs the token. Because Studio listens on loopback only, programs on other computers cannot reach it unless you forward the port yourself.
