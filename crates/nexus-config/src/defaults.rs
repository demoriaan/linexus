//! Built-in profile and page templates.

use crate::model::*;

pub const DEFAULT_PROFILE_ID: &str = "default";

/// Launch targets that the action planner resolves to the desktop's default
/// application at run time, so the built-in launcher works on any desktop.
pub const DEFAULT_BROWSER: &str = "default:browser";
pub const DEFAULT_FILES: &str = "default:files";
pub const DEFAULT_TERMINAL: &str = "default:terminal";
pub const DEFAULT_MAIL: &str = "default:mail";

pub fn default_config() -> NexusConfig {
    NexusConfig {
        schema_version: crate::SCHEMA_VERSION,
        active_profile: DEFAULT_PROFILE_ID.into(),
        settings: Settings::default(),
        profiles: vec![Profile {
            id: DEFAULT_PROFILE_ID.into(),
            name: "Default".into(),
            brightness: 80,
            theme: Theme::default(),
            pages: vec![
                template_page(PageTemplate::Overview, "overview"),
                template_page(PageTemplate::Media, "media"),
                template_page(PageTemplate::Launcher, "launcher"),
                template_page(PageTemplate::News, "news"),
                template_page(PageTemplate::Focus, "focus"),
            ],
            apps: Vec::new(),
        }],
    }
}

/// Starting points offered by Studio's "add page" gallery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageTemplate {
    Overview,
    Performance,
    Media,
    Launcher,
    News,
    Focus,
    Home,
    Streaming,
    Blank,
}

impl PageTemplate {
    pub const ALL: [PageTemplate; 9] = [
        Self::Overview,
        Self::Performance,
        Self::Media,
        Self::Launcher,
        Self::News,
        Self::Focus,
        Self::Home,
        Self::Streaming,
        Self::Blank,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Performance => "Performance",
            Self::Media => "Media",
            Self::Launcher => "Launcher",
            Self::News => "News",
            Self::Focus => "Focus",
            Self::Home => "Home",
            Self::Streaming => "Streaming",
            Self::Blank => "Blank",
        }
    }
}

pub fn template_page(template: PageTemplate, id: &str) -> Page {
    let widgets = match template {
        PageTemplate::Overview => vec![
            Widget::new("clock", 0, 104, WidgetKind::Clock(ClockProps::default())),
            metric(
                "cpu",
                104,
                134,
                "cpu.usage",
                Some("cpu.temp"),
                MetricStyle::Sparkline,
            ),
            metric(
                "gpu",
                238,
                134,
                "gpu.usage",
                Some("gpu.temp"),
                MetricStyle::Sparkline,
            ),
            metric(
                "ram",
                372,
                134,
                "ram.used",
                Some("ram.usage"),
                MetricStyle::Bar,
            ),
            Widget::new(
                "net",
                506,
                134,
                WidgetKind::Network(NetworkProps::default()),
            ),
        ],
        PageTemplate::Performance => vec![
            metric(
                "cpu",
                0,
                128,
                "cpu.usage",
                Some("cpu.temp"),
                MetricStyle::Ring,
            ),
            metric(
                "gpu",
                128,
                128,
                "gpu.usage",
                Some("gpu.temp"),
                MetricStyle::Ring,
            ),
            metric(
                "vram",
                256,
                128,
                "gpu.vram",
                Some("gpu.mem_temp"),
                MetricStyle::Ring,
            ),
            metric(
                "ram",
                384,
                128,
                "ram.usage",
                Some("ram.used"),
                MetricStyle::Ring,
            ),
            metric(
                "disk",
                512,
                128,
                "disk.usage",
                Some("disk.read"),
                MetricStyle::Ring,
            ),
        ],
        PageTemplate::Media => vec![
            Widget::new("media", 0, 480, WidgetKind::Media(MediaProps::default())),
            Widget::new(
                "volume",
                480,
                160,
                WidgetKind::Volume(VolumeProps::default()),
            ),
        ],
        PageTemplate::Launcher => {
            let buttons = [
                (
                    "browser",
                    "Browser",
                    "lucide:globe",
                    launch(DEFAULT_BROWSER),
                ),
                ("files", "Files", "lucide:folder", launch(DEFAULT_FILES)),
                (
                    "terminal",
                    "Terminal",
                    "lucide:square-terminal",
                    launch(DEFAULT_TERMINAL),
                ),
                ("mail", "Mail", "lucide:mail", launch(DEFAULT_MAIL)),
                ("mic", "Mic", "lucide:mic-off", ActionSpec::MicrophoneMute),
                ("lock", "Lock", "lucide:lock", ActionSpec::LockScreen),
            ];
            let widths = [107u16, 107, 106, 107, 107, 106];
            let mut x = 0;
            buttons
                .into_iter()
                .zip(widths)
                .map(|((wid, label, icon, action), w)| {
                    let widget = button(wid, x, w, label, icon).with_tap(action);
                    x += w;
                    widget
                })
                .collect()
        }
        PageTemplate::News => vec![Widget::new(
            "news",
            0,
            STRIP_WIDTH,
            WidgetKind::News(NewsProps::default()),
        )],
        PageTemplate::Focus => vec![
            Widget::new("timer", 0, 220, WidgetKind::Timer(TimerProps::default())),
            Widget::new(
                "weather",
                220,
                220,
                WidgetKind::Weather(WeatherProps {
                    show_range: true,
                    ..WeatherProps::default()
                }),
            ),
            Widget::new(
                "clock",
                440,
                200,
                WidgetKind::Clock(ClockProps {
                    show_seconds: true,
                    ..ClockProps::default()
                }),
            ),
        ],
        PageTemplate::Home => vec![
            Widget::new(
                "weather",
                0,
                220,
                WidgetKind::Weather(WeatherProps {
                    show_range: true,
                    ..WeatherProps::default()
                }),
            ),
            entity(
                "light",
                220,
                140,
                "light.living_room",
                "Light",
                "lucide:lightbulb",
            ),
            entity("lamp", 360, 140, "light.desk", "Desk", "lucide:lamp-desk"),
            entity("fan", 500, 140, "switch.fan", "Fan", "lucide:fan"),
        ],
        PageTemplate::Streaming => {
            // "Mic/Aux" and "Scene" are the names a fresh English OBS uses.
            let buttons = [
                (
                    "record",
                    "Record",
                    "lucide:circle-dot",
                    obs(ObsCommand::ToggleRecord, None),
                ),
                (
                    "pause",
                    "Pause",
                    "lucide:circle-pause",
                    obs(ObsCommand::PauseRecord, None),
                ),
                (
                    "stream",
                    "Stream",
                    "lucide:radio",
                    obs(ObsCommand::ToggleStream, None),
                ),
                (
                    "mic",
                    "Mic",
                    "lucide:mic",
                    obs(ObsCommand::ToggleMute, Some("Mic/Aux")),
                ),
                (
                    "scene",
                    "Scene",
                    "lucide:clapperboard",
                    obs(ObsCommand::SetScene, Some("Scene")),
                ),
                (
                    "clip",
                    "Clip",
                    "lucide:film",
                    obs(ObsCommand::SaveReplay, None),
                ),
            ];
            let widths = [107u16, 107, 106, 107, 107, 106];
            let mut x = 0;
            buttons
                .into_iter()
                .zip(widths)
                .map(|((wid, label, icon, action), w)| {
                    let widget = button(wid, x, w, label, icon).with_tap(action);
                    x += w;
                    widget
                })
                .collect()
        }
        PageTemplate::Blank => Vec::new(),
    };
    Page {
        id: id.into(),
        name: template.name().into(),
        hidden: false,
        auto_return_seconds: None,
        background: None,
        widgets,
    }
}

fn metric(
    id: &str,
    x: u16,
    w: u16,
    source: &str,
    secondary: Option<&str>,
    style: MetricStyle,
) -> Widget {
    Widget::new(
        id,
        x,
        w,
        WidgetKind::Metric(MetricProps {
            source: source.into(),
            style,
            secondary: secondary.map(Into::into),
            ..MetricProps::default()
        }),
    )
}

pub fn button(id: &str, x: u16, w: u16, label: &str, icon: &str) -> Widget {
    Widget::new(
        id,
        x,
        w,
        WidgetKind::Button(ButtonProps {
            label: label.into(),
            icon: Some(icon.into()),
            ..ButtonProps::default()
        }),
    )
}

fn entity(id: &str, x: u16, w: u16, entity_id: &str, label: &str, icon: &str) -> Widget {
    Widget::new(
        id,
        x,
        w,
        WidgetKind::Entity(EntityProps {
            entity_id: entity_id.into(),
            label: Some(label.into()),
            icon: Some(icon.into()),
        }),
    )
}

fn obs(command: ObsCommand, target: Option<&str>) -> ActionSpec {
    ActionSpec::Obs {
        command,
        target: target.map(Into::into),
    }
}

fn launch(target: &str) -> ActionSpec {
    ActionSpec::LaunchApp {
        target: target.into(),
    }
}
