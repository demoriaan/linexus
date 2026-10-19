use crate::model::*;
use std::collections::HashSet;

/// Upper bound for embedded images (backgrounds, image widgets, icons).
pub const MAX_IMAGE_DATA_URL_BYTES: usize = 3 * 1024 * 1024;
/// Longest pause between two steps of a sequence.
pub const MAX_SEQUENCE_DELAY_MS: u32 = 60_000;
/// Most steps one sequence may run.
pub const MAX_SEQUENCE_STEPS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("unsupported config schema version {actual}; expected {expected}")]
    UnsupportedSchema { expected: u32, actual: u32 },
    #[error("active profile '{0}' does not exist")]
    MissingActiveProfile(String),
    #[error("profile id '{0}' is used twice")]
    DuplicateProfileId(String),
    #[error("a profile has an empty id")]
    EmptyProfileId,
    #[error("a page in profile '{profile}' has an empty id")]
    EmptyPageId { profile: String },
    #[error("a widget on page '{page}' has an empty id")]
    EmptyWidgetId { page: String },
    #[error("profile '{0}' has no visible page")]
    EmptyProfile(String),
    #[error("profile '{profile}' brightness {brightness} is above 100")]
    InvalidBrightness { profile: String, brightness: u8 },
    #[error("page id '{page}' is used twice in profile '{profile}'")]
    DuplicatePageId { profile: String, page: String },
    #[error("widget id '{widget}' is used twice on page '{page}'")]
    DuplicateWidgetId { page: String, widget: String },
    #[error("widget '{widget}' on page '{page}' does not fit the 640 px strip")]
    WidgetOutOfBounds { page: String, widget: String },
    #[error("widget '{widget}' on page '{page}' is narrower than {MIN_WIDGET_WIDTH} px")]
    WidgetTooNarrow { page: String, widget: String },
    #[error("widgets '{left}' and '{right}' overlap on page '{page}'")]
    WidgetsOverlap {
        page: String,
        left: String,
        right: String,
    },
    #[error("widget '{widget}' switches to missing profile '{target}'")]
    MissingProfileTarget { widget: String, target: String },
    #[error("widget '{widget}' opens missing page '{target}'")]
    MissingPageTarget { widget: String, target: String },
    #[error("widget '{widget}' sets the volume without a percentage")]
    VolumeSetMissingPercent { widget: String },
    #[error("widget '{widget}' sets the volume to {percent}%; the most is 100%")]
    VolumeSetTooHigh { widget: String, percent: u8 },
    #[error("widget '{widget}' sets the brightness without a percentage")]
    BrightnessSetMissingPercent { widget: String },
    #[error("widget '{widget}' sets the brightness to {percent}%; the most is 100%")]
    BrightnessSetTooHigh { widget: String, percent: u8 },
    #[error("widget '{widget}' runs a command with an invalid program name")]
    InvalidShellProgram { widget: String },
    #[error("widget '{widget}' has an invalid URL")]
    InvalidUrl { widget: String },
    #[error("widget '{widget}' sends an HTTP content type with a line break")]
    InvalidContentType { widget: String },
    #[error("widget '{widget}' nests sequences too deeply")]
    SequenceTooDeep { widget: String },
    #[error(
        "widget '{widget}' runs a sequence of {steps} steps; the most is {MAX_SEQUENCE_STEPS}"
    )]
    SequenceTooLong { widget: String, steps: usize },
    #[error("widget '{widget}' waits {delay_ms} ms between sequence steps; the most is {MAX_SEQUENCE_DELAY_MS} ms")]
    SequenceDelayTooLong { widget: String, delay_ms: u32 },
    #[error("page '{page}' goes back after 0 seconds; it needs at least 1 second")]
    InvalidAutoReturn { page: String },
    #[error("idle dimming to {dim_percent}% is above 100%")]
    InvalidIdleDim { dim_percent: u8 },
    #[error("page '{page}' background dim {dim_percent}% is above 90%")]
    InvalidBackgroundDim { page: String, dim_percent: u8 },
    #[error("image on page '{page}' is not a base64 data URL below 3 MiB")]
    InvalidImage { page: String },
}

pub type Result<T> = std::result::Result<T, ConfigError>;

pub fn validate(config: &NexusConfig) -> Result<()> {
    if config.schema_version != crate::SCHEMA_VERSION {
        return Err(ConfigError::UnsupportedSchema {
            expected: crate::SCHEMA_VERSION,
            actual: config.schema_version,
        });
    }
    if config.settings.idle.dim_percent > 100 {
        return Err(ConfigError::InvalidIdleDim {
            dim_percent: config.settings.idle.dim_percent,
        });
    }
    let mut profile_ids = HashSet::new();
    for profile in &config.profiles {
        if profile.id.trim().is_empty() {
            return Err(ConfigError::EmptyProfileId);
        }
        if !profile_ids.insert(profile.id.as_str()) {
            return Err(ConfigError::DuplicateProfileId(profile.id.clone()));
        }
    }
    if !profile_ids.contains(config.active_profile.as_str()) {
        return Err(ConfigError::MissingActiveProfile(
            config.active_profile.clone(),
        ));
    }
    for profile in &config.profiles {
        validate_profile(profile, &profile_ids)?;
    }
    Ok(())
}

fn validate_profile(profile: &Profile, profile_ids: &HashSet<&str>) -> Result<()> {
    if profile.brightness > 100 {
        return Err(ConfigError::InvalidBrightness {
            profile: profile.id.clone(),
            brightness: profile.brightness,
        });
    }
    if !profile.pages.iter().any(|page| !page.hidden) {
        return Err(ConfigError::EmptyProfile(profile.id.clone()));
    }
    let mut page_ids = HashSet::new();
    for page in &profile.pages {
        if page.id.trim().is_empty() {
            return Err(ConfigError::EmptyPageId {
                profile: profile.id.clone(),
            });
        }
        if !page_ids.insert(page.id.as_str()) {
            return Err(ConfigError::DuplicatePageId {
                profile: profile.id.clone(),
                page: page.id.clone(),
            });
        }
    }
    for page in &profile.pages {
        validate_page(page, profile_ids, &page_ids)?;
    }
    Ok(())
}

fn validate_page(page: &Page, profile_ids: &HashSet<&str>, page_ids: &HashSet<&str>) -> Result<()> {
    if page.auto_return_seconds == Some(0) {
        return Err(ConfigError::InvalidAutoReturn {
            page: page.id.clone(),
        });
    }
    if let Some(background) = &page.background {
        if background.dim_percent > 90 {
            return Err(ConfigError::InvalidBackgroundDim {
                page: page.id.clone(),
                dim_percent: background.dim_percent,
            });
        }
        if let Some(image) = &background.image {
            check_image(page, image)?;
        }
    }
    let mut widget_ids = HashSet::new();
    for widget in &page.widgets {
        if widget.id.trim().is_empty() {
            return Err(ConfigError::EmptyWidgetId {
                page: page.id.clone(),
            });
        }
        if !widget_ids.insert(widget.id.as_str()) {
            return Err(ConfigError::DuplicateWidgetId {
                page: page.id.clone(),
                widget: widget.id.clone(),
            });
        }
        if widget.end() > STRIP_WIDTH {
            return Err(ConfigError::WidgetOutOfBounds {
                page: page.id.clone(),
                widget: widget.id.clone(),
            });
        }
        if widget.w < MIN_WIDGET_WIDTH {
            return Err(ConfigError::WidgetTooNarrow {
                page: page.id.clone(),
                widget: widget.id.clone(),
            });
        }
        match &widget.kind {
            WidgetKind::Image(props) => check_image(page, &props.src)?,
            WidgetKind::Button(ButtonProps {
                icon: Some(icon), ..
            }) if icon.starts_with("data:") => check_image(page, icon)?,
            WidgetKind::Command(props) => check_program(&widget.id, &props.program)?,
            _ => {}
        }
        for action in [&widget.tap, &widget.double_tap, &widget.hold]
            .into_iter()
            .flatten()
        {
            validate_action(&widget.id, action, profile_ids, page_ids, 0)?;
        }
    }
    let mut sorted: Vec<&Widget> = page.widgets.iter().collect();
    sorted.sort_by_key(|widget| widget.x);
    for pair in sorted.windows(2) {
        if pair[1].x < pair[0].end() {
            return Err(ConfigError::WidgetsOverlap {
                page: page.id.clone(),
                left: pair[0].id.clone(),
                right: pair[1].id.clone(),
            });
        }
    }
    Ok(())
}

fn check_image(page: &Page, data_url: &str) -> Result<()> {
    if !data_url.starts_with("data:image/")
        || !data_url.contains(";base64,")
        || data_url.len() > MAX_IMAGE_DATA_URL_BYTES
    {
        return Err(ConfigError::InvalidImage {
            page: page.id.clone(),
        });
    }
    Ok(())
}

fn check_program(widget: &str, program: &str) -> Result<()> {
    if program.trim().is_empty() || program.contains('/') {
        return Err(ConfigError::InvalidShellProgram {
            widget: widget.to_string(),
        });
    }
    Ok(())
}

fn validate_action(
    widget: &str,
    action: &ActionSpec,
    profile_ids: &HashSet<&str>,
    page_ids: &HashSet<&str>,
    depth: usize,
) -> Result<()> {
    match action {
        ActionSpec::SwitchProfile { profile_id } if !profile_ids.contains(profile_id.as_str()) => {
            Err(ConfigError::MissingProfileTarget {
                widget: widget.to_string(),
                target: profile_id.clone(),
            })
        }
        ActionSpec::GoToPage { page_id } if !page_ids.contains(page_id.as_str()) => {
            Err(ConfigError::MissingPageTarget {
                widget: widget.to_string(),
                target: page_id.clone(),
            })
        }
        ActionSpec::Volume {
            command: VolumeAction::Set,
            percent: None,
            ..
        } => Err(ConfigError::VolumeSetMissingPercent {
            widget: widget.to_string(),
        }),
        ActionSpec::Volume {
            command: VolumeAction::Set,
            percent: Some(percent),
            ..
        } if *percent > 100 => Err(ConfigError::VolumeSetTooHigh {
            widget: widget.to_string(),
            percent: *percent,
        }),
        ActionSpec::Brightness {
            command: BrightnessAction::Set,
            percent: None,
        } => Err(ConfigError::BrightnessSetMissingPercent {
            widget: widget.to_string(),
        }),
        ActionSpec::Brightness {
            command: BrightnessAction::Set,
            percent: Some(percent),
        } if *percent > 100 => Err(ConfigError::BrightnessSetTooHigh {
            widget: widget.to_string(),
            percent: *percent,
        }),
        ActionSpec::ShellCommand { program, .. } => check_program(widget, program),
        ActionSpec::HttpRequest { url, .. }
            if !(url.starts_with("http://") || url.starts_with("https://")) =>
        {
            Err(ConfigError::InvalidUrl {
                widget: widget.to_string(),
            })
        }
        // The content type becomes a header line; a line break would add more.
        ActionSpec::HttpRequest {
            content_type: Some(content_type),
            ..
        } if content_type.contains(['\r', '\n']) => Err(ConfigError::InvalidContentType {
            widget: widget.to_string(),
        }),
        ActionSpec::Sequence { steps, delay_ms } => {
            if depth >= 2 {
                return Err(ConfigError::SequenceTooDeep {
                    widget: widget.to_string(),
                });
            }
            if steps.len() > MAX_SEQUENCE_STEPS {
                return Err(ConfigError::SequenceTooLong {
                    widget: widget.to_string(),
                    steps: steps.len(),
                });
            }
            if *delay_ms > MAX_SEQUENCE_DELAY_MS {
                return Err(ConfigError::SequenceDelayTooLong {
                    widget: widget.to_string(),
                    delay_ms: *delay_ms,
                });
            }
            for step in steps {
                validate_action(widget, step, profile_ids, page_ids, depth + 1)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
