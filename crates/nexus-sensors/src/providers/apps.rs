//! Desktop integration for launcher pages: the installed applications
//! (from .desktop files), which of them are running, and their icons.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppEntry {
    /// Desktop file id, e.g. `org.kde.konsole.desktop`; what `gtk-launch`,
    /// `kioclient exec` or `gio launch` start.
    pub id: String,
    /// Name in the user's language when the entry has a translation.
    pub name: String,
    /// Icon theme name or absolute path; see [`resolve_icon`].
    pub icon: Option<String>,
    /// Basename of the program `Exec` starts (`konsole`, `flatpak`), used to
    /// spot running instances. None when it cannot identify the app because
    /// many entries share one program: URL launchers (`steam
    /// steam://rungameid/…`) and browser web apps (`brave --app-id=…`).
    pub exec_program: Option<String>,
    pub categories: Vec<String>,
}

/// Every application a desktop menu would show, sorted by name. Follows the
/// XDG rules: `$XDG_DATA_HOME/applications` first, then each
/// `$XDG_DATA_DIRS/applications` plus the Flatpak export directories; the
/// first file for a desktop id wins (a user copy with `Hidden=true` hides
/// the system entry). Entries with `NoDisplay`, a missing `TryExec` program,
/// another `Type` or `OnlyShowIn`/`NotShowIn` excluding this desktop are left
/// out.
pub fn list_applications() -> Vec<AppEntry> {
    let dirs: Vec<PathBuf> = data_dirs()
        .into_iter()
        .map(|dir| dir.join("applications"))
        .collect();
    list_applications_in(&dirs, &locale_candidates(), Some(&current_desktops()))
}

/// Every installed application entry, including those kept out of menus
/// (`NoDisplay`, `OnlyShowIn` another desktop), such as the helpers that
/// send notifications. Deleted (`Hidden`) entries and those whose `TryExec`
/// program is missing are left out, as in [`list_applications`].
pub fn list_desktop_entries() -> Vec<AppEntry> {
    let dirs: Vec<PathBuf> = data_dirs()
        .into_iter()
        .map(|dir| dir.join("applications"))
        .collect();
    list_applications_in(&dirs, &locale_candidates(), None)
}

/// Basenames of the programs the current user is running: the executable
/// (`/proc/<pid>/exe`) and the kernel's process name (`comm`, at most 15
/// characters, which is what scripts show up as).
pub fn running_programs() -> HashSet<String> {
    scan_processes(Path::new("/proc")).programs
}

/// Desktop ids (and program names) of the `apps` that are running.
///
/// An app counts as running when
/// - a process lives in the systemd scope its launcher created: Plasma and
///   GNOME start menu apps in `app-<desktop id>@….service` / `app-gnome-…`
///   units and Flatpak always uses `app-flatpak-<app id>-<pid>.scope`, so
///   this is exact for anything started from the desktop; or
/// - its `exec_program` is among [`running_programs`] (apps started from a
///   terminal), unless that program is a generic wrapper like `env`, `sh` or
///   `flatpak`; or
/// - for Flatpak entries on sessions without systemd app units at all, a
///   process is named like the last part of the app id
///   (`org.mozilla.firefox` → `firefox`). This is a loose guess that an
///   unrelated program of that name can fool, so it is skipped whenever
///   scopes exist (Flatpak then always has one while running).
pub fn running_targets(apps: &[AppEntry]) -> HashSet<String> {
    match_running(apps, &scan_processes(Path::new("/proc")))
}

/// Path of a PNG or SVG for an icon name at `size` pixels, following the
/// user's icon theme (KDE `kdeglobals` or GTK `settings.ini`), then breeze,
/// Adwaita and hicolor, then the unthemed pixmaps. Within a theme an exact
/// size wins, then a scalable SVG, then the next larger size, then a smaller
/// one; HiDPI (`@2x`) directories come last. Absolute paths and `file://` URLs are returned when the file exists.
/// Results are cached for a few minutes, so calling this per frame is cheap.
pub fn resolve_icon(icon: &str, size: u32) -> Option<PathBuf> {
    let icon = icon.trim();
    if icon.is_empty() {
        return None;
    }
    if let Some(path) = super::media::file_url_path(icon) {
        return path.is_file().then_some(path);
    }
    if icon.starts_with('/') {
        let path = PathBuf::from(icon);
        return path.is_file().then_some(path);
    }
    if icon.contains('/') {
        return None;
    }
    let key = (icon.to_string(), size);
    let cache = ICON_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((path, at)) = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
    {
        // A cached hit is only reused while the file is still there.
        let file_gone = matches!(path, Some(path) if !path.is_file());
        if at.elapsed() < ICON_CACHE_TTL && !file_gone {
            return path.clone();
        }
    }
    let bases = icon_base_dirs();
    let themes = icon_themes(&bases);
    let pixmaps: Vec<PathBuf> = data_dirs()
        .into_iter()
        .map(|dir| dir.join("pixmaps"))
        .collect();
    let found = lookup_icon(icon, size.max(1), &bases, &themes, &pixmaps);
    let mut cache = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if cache.len() > 2048 {
        cache.clear();
    }
    cache.insert(key, (found.clone(), Instant::now()));
    found
}

/// `(icon, size)` → lookup result and when it was made.
type IconCache = HashMap<(String, u32), (Option<PathBuf>, Instant)>;

static ICON_CACHE: OnceLock<Mutex<IconCache>> = OnceLock::new();
const ICON_CACHE_TTL: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------------------
// Desktop entries

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DesktopEntry {
    kind: String,
    name: String,
    icon: Option<String>,
    exec: Option<String>,
    try_exec: Option<String>,
    categories: Vec<String>,
    no_display: bool,
    hidden: bool,
    only_show_in: Vec<String>,
    not_show_in: Vec<String>,
}

impl DesktopEntry {
    fn shown_on(&self, desktops: &[String]) -> bool {
        let on_desktop = |list: &[String]| {
            list.iter()
                .any(|item| desktops.iter().any(|d| d.eq_ignore_ascii_case(item)))
        };
        self.kind == "Application"
            && !self.no_display
            && !self.hidden
            && (self.only_show_in.is_empty() || on_desktop(&self.only_show_in))
            && !on_desktop(&self.not_show_in)
    }

    fn into_app(self, id: String) -> AppEntry {
        AppEntry {
            id,
            name: self.name,
            icon: self.icon,
            exec_program: self.exec.as_deref().and_then(exec_program),
            categories: self.categories,
        }
    }
}

/// `desktops`: the current desktop names, to list only what its menu shows;
/// None lists every application entry.
fn list_applications_in(
    dirs: &[PathBuf],
    locale: &[String],
    desktops: Option<&[String]>,
) -> Vec<AppEntry> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for dir in dirs {
        let mut files = Vec::new();
        collect_desktop_files(dir, dir, 0, &mut files);
        files.sort();
        for (id, path) in files {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Some(entry) = fs::read_to_string(&path)
                .ok()
                .and_then(|text| parse_desktop_entry(&text, locale))
            else {
                continue;
            };
            let listed = match desktops {
                Some(desktops) => entry.shown_on(desktops),
                None => entry.kind == "Application" && !entry.hidden,
            };
            if !listed {
                continue;
            }
            if entry
                .try_exec
                .as_deref()
                .is_some_and(|program| !program_exists(program))
            {
                continue;
            }
            apps.push(entry.into_app(id));
        }
    }
    apps.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    apps
}

/// Desktop ids are paths below `applications/` with `/` turned into `-`
/// (`kde4/foo.desktop` → `kde4-foo.desktop`).
fn collect_desktop_files(root: &Path, dir: &Path, depth: u8, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth < 3 {
                collect_desktop_files(root, &path, depth + 1, out);
            }
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("desktop") {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let id = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("-");
        out.push((id, path));
    }
}

/// Reads the `[Desktop Entry]` group; other groups (desktop actions) are
/// ignored. `locale` lists the `Name[…]` suffixes to try, best first.
fn parse_desktop_entry(text: &str, locale: &[String]) -> Option<DesktopEntry> {
    let mut fields: HashMap<&str, &str> = HashMap::new();
    let mut in_group = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            if in_group {
                break;
            }
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            fields.entry(key.trim()).or_insert(value.trim());
        }
    }
    let text = |key: &str| {
        fields
            .get(key)
            .map(|value| unescape_value(value))
            .filter(|value| !value.is_empty())
    };
    let list = |key: &str| {
        text(key)
            .map(|value| {
                value
                    .split(';')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let flag = |key: &str| text(key).is_some_and(|value| value.eq_ignore_ascii_case("true"));
    let name = locale
        .iter()
        .find_map(|tag| text(&format!("Name[{tag}]")))
        .or_else(|| text("Name"))?;
    Some(DesktopEntry {
        kind: text("Type")?,
        name,
        icon: text("Icon"),
        exec: text("Exec"),
        try_exec: text("TryExec"),
        categories: list("Categories"),
        no_display: flag("NoDisplay"),
        hidden: flag("Hidden"),
        only_show_in: list("OnlyShowIn"),
        not_show_in: list("NotShowIn"),
    })
}

fn unescape_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            // `\;` in lists and anything unknown stay as written.
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Programs that only start something else; matching them would light up
/// every app that uses them.
const WRAPPER_PROGRAMS: &[&str] = &[
    "flatpak",
    "env",
    "sh",
    "bash",
    "dash",
    "zsh",
    "wine",
    "snap",
    "gio",
    "xdg-open",
    "kioclient",
    "kioclient5",
    "kde-open",
    "kde-open5",
    "systemd-run",
    "gtk-launch",
    "gtk4-launch",
    "electron",
    "appimage-run",
    "distrobox-enter",
    "toolbox",
];

const INTERPRETERS: &[&str] = &[
    "python", "python2", "python3", "perl", "ruby", "node", "gjs",
];

fn exec_program(exec: &str) -> Option<String> {
    program_from_tokens(&split_exec(exec), true)
}

fn program_from_tokens(tokens: &[String], allow_shell: bool) -> Option<String> {
    let mut tokens = tokens;
    if !allow_shell {
        // Inside `sh -c "…"`: the last command of a chain is the app
        // (`cd ~ && exec code`), and `exec` only replaces the shell.
        if let Some(pos) = tokens
            .iter()
            .rposition(|token| matches!(token.as_str(), "&&" | "||" | ";"))
        {
            tokens = &tokens[pos + 1..];
        }
        if tokens.first().is_some_and(|token| token == "exec") {
            tokens = &tokens[1..];
        }
    }
    let mut idx = 0;
    if tokens.first().map(|token| basename(token)) == Some("env") {
        idx = 1;
        while tokens
            .get(idx)
            .is_some_and(|token| token.starts_with('-') || token.contains('='))
        {
            idx += 1;
        }
    }
    let program = basename(tokens.get(idx)?);
    let args = &tokens[idx + 1..];
    if args
        .iter()
        .any(|arg| arg.contains("://") || arg.starts_with("--app-id="))
    {
        return None;
    }
    let is_shell = matches!(program, "sh" | "bash" | "dash" | "zsh");
    if is_shell && allow_shell {
        if let Some(pos) = args.iter().position(|arg| arg == "-c") {
            let command = args.get(pos + 1)?;
            return program_from_tokens(&split_exec(command), false);
        }
    }
    let is_interpreter = INTERPRETERS.contains(&program)
        || program
            .strip_prefix("python")
            .is_some_and(|version| version.bytes().all(|b| b.is_ascii_digit() || b == b'.'));
    if is_shell || is_interpreter {
        // `python3 /usr/share/app/app.py`: the script names the app.
        if let Some(script) = args
            .iter()
            .find(|arg| !arg.starts_with('-') && !arg.starts_with('%'))
        {
            return Some(basename(script).to_string());
        }
    }
    (!program.is_empty()).then(|| program.to_string())
}

/// Splits an `Exec` value into arguments: whitespace separates, double (and,
/// for `sh -c` strings, single) quotes group, backslash escapes inside double
/// quotes.
fn split_exec(exec: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut quote: Option<char> = None;
    let mut chars = exec.chars();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (None, '"' | '\'') => {
                quote = Some(ch);
                has_token = true;
            }
            (Some(open), _) if ch == open => quote = None,
            (Some('"'), '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
            }
            (None, ch) if ch.is_whitespace() => {
                if has_token {
                    tokens.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            (_, ch) => {
                current.push(ch);
                has_token = true;
            }
        }
    }
    if has_token {
        tokens.push(current);
    }
    tokens
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn program_exists(program: &str) -> bool {
    if program.contains('/') {
        Path::new(program).is_file()
    } else {
        crate::which(program).is_some()
    }
}

// ---------------------------------------------------------------------------
// Environment

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home().map(|home| home.join(".local/share")))
        .unwrap_or_else(|| PathBuf::from(".local/share"))
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// XDG data directories in precedence order, Flatpak exports included even
/// when the session did not add them to `$XDG_DATA_DIRS`.
fn data_dirs() -> Vec<PathBuf> {
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let mut dirs = vec![data_home()];
    dirs.extend(
        system
            .split(':')
            .map(PathBuf::from)
            .filter(|path| path.is_absolute()),
    );
    dirs.push(data_home().join("flatpak/exports/share"));
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share"));
    let mut seen = HashSet::new();
    dirs.retain(|dir| seen.insert(dir.clone()));
    dirs
}

/// `Name[…]` suffixes for the UI language, best first: `de_DE.UTF-8` gives
/// `de_DE`, then `de`.
fn locale_candidates() -> Vec<String> {
    let raw = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|value| !value.is_empty()))
        .unwrap_or_default();
    parse_locale(&raw)
}

fn parse_locale(raw: &str) -> Vec<String> {
    let (base, modifier) = match raw.split_once('@') {
        Some((base, modifier)) => (base, Some(modifier)),
        None => (raw, None),
    };
    let base = base.split('.').next().unwrap_or_default();
    if base.is_empty() || base == "C" || base == "POSIX" {
        return Vec::new();
    }
    let lang = base.split('_').next().unwrap_or(base);
    let mut out = Vec::new();
    if let Some(modifier) = modifier {
        out.push(format!("{base}@{modifier}"));
    }
    out.push(base.to_string());
    if lang != base {
        if let Some(modifier) = modifier {
            out.push(format!("{lang}@{modifier}"));
        }
        out.push(lang.to_string());
    }
    out
}

fn current_desktops() -> Vec<String> {
    std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .split(':')
        .map(str::trim)
        .filter(|desktop| !desktop.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

// ---------------------------------------------------------------------------
// Running processes

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ProcessScan {
    programs: HashSet<String>,
    /// Desktop ids (without `.desktop`) from systemd `app-…` units.
    app_scopes: HashSet<String>,
}

fn scan_processes(proc_root: &Path) -> ProcessScan {
    let mut scan = ProcessScan::default();
    let uid = fs::metadata(proc_root.join("self")).map(|m| m.uid()).ok();
    let Ok(entries) = fs::read_dir(proc_root) else {
        return scan;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name
            .to_str()
            .is_some_and(|name| !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let path = entry.path();
        // Other users' processes are not "my apps" (and mostly unreadable).
        if uid.is_some() && fs::metadata(&path).map(|m| m.uid()).ok() != uid {
            continue;
        }
        if let Ok(comm) = fs::read_to_string(path.join("comm")) {
            let comm = comm.trim_end_matches('\n');
            if !comm.is_empty() {
                scan.programs.insert(comm.to_string());
            }
        }
        if let Ok(exe) = fs::read_link(path.join("exe")) {
            if let Some(file) = exe.file_name() {
                let file = file.to_string_lossy();
                let file = file.strip_suffix(" (deleted)").unwrap_or(&file);
                scan.programs.insert(file.to_string());
            }
        }
        if let Ok(cgroup) = fs::read_to_string(path.join("cgroup")) {
            for line in cgroup.lines() {
                if let Some(id) = line.rsplit('/').next().and_then(parse_app_unit) {
                    scan.app_scopes.insert(id);
                }
            }
        }
    }
    scan
}

/// The desktop id stem a launcher encoded into its systemd unit:
/// `app-org.kde.dolphin@f4e4….service`, `app-brave\x2dbrowser@….service`,
/// `app-flatpak-org.mozilla.firefox-1234.scope`,
/// `app-gnome-org.gnome.Nautilus-5678.scope`.
fn parse_app_unit(unit: &str) -> Option<String> {
    let rest = unit.strip_prefix("app-")?;
    let (stem, is_scope) = match rest.strip_suffix(".scope") {
        Some(stem) => (stem, true),
        None => (rest.strip_suffix(".service")?, false),
    };
    // Literal dashes inside the id are escaped as \x2d, so a raw dash
    // separates the launcher tag and the trailing instance number.
    let stem = match stem.split_once('@') {
        Some((before, _)) => before,
        None if is_scope => stem.rsplit_once('-').map_or(stem, |(before, _)| before),
        None => stem,
    };
    let stem = stem
        .strip_prefix("flatpak-")
        .or_else(|| stem.strip_prefix("gnome-"))
        .unwrap_or(stem);
    let id = unescape_systemd(stem);
    (!id.is_empty()).then_some(id)
}

fn unescape_systemd(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'\\' && bytes.get(idx + 1) == Some(&b'x') {
            if let Some(byte) = bytes
                .get(idx + 2..idx + 4)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                out.push(byte);
                idx += 4;
                continue;
            }
        }
        out.push(bytes[idx]);
        idx += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn match_running(apps: &[AppEntry], scan: &ProcessScan) -> HashSet<String> {
    let lowercase: HashSet<String> = scan
        .programs
        .iter()
        .map(|program| program.to_lowercase())
        .collect();
    let mut targets = HashSet::new();
    for app in apps {
        let stem = app.id.strip_suffix(".desktop").unwrap_or(&app.id);
        let program = app.exec_program.as_deref();
        let by_program = program
            .filter(|program| !WRAPPER_PROGRAMS.contains(program))
            .is_some_and(|program| program_running(program, &scan.programs));
        let by_flatpak_name = program == Some("flatpak")
            && scan.app_scopes.is_empty()
            && stem
                .rsplit('.')
                .next()
                .is_some_and(|last| lowercase.contains(&last.to_lowercase()));
        if scan.app_scopes.contains(stem) || by_program || by_flatpak_name {
            targets.insert(app.id.clone());
            if let (true, Some(program)) = (by_program, program) {
                targets.insert(program.to_string());
            }
        }
    }
    targets
}

/// `comm` is cut to 15 bytes, so long script names only match their prefix.
fn program_running(program: &str, running: &HashSet<String>) -> bool {
    running.contains(program)
        || (program.len() > 15
            && program
                .get(..15)
                .is_some_and(|short| running.contains(short)))
}

// ---------------------------------------------------------------------------
// Icons

fn icon_base_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![data_home().join("icons")];
    if let Some(home) = home() {
        dirs.push(home.join(".icons"));
    }
    dirs.extend(data_dirs().into_iter().map(|dir| dir.join("icons")));
    let mut seen = HashSet::new();
    dirs.retain(|dir| seen.insert(dir.clone()));
    dirs
}

/// Theme search order: the user's theme and what it inherits, then breeze
/// and Adwaita (with their parents), hicolor last as the spec requires.
fn icon_themes(bases: &[PathBuf]) -> Vec<String> {
    let mut themes = Vec::new();
    for theme in user_icon_theme()
        .into_iter()
        .chain(["breeze".to_string(), "Adwaita".to_string()])
    {
        add_theme_with_parents(&theme, bases, &mut themes, 0);
    }
    themes.push("hicolor".into());
    themes
}

fn add_theme_with_parents(theme: &str, bases: &[PathBuf], out: &mut Vec<String>, depth: u8) {
    if theme == "hicolor" || out.iter().any(|known| known == theme) || depth > 8 {
        return;
    }
    out.push(theme.to_string());
    let Some(index) = bases
        .iter()
        .find_map(|base| fs::read_to_string(base.join(theme).join("index.theme")).ok())
    else {
        return;
    };
    if let Some(parents) = ini_value(&index, "Icon Theme", "Inherits") {
        for parent in parents.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            add_theme_with_parents(parent, bases, out, depth + 1);
        }
    }
}

fn user_icon_theme() -> Option<String> {
    let config = crate::config_home();
    let kde = fs::read_to_string(config.join("kdeglobals"))
        .ok()
        .and_then(|text| ini_value(&text, "Icons", "Theme"));
    if kde.is_some() {
        return kde;
    }
    // Plasma without an explicit choice uses breeze, which comes next anyway;
    // the GTK setting there only describes GTK apps.
    if current_desktops()
        .iter()
        .any(|desktop| desktop.eq_ignore_ascii_case("KDE"))
    {
        return None;
    }
    fs::read_to_string(config.join("gtk-3.0/settings.ini"))
        .ok()
        .and_then(|text| ini_value(&text, "Settings", "gtk-icon-theme-name"))
}

pub(crate) fn ini_value(text: &str, section: &str, key: &str) -> Option<String> {
    let header = format!("[{section}]");
    let mut in_section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == header;
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((name, value)) = line.split_once('=') {
            if name.trim() == key {
                let value = value.trim().trim_matches('"');
                return (!value.is_empty()).then(|| value.to_string());
            }
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct IconRank {
    /// HiDPI directories (`48x48@2`, breeze `16@3x`) are drawn for scaled
    /// screens; the panel is 1x, so they only serve when nothing else fits.
    scaled: bool,
    /// 0 exact size, 1 scalable, 2 larger, 3 smaller.
    tier: u8,
    distance: u32,
    other_context: bool,
}

fn lookup_icon(
    icon: &str,
    size: u32,
    bases: &[PathBuf],
    themes: &[String],
    pixmaps: &[PathBuf],
) -> Option<PathBuf> {
    let stem = [".png", ".svg", ".xpm"]
        .iter()
        .find_map(|ext| icon.strip_suffix(ext))
        .unwrap_or(icon);
    for theme in themes {
        let mut best: Option<(IconRank, PathBuf)> = None;
        for root in bases.iter().map(|base| base.join(theme)) {
            for (rank, path) in theme_candidates(&root, stem, size) {
                let better = match &best {
                    Some((best_rank, _)) => rank < *best_rank,
                    None => true,
                };
                if better {
                    best = Some((rank, path));
                }
            }
        }
        if let Some((_, path)) = best {
            return Some(path);
        }
    }
    // Unthemed icons: pixmaps and loose files in the icon base directories.
    for dir in pixmaps.iter().chain(bases) {
        for ext in ["png", "svg"] {
            let path = dir.join(format!("{stem}.{ext}"));
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// Every file named `stem.png`/`stem.svg` in a theme, in both layouts:
/// `48x48/apps/…` (hicolor, Adwaita) and `apps/48/…` (breeze).
fn theme_candidates(root: &Path, stem: &str, size: u32) -> Vec<(IconRank, PathBuf)> {
    let mut out = Vec::new();
    let Ok(top) = fs::read_dir(root) else {
        return out;
    };
    for top in top.flatten() {
        let top_name = top.file_name().to_string_lossy().into_owned();
        let top_path = top.path();
        let Ok(inner) = fs::read_dir(&top_path) else {
            continue;
        };
        let top_size = parse_size_dir(&top_name);
        for inner in inner.flatten() {
            let inner_name = inner.file_name().to_string_lossy().into_owned();
            let (dir_size, context) = match top_size {
                Some(dir_size) => (dir_size, inner_name.as_str()),
                None => match parse_size_dir(&inner_name) {
                    Some(dir_size) => (dir_size, top_name.as_str()),
                    None => continue,
                },
            };
            for ext in ["png", "svg"] {
                let path = inner.path().join(format!("{stem}.{ext}"));
                if path.is_file() {
                    out.push((rank_icon(dir_size, context, size), path));
                }
            }
        }
    }
    out
}

/// `(pixel size, scale)`; size None for `scalable`. Accepts `48x48`,
/// `48x48@2`, `48`, `48@2x`. Symbolic (monochrome) icons are skipped.
fn parse_size_dir(name: &str) -> Option<(Option<u32>, u32)> {
    if name == "scalable" {
        return Some((None, 1));
    }
    let (size, scale) = match name.split_once('@') {
        Some((size, scale)) => (size, scale.trim_end_matches('x').parse().ok()?),
        None => (name, 1),
    };
    let size = match size.split_once('x') {
        Some((w, h)) if w == h => w,
        Some(_) => return None,
        None => size,
    };
    Some((Some(size.parse().ok()?), scale))
}

fn rank_icon((dir_size, scale): (Option<u32>, u32), context: &str, want: u32) -> IconRank {
    let other_context = context != "apps";
    let Some(dir_size) = dir_size else {
        return IconRank {
            tier: 1,
            distance: 0,
            other_context,
            scaled: false,
        };
    };
    let pixels = dir_size.saturating_mul(scale.max(1));
    let (tier, distance) = match pixels.cmp(&want) {
        std::cmp::Ordering::Equal => (0, 0),
        std::cmp::Ordering::Greater => (2, pixels - want),
        std::cmp::Ordering::Less => (3, want - pixels),
    };
    IconRank {
        tier,
        distance,
        other_context,
        scaled: scale > 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("nexus-apps-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn parses_desktop_entry_with_localized_name() {
        let text = "\
# comment
[Desktop Entry]
Type=Application
Name=Terminal
Name[de]=Terminal-Emulator
Name[de_AT]=Terminal (AT)
Icon=utilities-terminal
Exec=konsole %U
Categories=Qt;KDE;System;TerminalEmulator;
X-Custom=1

[Desktop Action NewWindow]
Name=New Window
Exec=konsole --new-window
";
        let entry = parse_desktop_entry(text, &["de_DE".into(), "de".into()]).unwrap();
        assert_eq!(entry.name, "Terminal-Emulator");
        assert_eq!(entry.exec.as_deref(), Some("konsole %U"));
        assert_eq!(
            entry.categories,
            vec!["Qt", "KDE", "System", "TerminalEmulator"]
        );
        let english = parse_desktop_entry(text, &[]).unwrap();
        assert_eq!(english.name, "Terminal");
        assert!(parse_desktop_entry("[Desktop Entry]\nName=x\n", &[]).is_none());
        assert!(parse_desktop_entry("Name=x\nType=Application\n", &[]).is_none());
    }

    #[test]
    fn extracts_the_program_behind_exec() {
        let cases = [
            ("/usr/bin/konsole %U", Some("konsole")),
            ("env GDK_BACKEND=x11 FOO=1 /opt/app/bin/app --flag %U", Some("app")),
            ("\"/opt/My App/run\" %f", Some("run")),
            (
                "/usr/bin/flatpak run --branch=stable --arch=x86_64 --command=firefox org.mozilla.firefox @@u %u @@",
                Some("flatpak"),
            ),
            ("sh -c \"cd ~ && exec code --new-window\"", Some("code")),
            ("bash -c 'exec /usr/bin/kitty -1'", Some("kitty")),
            ("steam steam://rungameid/570", None),
            (
                "/opt/brave-bin/brave --profile-directory=Default --app-id=pokmkoiooknndbddmgmaolnlgelpkhep",
                None,
            ),
            ("python3 /usr/share/foo/foo-gui.py %F", Some("foo-gui.py")),
            ("python3.12 -O /opt/x/main", Some("main")),
            ("", None),
        ];
        for (exec, program) in cases {
            assert_eq!(exec_program(exec).as_deref(), program, "{exec}");
        }
        assert_eq!(
            split_exec(r#"a "b \"c\" d" e\ f"#),
            vec!["a", "b \"c\" d", "e\\", "f"]
        );
    }

    #[test]
    fn lists_applications_with_precedence_and_filters() {
        let root = temp_root("list");
        let user = root.join("user/applications");
        let system = root.join("system/applications");
        let entry = |name: &str, extra: &str| {
            format!("[Desktop Entry]\nType=Application\nName={name}\nExec={name}\n{extra}")
        };
        write(
            &user.join("hidden.desktop"),
            &entry("Hidden", "Hidden=true\n"),
        );
        write(&system.join("hidden.desktop"), &entry("Hidden system", ""));
        write(
            &user.join("editor.desktop"),
            &entry("Editor (user)", "Icon=editor\n"),
        );
        write(&system.join("editor.desktop"), &entry("Editor", ""));
        write(&system.join("kde4/old.desktop"), &entry("Old", ""));
        write(
            &system.join("nodisplay.desktop"),
            &entry("NoDisplay", "NoDisplay=true\n"),
        );
        write(
            &system.join("gnome.desktop"),
            &entry("GnomeOnly", "OnlyShowIn=GNOME;\n"),
        );
        write(
            &system.join("notkde.desktop"),
            &entry("NotKde", "NotShowIn=KDE;\n"),
        );
        write(
            &system.join("kde.desktop"),
            &entry("KdeOnly", "OnlyShowIn=KDE;\n"),
        );
        write(
            &system.join("gone.desktop"),
            &entry("Gone", "TryExec=definitely-not-installed-4711\n"),
        );
        write(
            &system.join("link.desktop"),
            "[Desktop Entry]\nType=Link\nName=Link\n",
        );
        write(&system.join("readme.txt"), "not an entry");

        let dirs = [user, system];
        let apps = list_applications_in(&dirs, &[], Some(&["KDE".into()]));
        let all = list_applications_in(&dirs, &[], None);
        let _ = fs::remove_dir_all(&root);

        let all_ids: Vec<_> = all.iter().map(|app| app.id.as_str()).collect();
        assert_eq!(
            all_ids,
            vec![
                "editor.desktop",
                "gnome.desktop",
                "kde.desktop",
                "nodisplay.desktop",
                "notkde.desktop",
                "kde4-old.desktop",
            ]
        );

        let ids: Vec<_> = apps.iter().map(|app| app.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["editor.desktop", "kde.desktop", "kde4-old.desktop"]
        );
        assert_eq!(apps[0].name, "Editor (user)");
        assert_eq!(apps[0].icon.as_deref(), Some("editor"));
        assert_eq!(apps[0].exec_program.as_deref(), Some("Editor"));
    }

    #[test]
    fn parses_launcher_units() {
        let cases = [
            (
                "app-org.kde.dolphin@f4e44e502f6644a09b93842069b2886c.service",
                Some("org.kde.dolphin"),
            ),
            (
                "app-brave\\x2dbrowser@a3c35a30400d.service",
                Some("brave-browser"),
            ),
            (
                "app-org.chromium.Chromium-14802.scope",
                Some("org.chromium.Chromium"),
            ),
            (
                "app-flatpak-org.mozilla.firefox-12345.scope",
                Some("org.mozilla.firefox"),
            ),
            (
                "app-gnome-org.gnome.Nautilus-5678.scope",
                Some("org.gnome.Nautilus"),
            ),
            (
                "app-gnome\\x2dcalculator@1.service",
                Some("gnome-calculator"),
            ),
            ("session-3.scope", None),
            ("app.slice", None),
        ];
        for (unit, id) in cases {
            assert_eq!(parse_app_unit(unit).as_deref(), id, "{unit}");
        }
    }

    #[test]
    fn matches_running_apps_by_scope_program_and_flatpak_name() {
        let app = |id: &str, program: Option<&str>| AppEntry {
            id: id.into(),
            name: id.into(),
            icon: None,
            exec_program: program.map(Into::into),
            categories: Vec::new(),
        };
        let apps = [
            app("org.kde.dolphin.desktop", Some("dolphin")),
            app("org.kde.konsole.desktop", Some("konsole")),
            app("org.mozilla.firefox.desktop", Some("flatpak")),
            app("com.spotify.Client.desktop", Some("flatpak")),
            app("shell.desktop", Some("bash")),
            app("monitor.desktop", Some("plasma-systemmonitor")),
            app("kate.desktop", Some("kate")),
        ];
        let programs: HashSet<String> = ["konsole", "bash", "firefox", "plasma-systemmo"]
            .into_iter()
            .map(Into::into)
            .collect();
        let sorted = |scan: &ProcessScan| {
            let mut running: Vec<String> = match_running(&apps, scan).into_iter().collect();
            running.sort_unstable();
            running
        };

        // systemd session: scopes are authoritative for Flatpak, so a stray
        // `firefox` process does not light up the Flatpak entry.
        let with_scopes = ProcessScan {
            programs: programs.clone(),
            app_scopes: ["org.kde.dolphin"].into_iter().map(Into::into).collect(),
        };
        assert_eq!(
            sorted(&with_scopes),
            vec![
                "konsole",
                "monitor.desktop",
                "org.kde.dolphin.desktop",
                "org.kde.konsole.desktop",
                "plasma-systemmonitor",
            ]
        );

        // No systemd app units anywhere: fall back to the name guess.
        let without_scopes = ProcessScan {
            programs,
            app_scopes: HashSet::new(),
        };
        assert_eq!(
            sorted(&without_scopes),
            vec![
                "konsole",
                "monitor.desktop",
                "org.kde.konsole.desktop",
                "org.mozilla.firefox.desktop",
                "plasma-systemmonitor",
            ]
        );
    }

    #[test]
    fn scans_a_proc_tree() {
        let root = temp_root("proc");
        fs::create_dir_all(root.join("self")).unwrap();
        write(&root.join("101/comm"), "konsole\n");
        std::os::unix::fs::symlink("/usr/bin/konsole (deleted)", root.join("101/exe")).unwrap();
        write(
            &root.join("101/cgroup"),
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-org.kde.konsole@ab12.service\n",
        );
        write(&root.join("202/comm"), "kworker/0:1\n");
        write(&root.join("not-a-pid/comm"), "ignored\n");
        let scan = scan_processes(&root);
        let _ = fs::remove_dir_all(&root);
        assert!(scan.programs.contains("konsole"));
        assert!(scan.programs.contains("kworker/0:1"));
        assert!(!scan.programs.contains("ignored"));
        assert_eq!(scan.programs.len(), 2);
        assert!(scan.app_scopes.contains("org.kde.konsole"));
    }

    #[test]
    fn parses_locales() {
        assert_eq!(parse_locale("de_DE.UTF-8"), vec!["de_DE", "de"]);
        assert_eq!(
            parse_locale("sr_RS.UTF-8@latin"),
            vec!["sr_RS@latin", "sr_RS", "sr@latin", "sr"]
        );
        assert_eq!(parse_locale("de"), vec!["de"]);
        assert!(parse_locale("C.UTF-8").is_empty());
        assert!(parse_locale("").is_empty());
    }

    #[test]
    fn resolves_icons_by_theme_and_size() {
        let root = temp_root("icons");
        let base = root.join("icons");
        let pixmaps = root.join("pixmaps");
        write(&base.join("hicolor/16x16/apps/app.png"), "16");
        write(&base.join("hicolor/256x256/apps/app.png"), "256");
        write(&base.join("hicolor/128x128/apps/app.png"), "128");
        write(&base.join("hicolor/scalable/apps/vector.svg"), "svg");
        write(&base.join("hicolor/64x64/apps/vector.png"), "64");
        write(&base.join("hicolor/32x32@2/apps/hidpi.png"), "32@2");
        write(&base.join("hicolor/32x32@2/apps/both.png"), "32@2");
        write(&base.join("hicolor/96x96/apps/both.png"), "96");
        write(&base.join("breeze/apps/48/app.svg"), "breeze");
        write(
            &base.join("breeze/status/22/dialog-information.svg"),
            "status",
        );
        write(
            &base.join("breeze/index.theme"),
            "[Icon Theme]\nName=Breeze\n",
        );
        write(
            &base.join("custom/index.theme"),
            "[Icon Theme]\nName=Custom\nInherits=breeze,hicolor\n",
        );
        write(&pixmaps.join("legacy.png"), "pixmap");

        let bases = [base.clone()];
        let mut themes = Vec::new();
        add_theme_with_parents("custom", &bases, &mut themes, 0);
        themes.push("hicolor".into());
        assert_eq!(themes, vec!["custom", "breeze", "hicolor"]);

        let find = |icon: &str, size: u32| {
            lookup_icon(icon, size, &bases, &themes, std::slice::from_ref(&pixmaps))
                .map(|path| fs::read_to_string(path).unwrap())
        };
        // breeze comes before hicolor and has the exact size.
        assert_eq!(find("app", 48).as_deref(), Some("breeze"));
        // Only hicolor has it: exact, then scalable, then larger, then smaller.
        assert_eq!(find("vector", 64).as_deref(), Some("64"));
        assert_eq!(find("vector", 48).as_deref(), Some("svg"));
        // HiDPI directories only when no 1x size exists.
        assert_eq!(find("hidpi", 64).as_deref(), Some("32@2"));
        assert_eq!(find("both", 64).as_deref(), Some("96"));
        assert_eq!(find("dialog-information", 22).as_deref(), Some("status"));
        assert_eq!(find("legacy", 48).as_deref(), Some("pixmap"));
        assert_eq!(find("legacy.png", 48).as_deref(), Some("pixmap"));
        assert_eq!(find("missing", 48), None);

        let only_hicolor = ["hicolor".to_string()];
        let hicolor = |size| {
            lookup_icon("app", size, &bases, &only_hicolor, &[])
                .map(|path| fs::read_to_string(path).unwrap())
        };
        assert_eq!(hicolor(100).as_deref(), Some("128"));
        assert_eq!(hicolor(512).as_deref(), Some("256"));
        assert_eq!(hicolor(16).as_deref(), Some("16"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn parses_icon_size_directories() {
        assert_eq!(parse_size_dir("48x48"), Some((Some(48), 1)));
        assert_eq!(parse_size_dir("48x48@2"), Some((Some(48), 2)));
        assert_eq!(parse_size_dir("22@3x"), Some((Some(22), 3)));
        assert_eq!(parse_size_dir("scalable"), Some((None, 1)));
        assert_eq!(parse_size_dir("apps"), None);
        assert_eq!(parse_size_dir("symbolic"), None);
        assert_eq!(parse_size_dir("48x32"), None);
    }

    #[test]
    fn resolves_absolute_paths_directly() {
        let root = temp_root("abs");
        let file = root.join("icon.png");
        write(&file, "png");
        assert_eq!(resolve_icon(file.to_str().unwrap(), 48), Some(file.clone()));
        assert_eq!(
            resolve_icon(&format!("file://{}", file.display()), 48),
            Some(file.clone())
        );
        let _ = fs::remove_dir_all(&root);
        assert_eq!(resolve_icon(file.to_str().unwrap(), 48), None);
        assert_eq!(resolve_icon("relative/icon.png", 48), None);
        assert_eq!(resolve_icon("  ", 48), None);
    }
}
