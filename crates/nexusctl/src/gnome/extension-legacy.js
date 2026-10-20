// Linexus focus, for GNOME Shell 42 to 44. extension.js is the same for 45
// and newer; see there for what it does.

/* exported init */
const {GLib, Shell} = imports.gi;

class LinexusFocus {
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
        GLib.unlink(this._path);
    }

    _write() {
        const window = global.display.focus_window;
        const app = window ? Shell.WindowTracker.get_default().get_window_app(window) : null;
        const focus = [app ? app.get_id() : '', window ? window.get_wm_class() || '' : ''];
        try {
            GLib.file_set_contents(this._path, `${JSON.stringify(focus)}\n`);
        } catch (e) {
            log(`Linexus focus: cannot write ${this._path}: ${e.message}`);
        }
    }
}

function init() {
    return new LinexusFocus();
}
