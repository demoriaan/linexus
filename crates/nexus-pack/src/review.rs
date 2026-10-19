//! What a pack would install, for review before it replaces the config.
//!
//! Packs are shared files, so their configs are untrusted. Most actions only
//! change what the strip shows; the ones listed here act on the computer
//! with the user's rights as soon as a button is tapped: they run programs,
//! type into the focused window (a line break presses Enter, so text typed
//! into a terminal runs as a command), send web requests or open things.

use crate::{load_checked, profile_summary, PackProfileSummary};
use anyhow::Result;
use nexus_config::{ActionSpec, NexusConfig, WidgetKind};
use serde::Serialize;
use std::path::Path;

/// Longest `detail` shown whole. Longer ones keep their start and end, and
/// the cut says how much is not shown, so nothing disappears silently.
const MAX_DETAIL_CHARS: usize = 1000;
const DETAIL_HEAD_CHARS: usize = 600;
const DETAIL_TAIL_CHARS: usize = 200;

/// Ways an action reaches outside the strip, most powerful first.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ReachKind {
    ShellCommand,
    CommandWidget,
    TextInput,
    KeyboardShortcut,
    HttpRequest,
    LaunchApp,
    OpenPath,
    OpenUrl,
    HomeAssistant,
}

impl ReachKind {
    /// What actions of this kind can do, in plain words.
    pub fn describe(self) -> &'static str {
        match self {
            Self::ShellCommand => "Run programs on this computer",
            Self::CommandWidget => "Run programs again and again to show their output",
            Self::TextInput => "Type text into the focused window (a line break presses Enter)",
            Self::KeyboardShortcut => "Press keyboard shortcuts",
            Self::HttpRequest => "Send web requests",
            Self::LaunchApp => "Start applications",
            Self::OpenPath => "Open files and folders",
            Self::OpenUrl => "Open web pages",
            Self::HomeAssistant => "Control Home Assistant devices",
        }
    }
}

/// One action (or command widget) that reaches outside the strip.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Reach {
    pub kind: ReachKind,
    /// Profile and page names, widget id: where the button sits.
    pub profile: String,
    pub page: String,
    pub widget: String,
    /// `tap`, `double_tap` or `hold`; absent for command widgets, which run
    /// on their own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gesture: Option<&'static str>,
    /// The action is a step of a `sequence`.
    pub in_sequence: bool,
    /// What exactly it does: command line, URL, keys or text.
    pub detail: String,
}

/// Every action in `config` that reaches outside the strip, grouped by kind
/// (most powerful first) and otherwise in config order.
pub fn outside_reach(config: &NexusConfig) -> Vec<Reach> {
    let mut found = Vec::new();
    for profile in &config.profiles {
        for page in &profile.pages {
            for widget in &page.widgets {
                // Every field comes from the pack, so none may carry escape
                // sequences or invisible characters that could hide or fake
                // lines in a terminal or reorder text in the browser.
                let at = |kind, gesture, in_sequence, detail: String| Reach {
                    kind,
                    profile: visible_text(&profile.name),
                    page: visible_text(&page.name),
                    widget: visible_text(&widget.id),
                    gesture,
                    in_sequence,
                    detail: shorten(&visible_text(&detail)),
                };
                if let WidgetKind::Command(props) = &widget.kind {
                    let line = command_line(&props.program, &props.args);
                    found.push(at(ReachKind::CommandWidget, None, false, line));
                }
                let gestures = [
                    ("tap", &widget.tap),
                    ("double_tap", &widget.double_tap),
                    ("hold", &widget.hold),
                ];
                for (gesture, action) in gestures {
                    if let Some(action) = action {
                        visit(action, false, &mut |kind, in_sequence, detail| {
                            found.push(at(kind, Some(gesture), in_sequence, detail));
                        });
                    }
                }
            }
        }
    }
    found.sort_by_key(|reach| reach.kind);
    found
}

fn visit(action: &ActionSpec, in_sequence: bool, emit: &mut impl FnMut(ReachKind, bool, String)) {
    let (kind, detail) = match action {
        ActionSpec::ShellCommand { program, args } => {
            (ReachKind::ShellCommand, command_line(program, args))
        }
        ActionSpec::TextInput { text } => (ReachKind::TextInput, text.clone()),
        ActionSpec::KeyboardShortcut { keys } => (ReachKind::KeyboardShortcut, keys.join("+")),
        ActionSpec::HttpRequest {
            method, url, body, ..
        } => {
            let method = serde_json::to_value(method)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default();
            let detail = match body {
                Some(body) => format!("{method} {url} with body {body}"),
                None => format!("{method} {url}"),
            };
            (ReachKind::HttpRequest, detail)
        }
        ActionSpec::LaunchApp { target } => (ReachKind::LaunchApp, target.clone()),
        ActionSpec::OpenPath { path } => (ReachKind::OpenPath, path.clone()),
        ActionSpec::OpenUrl { url } => (ReachKind::OpenUrl, url.clone()),
        ActionSpec::HomeAssistant { service, entity_id } => {
            let detail = match entity_id {
                Some(entity) => format!("{service} {entity}"),
                None => service.clone(),
            };
            (ReachKind::HomeAssistant, detail)
        }
        ActionSpec::Sequence { steps, .. } => {
            for step in steps {
                visit(step, true, emit);
            }
            return;
        }
        // Listed one by one, so a new action type has to be sorted in here.
        ActionSpec::Media { .. }
        | ActionSpec::Volume { .. }
        | ActionSpec::MicrophoneMute
        | ActionSpec::NextPage
        | ActionSpec::PreviousPage
        | ActionSpec::GoToPage { .. }
        | ActionSpec::GoBack
        | ActionSpec::SwitchProfile { .. }
        | ActionSpec::NextProfile
        | ActionSpec::PreviousProfile
        | ActionSpec::Brightness { .. }
        | ActionSpec::DisplayOff
        | ActionSpec::LockScreen => return,
    };
    emit(kind, in_sequence, detail);
}

/// Program and arguments as one line; arguments with spaces are quoted.
fn command_line(program: &str, args: &[String]) -> String {
    let mut line = program.to_string();
    for arg in args {
        line.push(' ');
        if arg.is_empty() || arg.contains(char::is_whitespace) {
            line.push_str(&format!("{arg:?}"));
        } else {
            line.push_str(arg);
        }
    }
    line
}

/// Shows line breaks, control characters (C0, C1, escape sequences) and
/// invisible formatting characters (bidi overrides, zero-width marks) as
/// escapes, so typed text that presses Enter is visible as such and no
/// field can rewrite or reorder what is printed around it.
pub fn visible_text(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '\n' => "\\n".to_string(),
            '\r' => "\\r".to_string(),
            '\t' => "\\t".to_string(),
            ch if ch.is_control() || is_invisible_format(ch) => {
                format!("\\u{{{:x}}}", u32::from(ch))
            }
            ch => ch.to_string(),
        })
        .collect()
}

/// Unicode format characters that change how text around them is shown.
fn is_invisible_format(ch: char) -> bool {
    matches!(
        ch,
        '\u{ad}'
            | '\u{61c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
    )
}

/// Keeps long details readable without hiding that something was left out.
fn shorten(text: &str) -> String {
    let count = text.chars().count();
    if count <= MAX_DETAIL_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(DETAIL_HEAD_CHARS).collect();
    let tail: String = text.chars().skip(count - DETAIL_TAIL_CHARS).collect();
    let hidden = count - DETAIL_HEAD_CHARS - DETAIL_TAIL_CHARS;
    format!("{head} [… {hidden} more characters not shown …] {tail}")
}

/// What importing a pack would install. Text taken from the pack is
/// escaped like [`Reach`] fields, so it is safe to print.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PackReview {
    pub active_profile: String,
    /// Profiles as they will be after the import (older packs upgraded).
    pub profiles: Vec<PackProfileSummary>,
    pub page_count: usize,
    pub embedded_images: usize,
    /// Schema the pack's config is upgraded from, if it is older.
    pub migrated_from: Option<u32>,
    /// What the upgrade could not carry over.
    pub notes: Vec<String>,
    pub reach: Vec<Reach>,
}

/// Reads and checks a pack like [`crate::import_pack`] does and describes
/// what it would install, without writing anything.
pub fn review_pack(path: &Path) -> Result<PackReview> {
    let (_, config, migration) = load_checked(path)?;
    let profiles = config
        .profiles
        .iter()
        .map(|profile| {
            let summary = profile_summary(profile);
            PackProfileSummary {
                id: visible_text(&summary.id),
                name: visible_text(&summary.name),
                ..summary
            }
        })
        .collect();
    let notes = migration.as_ref().map_or_else(Vec::new, |m| {
        m.notes.iter().map(|note| visible_text(note)).collect()
    });
    Ok(PackReview {
        active_profile: visible_text(&config.active_profile),
        profiles,
        page_count: config.profiles.iter().map(|p| p.pages.len()).sum(),
        embedded_images: crate::count_embedded_images(&config),
        migrated_from: migration.as_ref().map(|m| m.from_version),
        notes,
        reach: outside_reach(&config),
    })
}
