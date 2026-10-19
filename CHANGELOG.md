# Changelog

All notable changes to this project are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses [semantic versioning](https://semver.org/).

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
