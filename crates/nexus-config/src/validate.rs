use crate::audio::*;
use crate::model::*;
use std::collections::HashSet;

/// Upper bound for embedded images (backgrounds, image widgets, icons).
pub const MAX_IMAGE_DATA_URL_BYTES: usize = 3 * 1024 * 1024;
/// Longest pause between two steps of a sequence.
pub const MAX_SEQUENCE_DELAY_MS: u32 = 60_000;
/// Most steps one sequence may run.
pub const MAX_SEQUENCE_STEPS: usize = 16;
/// PipeWire node names run longer than the names people give devices.
const MAX_NODE_NAME_CHARS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    #[error("touch correction {left}/{right} px is out of range (at most {max} px either way)", max = MAX_TOUCH_CORRECTION)]
    InvalidTouchCorrection { left: i16, right: i16 },
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
    #[error("widget '{widget}' controls OBS without naming the scene or audio source")]
    ObsTargetMissing { widget: String },
    #[error("widget '{widget}' joins a Discord voice channel without naming a valid channel")]
    DiscordChannelMissing { widget: String },
    #[error("widget '{widget}' names an invalid Discord server id")]
    InvalidDiscordGuild { widget: String },
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
    #[error(
        "breaking-news banners stay {duration_ms} ms; allowed are {} to {} ms",
        ALERT_DURATION_MS.start(),
        ALERT_DURATION_MS.end()
    )]
    InvalidAlertDuration { duration_ms: u32 },
    #[error("page '{page}' background dim {dim_percent}% is above 90%")]
    InvalidBackgroundDim { page: String, dim_percent: u8 },
    #[error("image on page '{page}' is not a base64 data URL below 3 MiB")]
    InvalidImage { page: String },
    #[error("settings.ping.host is not a host name or IP address")]
    InvalidDefaultPingHost,
    #[error("ping interval {interval_ms} ms is out of range ({MIN_PING_INTERVAL_MS} to {MAX_PING_INTERVAL_MS} ms)")]
    InvalidPingInterval { interval_ms: u32 },
    #[error("widget '{widget}' pings something that is not a host name or IP address")]
    InvalidPingHost { widget: String },
    #[error("widget '{widget}' turns amber at {warn_ms} ms and red at {critical_ms} ms; both must be 1 to {MAX_PING_THRESHOLD_MS} ms, amber below red")]
    InvalidPingThresholds {
        widget: String,
        warn_ms: u32,
        critical_ms: u32,
    },
    #[error("audio device '{node}' is listed twice, has an empty or overlong name, or the list is longer than {MAX_AUDIO_DEVICES}")]
    InvalidAudioDevice { node: String },
    #[error("widget '{widget}' names an empty or overlong sound device or app, or more than {MAX_AUDIO_LIST}")]
    InvalidAudioList { widget: String },
}

pub type Result<T> = std::result::Result<T, ConfigError>;

pub fn validate(config: &NexusConfig) -> Result<()> {
    if config.schema_version != crate::SCHEMA_VERSION {
        return Err(ConfigError::UnsupportedSchema {
            expected: crate::SCHEMA_VERSION,
            actual: config.schema_version,
        });
    }
    let touch = config.settings.touch;
    let limit = MAX_TOUCH_CORRECTION.unsigned_abs();
    if touch.left.unsigned_abs() > limit || touch.right.unsigned_abs() > limit {
        return Err(ConfigError::InvalidTouchCorrection {
            left: touch.left,
            right: touch.right,
        });
    }
    let ping = &config.settings.ping;
    if !is_valid_host(&ping.host) {
        return Err(ConfigError::InvalidDefaultPingHost);
    }
    if !(MIN_PING_INTERVAL_MS..=MAX_PING_INTERVAL_MS).contains(&ping.interval_ms) {
        return Err(ConfigError::InvalidPingInterval {
            interval_ms: ping.interval_ms,
        });
    }
    if config.settings.idle.dim_percent > 100 {
        return Err(ConfigError::InvalidIdleDim {
            dim_percent: config.settings.idle.dim_percent,
        });
    }
    let duration_ms = config.settings.overlays.alert_duration_ms;
    if !ALERT_DURATION_MS.contains(&duration_ms) {
        return Err(ConfigError::InvalidAlertDuration { duration_ms });
    }
    validate_audio(&config.settings.audio)?;
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
            WidgetKind::Ping(props) => check_ping(&widget.id, props)?,
            WidgetKind::Mixer(props) => check_audio_list(&widget.id, &props.pinned)?,
            WidgetKind::DiscordVoice(props)
                if props
                    .guild_id
                    .as_deref()
                    .is_some_and(|id| !is_snowflake(id)) =>
            {
                return Err(ConfigError::InvalidDiscordGuild {
                    widget: widget.id.clone(),
                })
            }
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

fn check_ping(widget: &str, props: &PingProps) -> Result<()> {
    if props
        .host
        .as_deref()
        .is_some_and(|host| !is_valid_host(host))
    {
        return Err(ConfigError::InvalidPingHost {
            widget: widget.to_string(),
        });
    }
    let threshold = 1..=MAX_PING_THRESHOLD_MS;
    if !threshold.contains(&props.warn_ms)
        || !threshold.contains(&props.critical_ms)
        || props.warn_ms >= props.critical_ms
    {
        return Err(ConfigError::InvalidPingThresholds {
            widget: widget.to_string(),
            warn_ms: props.warn_ms,
            critical_ms: props.critical_ms,
        });
    }
    Ok(())
}

/// An IP address to ping or a host name: dot-separated labels of letters,
/// digits and inner hyphens, at most 253 characters, optionally with a
/// final dot. A name ending in a number is refused, since resolvers would
/// read `1.2.3` as an address. Addresses that a probe cannot reach are
/// refused: unspecified, multicast, broadcast, and IPv6 link-local ones,
/// which would need an interface. IPv4-mapped IPv6 addresses count as the
/// IPv4 address they carry.
pub fn is_valid_host(host: &str) -> bool {
    if let Ok(addr) = host.parse::<std::net::IpAddr>() {
        let addr = addr.to_canonical();
        let link_local = matches!(addr, std::net::IpAddr::V6(v6) if v6.is_unicast_link_local());
        return !addr.is_unspecified()
            && !addr.is_multicast()
            && !link_local
            && addr != std::net::IpAddr::from(std::net::Ipv4Addr::BROADCAST);
    }
    let name = host.strip_suffix('.').unwrap_or(host);
    if name.is_empty() || host.len() > 253 {
        return false;
    }
    let label_ok = |label: &str| {
        (1..=63).contains(&label.len())
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    name.split('.').all(label_ok)
        && !name
            .rsplit('.')
            .next()
            .is_some_and(|last| last.bytes().all(|b| b.is_ascii_digit()))
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
        ActionSpec::AudioOutput { devices } => check_audio_list(widget, devices),
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
        ActionSpec::Obs { command, target }
            if command.needs_target() && target.as_deref().is_none_or(|t| t.trim().is_empty()) =>
        {
            Err(ConfigError::ObsTargetMissing {
                widget: widget.to_string(),
            })
        }
        ActionSpec::Discord {
            command, channel, ..
        } if command.needs_channel() && !channel.as_deref().is_some_and(is_snowflake) => {
            Err(ConfigError::DiscordChannelMissing {
                widget: widget.to_string(),
            })
        }
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

fn validate_audio(audio: &AudioSettings) -> Result<()> {
    let mut nodes = HashSet::new();
    for device in &audio.devices {
        let bad_name = device
            .name
            .as_deref()
            .is_some_and(|name| name.trim().is_empty() || too_long(name, MAX_AUDIO_NAME_CHARS));
        if device.node.trim().is_empty()
            || too_long(&device.node, MAX_NODE_NAME_CHARS)
            || bad_name
            || !nodes.insert(device.node.as_str())
            || nodes.len() > MAX_AUDIO_DEVICES
        {
            return Err(ConfigError::InvalidAudioDevice {
                node: device.node.clone(),
            });
        }
    }
    Ok(())
}

/// Device or app names of a widget: a few, none empty.
fn check_audio_list(widget: &str, names: &[String]) -> Result<()> {
    if names.len() > MAX_AUDIO_LIST
        || names
            .iter()
            .any(|name| name.trim().is_empty() || too_long(name, MAX_NODE_NAME_CHARS))
    {
        return Err(ConfigError::InvalidAudioList {
            widget: widget.to_string(),
        });
    }
    Ok(())
}

fn too_long(name: &str, chars: usize) -> bool {
    name.chars().count() > chars
}

/// A Discord id: a number of up to 20 digits.
fn is_snowflake(id: &str) -> bool {
    !id.is_empty() && id.len() <= 20 && id.bytes().all(|b| b.is_ascii_digit())
}
