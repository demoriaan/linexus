//! Upgrades older config files to the current schema.
//!
//! Schema 1 described pages by a fixed `kind` (status, performance, sensors,
//! media, button grid) plus tappable `elements`. Schema 2 composes every page
//! from widgets. Fixed kinds become their widget templates, elements become
//! buttons, and pure page-navigation buttons are dropped because a swipe now
//! changes pages everywhere. A button that has no room on its page is
//! dropped with a note in the report.

use crate::defaults::{template_page, PageTemplate};
use crate::model::*;
use crate::validate::{MAX_SEQUENCE_DELAY_MS, MAX_SEQUENCE_STEPS};
use serde_json::Value;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    pub from_version: u32,
    pub pages: usize,
    pub buttons: usize,
    /// Human-readable notes about things that could not be carried over.
    pub notes: Vec<String>,
}

/// Returns the upgraded config, or `None` when `raw` already has the current
/// schema version.
pub fn migrate(raw: &Value) -> Result<Option<(NexusConfig, MigrationReport)>, String> {
    let version = raw
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or("config has no schema_version")?;
    match u32::try_from(version) {
        Ok(v) if v == crate::SCHEMA_VERSION => Ok(None),
        Ok(1) => migrate_v1(raw).map(Some),
        _ => Err(format!("unsupported config schema version {version}")),
    }
}

/// Clamps values that older versions accepted and only limited when using
/// them, so such configs keep loading now that validation rejects them.
pub(crate) fn clamp_legacy_values(config: &mut NexusConfig) {
    let idle = &mut config.settings.idle;
    idle.dim_percent = idle.dim_percent.min(100);
    let profiles = fill_empty_ids(config.profiles.iter_mut().map(|p| &mut p.id), "profile");
    if let Some(new) = profiles.get(&config.active_profile) {
        config.active_profile = new.clone();
    }
    for profile in &mut config.profiles {
        let pages = fill_empty_ids(profile.pages.iter_mut().map(|p| &mut p.id), "page");
        for page in &mut profile.pages {
            if page.auto_return_seconds == Some(0) {
                page.auto_return_seconds = Some(1);
            }
            fill_empty_ids(page.widgets.iter_mut().map(|w| &mut w.id), "widget");
            for widget in &mut page.widgets {
                for action in [&mut widget.tap, &mut widget.double_tap, &mut widget.hold]
                    .into_iter()
                    .flatten()
                {
                    clamp_action(action);
                    retarget(action, &profiles, &pages);
                }
            }
        }
    }
}

fn clamp_action(action: &mut ActionSpec) {
    match action {
        ActionSpec::Volume {
            percent: Some(percent),
            ..
        }
        | ActionSpec::Brightness {
            percent: Some(percent),
            ..
        } => *percent = (*percent).min(100),
        ActionSpec::Sequence { steps, delay_ms } => {
            *delay_ms = (*delay_ms).min(MAX_SEQUENCE_DELAY_MS);
            steps.truncate(MAX_SEQUENCE_STEPS);
            steps.iter_mut().for_each(clamp_action);
        }
        ActionSpec::HttpRequest {
            content_type: Some(value),
            ..
        } => {
            if let Some(end) = value.find(['\r', '\n']) {
                value.truncate(end);
            }
        }
        _ => {}
    }
}

/// Gives blank ids (which older versions accepted) a unique `<kind>-<n>`
/// and returns which blank id became which, so references can follow.
fn fill_empty_ids<'a>(
    ids: impl Iterator<Item = &'a mut String>,
    kind: &str,
) -> std::collections::HashMap<String, String> {
    let ids: Vec<&mut String> = ids.collect();
    let mut taken: std::collections::HashSet<String> = ids
        .iter()
        .filter(|id| !id.trim().is_empty())
        .map(|id| id.to_string())
        .collect();
    let mut renamed = std::collections::HashMap::new();
    let mut n = 1;
    for id in ids {
        if id.trim().is_empty() {
            while taken.contains(&format!("{kind}-{n}")) {
                n += 1;
            }
            let new = format!("{kind}-{n}");
            renamed.entry(id.clone()).or_insert_with(|| new.clone());
            *id = new.clone();
            taken.insert(new);
        }
    }
    renamed
}

/// Points profile and page actions at the ids `fill_empty_ids` gave.
fn retarget(
    action: &mut ActionSpec,
    profiles: &std::collections::HashMap<String, String>,
    pages: &std::collections::HashMap<String, String>,
) {
    match action {
        ActionSpec::SwitchProfile { profile_id } => {
            if let Some(new) = profiles.get(profile_id) {
                *profile_id = new.clone();
            }
        }
        ActionSpec::GoToPage { page_id } => {
            if let Some(new) = pages.get(page_id) {
                *page_id = new.clone();
            }
        }
        ActionSpec::Sequence { steps, .. } => {
            for step in steps {
                retarget(step, profiles, pages);
            }
        }
        _ => {}
    }
}

fn migrate_v1(raw: &Value) -> Result<(NexusConfig, MigrationReport), String> {
    let mut report = MigrationReport {
        from_version: 1,
        ..MigrationReport::default()
    };
    let profiles = raw
        .get("profiles")
        .and_then(Value::as_array)
        .ok_or("config has no profiles")?;
    let mut out = Vec::new();
    for profile in profiles {
        let id = str_field(profile, "id").ok_or("profile without id")?;
        let name = str_field(profile, "name").unwrap_or_else(|| id.clone());
        let brightness = profile
            .get("brightness")
            .and_then(Value::as_u64)
            .unwrap_or(80)
            .min(100) as u8;
        let preset = match str_field(profile, "theme").as_deref() {
            Some("light") => "daylight",
            Some("cyberpunk") => "neon",
            Some("aurora") => "aurora",
            _ => "midnight",
        };
        let mut pages = Vec::new();
        for page in profile
            .get("pages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(page) = migrate_page(page, &mut report) {
                pages.push(page);
            }
        }
        if pages.is_empty() {
            report.notes.push(format!(
                "profile '{id}' had no usable page; added an overview"
            ));
            pages.push(template_page(PageTemplate::Overview, "overview"));
        }
        report.pages += pages.len();
        out.push(Profile {
            id,
            name,
            brightness,
            theme: Theme {
                preset: preset.into(),
                accent: None,
            },
            pages,
            apps: Vec::new(),
        });
    }
    let active_profile = str_field(raw, "active_profile")
        .filter(|id| out.iter().any(|profile| &profile.id == id))
        .or_else(|| out.first().map(|profile| profile.id.clone()))
        .ok_or("config has no profiles")?;
    let mut config = NexusConfig {
        schema_version: crate::SCHEMA_VERSION,
        active_profile,
        settings: Settings::default(),
        profiles: out,
    };
    drop_dangling_targets(&mut config, &mut report);
    Ok((config, report))
}

fn migrate_page(page: &Value, report: &mut MigrationReport) -> Option<Page> {
    let id = str_field(page, "id")?;
    let kind = str_field(page, "kind").unwrap_or_default();
    let template = match kind.as_str() {
        "status" => Some(PageTemplate::Overview),
        "performance" => Some(PageTemplate::Performance),
        "sensors" => Some(PageTemplate::News),
        "media" => Some(PageTemplate::Media),
        _ => None,
    };
    let mut migrated = match template {
        Some(template) => template_page(template, &id),
        None => Page {
            id: id.clone(),
            name: String::new(),
            hidden: false,
            auto_return_seconds: None,
            background: None,
            widgets: Vec::new(),
        },
    };
    // On a template page the buttons only fit into gaps its widgets leave.
    migrate_elements(page, &mut migrated.widgets, &id, report);
    if let Some(name) = str_field(page, "name") {
        migrated.name = name;
    }
    migrated.background = page.get("background").and_then(migrate_background);
    Some(migrated)
}

/// Adds the page's v1 elements as buttons at their old positions, next to
/// the `widgets` already on the page.
pub(crate) fn migrate_elements(
    page: &Value,
    widgets: &mut Vec<Widget>,
    page_id: &str,
    report: &mut MigrationReport,
) {
    let mut elements: Vec<&Value> = page
        .get("elements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    elements.sort_by_key(|element| element.get("x").and_then(Value::as_u64).unwrap_or(0));
    let built_in = widgets.len();
    for element in elements {
        let Some(id) = str_field(element, "id").filter(|id| !id.trim().is_empty()) else {
            report.notes.push(format!(
                "page '{page_id}': a button without an id was dropped"
            ));
            continue;
        };
        let action = element
            .get("action")
            .cloned()
            .and_then(|value| serde_json::from_value::<ActionSpec>(value).ok());
        if matches!(
            action,
            Some(ActionSpec::NextPage) | Some(ActionSpec::PreviousPage)
        ) {
            continue;
        }
        let coordinate = |key| {
            let value = element.get(key).and_then(Value::as_u64).unwrap_or(0);
            u16::try_from(value).unwrap_or(u16::MAX)
        };
        let x = coordinate("x").min(STRIP_WIDTH - MIN_WIDGET_WIDTH);
        let w = coordinate("width").clamp(MIN_WIDGET_WIDTH, STRIP_WIDTH - x);
        let clash = widgets
            .iter()
            .position(|other| other.id == id || (x < other.end() && other.x < x + w));
        if let Some(index) = clash {
            let reason = if widgets[index].id == id {
                "reused the id of another widget"
            } else if index < built_in {
                "had no room beside the page's built-in widgets"
            } else {
                "overlapped its neighbour"
            };
            report.notes.push(format!(
                "page '{page_id}': button '{id}' {reason} and was dropped"
            ));
            continue;
        }
        let label = str_field(element, "label").unwrap_or_default();
        let icon = str_field(element, "icon").map(|icon| map_icon(&icon));
        let mut widget = Widget::new(
            id,
            x,
            w,
            WidgetKind::Button(ButtonProps {
                label,
                icon,
                ..ButtonProps::default()
            }),
        );
        widget.tap = action;
        report.buttons += 1;
        widgets.push(widget);
    }
}

fn migrate_background(value: &Value) -> Option<Background> {
    let image = str_field(value, "image_data_url")?;
    let fit = match str_field(value, "scale").as_deref() {
        Some("fit") => ImageFit::Fit,
        Some("stretch") => ImageFit::Stretch,
        _ => ImageFit::Fill,
    };
    let dim_percent = value
        .get("dim_percent")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(90) as u8;
    Some(Background {
        image: Some(image),
        color: None,
        fit,
        dim_percent,
    })
}

/// Schema 1 icon names were freedesktop-style; map the ones the old
/// defaults used and pass anything else through as a Lucide name.
fn map_icon(icon: &str) -> String {
    let name = match icon {
        "browser" => "globe",
        "terminal" => "square-terminal",
        "folder" => "folder",
        "media-previous" => "skip-back",
        "media-next" => "skip-forward",
        "media-playback-start" => "play",
        "audio-volume-low" => "volume-1",
        "audio-volume-high" => "volume-2",
        "audio-volume-muted" => "volume-x",
        "system" => "monitor",
        other => other,
    };
    format!("lucide:{name}")
}

fn drop_dangling_targets(config: &mut NexusConfig, report: &mut MigrationReport) {
    let profile_ids: Vec<String> = config.profiles.iter().map(|p| p.id.clone()).collect();
    for profile in &mut config.profiles {
        for page in &mut profile.pages {
            for widget in &mut page.widgets {
                if let Some(ActionSpec::SwitchProfile { profile_id }) = &widget.tap {
                    if !profile_ids.contains(profile_id) {
                        report.notes.push(format!(
                            "button '{}' pointed at missing profile '{profile_id}'",
                            widget.id
                        ));
                        widget.tap = None;
                    }
                }
            }
        }
    }
}

fn str_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}
