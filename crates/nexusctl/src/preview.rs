//! `render-preview`: draws a page or a test pattern into a PNG with sample
//! data, without the daemon or the device.

use anyhow::{bail, Context, Result};
use clap::builder::PossibleValuesParser;
use nexus_config::{NexusConfig, Page, PageTemplate, TemperatureUnit, Theme, WidgetKind};
use nexus_core::{HEIGHT, WIDTH};
use nexus_render::{preview, Locale, UiState};
use std::path::{Path, PathBuf};

#[derive(Debug, clap::Args)]
#[command(group(clap::ArgGroup::new("source").args(["template", "config", "pattern"])))]
pub(crate) struct RenderPreviewArgs {
    /// Built-in page template (the default is `overview`).
    #[arg(long, value_enum)]
    template: Option<TemplateArg>,
    /// Render a page from this config file instead of a template.
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,
    /// Profile in --config; the active profile when omitted.
    #[arg(long, requires = "config")]
    profile: Option<String>,
    /// Page in --config; the profile's first visible page when omitted.
    #[arg(long, requires = "config")]
    page: Option<String>,
    /// Hardware test pattern instead of a page.
    #[arg(long, value_enum)]
    pattern: Option<Pattern>,
    /// Colour theme; the profile's theme (or midnight) when omitted.
    #[arg(long, value_parser = PossibleValuesParser::new(nexus_render::PRESETS), conflicts_with = "pattern")]
    theme: Option<String>,
    /// Language of dates and labels; the config's locale (or en) when omitted.
    #[arg(long, value_enum, conflicts_with = "pattern")]
    locale: Option<LocaleArg>,
    /// Enlarge by this integer factor (nearest neighbour), e.g. 2 for docs.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=16))]
    scale: u32,
    /// Output PNG file.
    #[arg(long, value_name = "FILE")]
    out: PathBuf,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum TemplateArg {
    Overview,
    Performance,
    Media,
    Launcher,
    News,
    Focus,
    Home,
    Blank,
}

impl From<TemplateArg> for PageTemplate {
    fn from(arg: TemplateArg) -> Self {
        match arg {
            TemplateArg::Overview => Self::Overview,
            TemplateArg::Performance => Self::Performance,
            TemplateArg::Media => Self::Media,
            TemplateArg::Launcher => Self::Launcher,
            TemplateArg::News => Self::News,
            TemplateArg::Focus => Self::Focus,
            TemplateArg::Home => Self::Home,
            TemplateArg::Blank => Self::Blank,
        }
    }
}

/// Test patterns for checking colour order and geometry on the panel.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum Pattern {
    /// Red, green and blue thirds.
    RgbBars,
    /// 1 px border with a 100 x 12 px grid.
    BorderGrid,
}

impl Pattern {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::RgbBars => "rgb-bars",
            Self::BorderGrid => "border-grid",
        }
    }

    pub(crate) fn frame(self) -> Vec<u8> {
        match self {
            Self::RgbBars => nexus_render::rgb_bars_frame(),
            Self::BorderGrid => nexus_render::border_grid_frame(),
        }
    }
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub(crate) enum LocaleArg {
    En,
    De,
}

impl From<LocaleArg> for Locale {
    fn from(arg: LocaleArg) -> Self {
        match arg {
            LocaleArg::En => Locale::En,
            LocaleArg::De => Locale::De,
        }
    }
}

pub(crate) fn run(args: RenderPreviewArgs) -> Result<()> {
    let (rgba, what) = if let Some(pattern) = args.pattern {
        (pattern.frame(), format!("pattern {}", pattern.name()))
    } else if let Some(path) = &args.config {
        render_from_config(path, &args)?
    } else {
        let template = PageTemplate::from(args.template.unwrap_or(TemplateArg::Overview));
        let page = nexus_config::template_page(template, "preview");
        let theme = theme_with_preset(Theme::default(), args.theme.as_deref());
        let locale = args.locale.map_or(Locale::En, Locale::from);
        let live = preview::sample_live(locale);
        let canvas = nexus_render::render_page(&page, &theme, &live, &UiState::default());
        (canvas.into_rgba(), format!("template {}", template.name()))
    };
    save_png(&args.out, &rgba, args.scale)?;
    println!(
        "Preview written: {} ({what}, {}x{})",
        args.out.display(),
        WIDTH as u32 * args.scale,
        HEIGHT as u32 * args.scale
    );
    Ok(())
}

fn render_from_config(path: &Path, args: &RenderPreviewArgs) -> Result<(Vec<u8>, String)> {
    let (config, migration) = nexus_config::load_with_report(path)
        .with_context(|| format!("failed to load config {}", path.display()))?;
    if let Some(report) = migration {
        eprintln!(
            "note: {} uses config schema {}; rendering the upgraded version (run `linexusctl config upgrade` to rewrite the file)",
            path.display(),
            report.from_version
        );
    }
    let (rgba, profile_id, page_id) = render_config_page(
        &config,
        args.profile.as_deref(),
        args.page.as_deref(),
        args.theme.as_deref(),
        args.locale.map(Locale::from),
    )?;
    Ok((rgba, format!("profile '{profile_id}', page '{page_id}'")))
}

/// Renders one page of `config` with sample readings. Returns the frame and
/// the ids of the profile and page that were drawn.
pub(crate) fn render_config_page(
    config: &NexusConfig,
    profile_id: Option<&str>,
    page_id: Option<&str>,
    theme: Option<&str>,
    locale: Option<Locale>,
) -> Result<(Vec<u8>, String, String)> {
    let profile_id = profile_id.unwrap_or(&config.active_profile);
    let Some(profile) = config.profiles.iter().find(|p| p.id == profile_id) else {
        bail!(
            "profile '{profile_id}' not found; available: {}",
            join_ids(config.profiles.iter().map(|p| p.id.as_str()))
        );
    };
    let visible: Vec<&Page> = profile.pages.iter().filter(|p| !p.hidden).collect();
    let page = match page_id {
        Some(id) => profile.pages.iter().find(|p| p.id == id).with_context(|| {
            format!(
                "page '{id}' not found in profile '{}'; available: {}",
                profile.id,
                join_ids(profile.pages.iter().map(|p| p.id.as_str()))
            )
        })?,
        None => visible
            .first()
            .copied()
            .with_context(|| format!("profile '{}' has no visible page", profile.id))?,
    };

    let theme = theme_with_preset(profile.theme.clone(), theme);
    let locale = locale.unwrap_or_else(|| Locale::resolve(&config.settings.locale));
    let mut live = preview::sample_live(locale);
    live.hour12 = config.settings.hour12;
    live.fahrenheit = config.settings.temperature_unit == TemperatureUnit::Fahrenheit;
    fill_sample_for_page(&mut live, page);
    let ui = UiState {
        page_index: visible.iter().position(|p| p.id == page.id).unwrap_or(0),
        page_count: visible.len(),
        ..UiState::default()
    };
    let canvas = nexus_render::render_page(page, &theme, &live, &ui);
    Ok((canvas.into_rgba(), profile.id.clone(), page.id.clone()))
}

/// The sample data is keyed for the built-in templates; give widgets of a
/// user page (own timer ids, weather places, Home Assistant entities)
/// matching sample readings too, so the preview shows content. Command
/// widgets keep their "no output yet" look: nothing runs for a preview.
fn fill_sample_for_page(live: &mut nexus_render::LiveData, page: &Page) {
    let timer = live.timers.values().next().cloned();
    let weather = live.weather.get("").cloned();
    let entity = live.entities.values().next().cloned();
    for widget in &page.widgets {
        match &widget.kind {
            WidgetKind::Timer(_) => {
                if let Some(timer) = &timer {
                    live.timers
                        .entry(widget.id.clone())
                        .or_insert_with(|| timer.clone());
                }
            }
            WidgetKind::Weather(props) => {
                if let Some(weather) = &weather {
                    let key = nexus_render::weather_key(props);
                    let mut sample = weather.clone();
                    if let Some(location) = &props.location {
                        sample.location = location.name.clone();
                    }
                    live.weather.entry(key).or_insert(sample);
                }
            }
            WidgetKind::Entity(props) => {
                if let Some(entity) = &entity {
                    live.entities
                        .entry(props.entity_id.clone())
                        .or_insert_with(|| nexus_render::EntityInfo {
                            name: props
                                .label
                                .clone()
                                .unwrap_or_else(|| props.entity_id.clone()),
                            ..entity.clone()
                        });
                }
            }
            _ => {}
        }
    }
}

fn theme_with_preset(theme: Theme, preset: Option<&str>) -> Theme {
    match preset {
        // An explicit preset shows that palette as designed, so the
        // profile's accent override is dropped.
        Some(preset) => Theme {
            preset: preset.to_string(),
            accent: None,
        },
        None => theme,
    }
}

fn join_ids<'a>(ids: impl Iterator<Item = &'a str>) -> String {
    ids.collect::<Vec<_>>().join(", ")
}

/// Writes an RGBA strip frame as PNG, enlarged by `scale`.
pub(crate) fn save_png(path: &Path, rgba: &[u8], scale: u32) -> Result<()> {
    let image = image::RgbaImage::from_raw(WIDTH as u32, HEIGHT as u32, rgba.to_vec())
        .context("frame does not have 640x48 RGBA pixels")?;
    save_scaled(path, image, scale)
}

pub(crate) fn save_scaled(path: &Path, image: image::RgbaImage, scale: u32) -> Result<()> {
    let image = if scale > 1 {
        image::imageops::resize(
            &image,
            image.width() * scale,
            image.height() * scale,
            image::imageops::FilterType::Nearest,
        )
    } else {
        image
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }
    image
        .save_with_format(path, image::ImageFormat::Png)
        .with_context(|| format!("failed to write PNG {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum;

    #[test]
    fn every_template_renders_a_full_frame() {
        let live = preview::sample_live(Locale::De);
        for arg in TemplateArg::value_variants() {
            let page = nexus_config::template_page(PageTemplate::from(*arg), "p");
            let canvas =
                nexus_render::render_page(&page, &Theme::default(), &live, &UiState::default());
            assert_eq!(canvas.rgba().len(), nexus_core::FRAME_BYTES, "{arg:?}");
        }
    }

    #[test]
    fn config_page_defaults_to_active_profile_and_first_visible_page() {
        let config = nexus_config::default_config();
        let (rgba, profile, page) = render_config_page(&config, None, None, None, None).unwrap();
        assert_eq!(rgba.len(), nexus_core::FRAME_BYTES);
        assert_eq!(profile, nexus_config::DEFAULT_PROFILE_ID);
        assert_eq!(page, "overview");

        let (_, _, page) = render_config_page(
            &config,
            None,
            Some("media"),
            Some("ember"),
            Some(Locale::En),
        )
        .unwrap();
        assert_eq!(page, "media");
    }

    #[test]
    fn unknown_page_lists_the_available_ones() {
        let config = nexus_config::default_config();
        let err = render_config_page(&config, None, Some("nope"), None, None).unwrap_err();
        assert!(err.to_string().contains("overview, media"), "{err}");
    }

    #[test]
    fn user_widgets_get_sample_readings() {
        let mut page = nexus_config::template_page(PageTemplate::Focus, "focus");
        page.widgets[0].id = "pomodoro".into();
        let mut live = preview::sample_live(Locale::En);
        fill_sample_for_page(&mut live, &page);
        assert!(live.timers.contains_key("pomodoro"));
    }

    #[test]
    fn scaled_png_has_scaled_size() {
        let dir = std::env::temp_dir().join(format!("linexusctl-preview-{}", std::process::id()));
        let out = dir.join("bars.png");
        save_png(&out, &Pattern::RgbBars.frame(), 3).unwrap();
        let img = image::open(&out).unwrap();
        assert_eq!((img.width(), img.height()), (1920, 144));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
