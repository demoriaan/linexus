// Linexus focus, for GNOME Shell 45 and newer.
//
// Tells the Linexus daemon which app has keyboard focus, so that profiles of
// the Corsair iCUE Nexus can follow it: on every focus change it writes
// ["<app id>", "<window class>"] to $XDG_RUNTIME_DIR/linexus/gnome-focus.json.
// GNOME on Wayland gives this information only to shell extensions.
// linexusctl installs this file; extension-legacy.js is the same for 42 to 44.

import GLib from 'gi://GLib';
import Shell from 'gi://Shell';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

export default class LinexusFocus extends Extension {
    enable() {
        const dir = GLib.build_filenamev([GLib.get_user_runtime_dir(), 'linexus']);
        GLib.mkdir_with_parents(dir, 0o700);
        this._path = GLib.build_filenamev([dir, 'gnome-focus.json']);
        this._handler = global.display.connect('notify::focus-window', () => this._write());
        this._write();
    }

    disable() {
        global.display.disconnect(this._handler);
        this._handler = null;
        // No file means no focus information, for example while locked.
        GLib.unlink(this._path);
    }

    _write() {
        const window = global.display.focus_window;
        const app = window ? Shell.WindowTracker.get_default().get_window_app(window) : null;
        const focus = [app?.get_id() ?? '', window?.get_wm_class() ?? ''];
        try {
            GLib.file_set_contents(this._path, `${JSON.stringify(focus)}\n`);
        } catch (e) {
            console.warn(`Linexus focus: cannot write ${this._path}: ${e.message}`);
        }
    }
}
