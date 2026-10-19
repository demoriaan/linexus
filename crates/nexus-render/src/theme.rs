//! Colour palettes for the strip.

use crate::Rgb;
use nexus_config::Theme;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub bg_top: Rgb,
    pub bg_bottom: Rgb,
    /// Resting fill of tiles and tracks.
    pub surface: Rgb,
    /// Track behind bars and sliders.
    pub track: Rgb,
    pub divider: Rgb,
    pub text: Rgb,
    pub text_soft: Rgb,
    pub text_dim: Rgb,
    pub accent: Rgb,
    pub good: Rgb,
    pub warn: Rgb,
    pub critical: Rgb,
    /// Colours for data series (CPU, GPU, RAM, network, ...).
    pub series: [Rgb; 6],
    pub light: bool,
}

pub const PRESETS: [&str; 6] = ["midnight", "aurora", "ember", "neon", "mono", "daylight"];

impl Palette {
    pub fn for_theme(theme: &Theme) -> Self {
        let mut palette = Self::preset(&theme.preset);
        if let Some(accent) = theme.accent {
            palette.accent = Rgb::from(accent);
        }
        palette
    }

    pub fn preset(name: &str) -> Self {
        let base = Self::midnight();
        match name {
            "aurora" => Self {
                bg_top: Rgb::hex(0x0a1a1f),
                bg_bottom: Rgb::hex(0x040b0e),
                surface: Rgb::hex(0x0f2329),
                track: Rgb::hex(0x1b3238),
                divider: Rgb::hex(0x1d3a40),
                accent: Rgb::hex(0x3ee6ca),
                series: [
                    Rgb::hex(0x3ee6ca),
                    Rgb::hex(0xffc75a),
                    Rgb::hex(0xa98bff),
                    Rgb::hex(0x58b9ff),
                    Rgb::hex(0xff7a93),
                    Rgb::hex(0x9be564),
                ],
                ..base
            },
            "ember" => Self {
                bg_top: Rgb::hex(0x1a1310),
                bg_bottom: Rgb::hex(0x0b0806),
                surface: Rgb::hex(0x241b16),
                track: Rgb::hex(0x33271f),
                divider: Rgb::hex(0x3a2c22),
                text: Rgb::hex(0xfff3ea),
                text_soft: Rgb::hex(0xdcc9bb),
                text_dim: Rgb::hex(0x9c8676),
                accent: Rgb::hex(0xff8a3d),
                series: [
                    Rgb::hex(0xff8a3d),
                    Rgb::hex(0xffd166),
                    Rgb::hex(0xef6f6c),
                    Rgb::hex(0x7cc6fe),
                    Rgb::hex(0xc792ea),
                    Rgb::hex(0xa3e635),
                ],
                ..base
            },
            "neon" => Self {
                bg_top: Rgb::hex(0x150722),
                bg_bottom: Rgb::hex(0x08030f),
                surface: Rgb::hex(0x22103a),
                track: Rgb::hex(0x2e1747),
                divider: Rgb::hex(0x3b1a5a),
                text_dim: Rgb::hex(0x9a82b8),
                accent: Rgb::hex(0xff3fd8),
                series: [
                    Rgb::hex(0x00f0ff),
                    Rgb::hex(0xffe600),
                    Rgb::hex(0xff3fd8),
                    Rgb::hex(0x7b61ff),
                    Rgb::hex(0xff5c5c),
                    Rgb::hex(0x39ff88),
                ],
                ..base
            },
            "mono" => Self {
                bg_top: Rgb::hex(0x111214),
                bg_bottom: Rgb::hex(0x070708),
                surface: Rgb::hex(0x1b1c1f),
                track: Rgb::hex(0x26282c),
                divider: Rgb::hex(0x2a2c31),
                accent: Rgb::hex(0xf2f3f5),
                series: [Rgb::hex(0xe8e9ec); 6],
                ..base
            },
            "daylight" => Self {
                bg_top: Rgb::hex(0xeef1f6),
                bg_bottom: Rgb::hex(0xdde3ec),
                surface: Rgb::hex(0xffffff),
                track: Rgb::hex(0xc9d2de),
                divider: Rgb::hex(0xc4ccd8),
                text: Rgb::hex(0x0d1320),
                text_soft: Rgb::hex(0x323c4e),
                text_dim: Rgb::hex(0x667186),
                accent: Rgb::hex(0x0b6bcb),
                good: Rgb::hex(0x14894b),
                warn: Rgb::hex(0xb86e00),
                critical: Rgb::hex(0xc8283c),
                series: [
                    Rgb::hex(0x0b8a7a),
                    Rgb::hex(0xc06c00),
                    Rgb::hex(0x7048e8),
                    Rgb::hex(0x0b6bcb),
                    Rgb::hex(0xd6336c),
                    Rgb::hex(0x2f9e44),
                ],
                light: true,
            },
            _ => base,
        }
    }

    fn midnight() -> Self {
        Self {
            bg_top: Rgb::hex(0x0e121b),
            bg_bottom: Rgb::hex(0x06080c),
            surface: Rgb::hex(0x151a25),
            track: Rgb::hex(0x232a38),
            divider: Rgb::hex(0x232a38),
            text: Rgb::hex(0xf3f6fb),
            text_soft: Rgb::hex(0xc5ccd9),
            text_dim: Rgb::hex(0x7d889c),
            accent: Rgb::hex(0x5aa9ff),
            good: Rgb::hex(0x4ade80),
            warn: Rgb::hex(0xffb84d),
            critical: Rgb::hex(0xff5c6c),
            series: [
                Rgb::hex(0x3ee6ca),
                Rgb::hex(0xffb84d),
                Rgb::hex(0xb98cff),
                Rgb::hex(0x5aa9ff),
                Rgb::hex(0xff6b8b),
                Rgb::hex(0x9be564),
            ],
            light: false,
        }
    }

    /// Series colour for a metric id, so CPU, GPU and RAM keep their colour
    /// across widgets and pages.
    pub fn metric_color(&self, source: &str) -> Rgb {
        let family = source.split(['.', ':']).next().unwrap_or(source);
        match family {
            "cpu" => self.series[0],
            "gpu" => self.series[1],
            "ram" | "swap" => self.series[2],
            "net" => self.series[3],
            "disk" => self.series[4],
            "hwmon" => self.series[5],
            "battery" => self.series[2],
            _ => self.accent,
        }
    }
}
