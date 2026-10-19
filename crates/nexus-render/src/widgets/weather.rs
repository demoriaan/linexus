use super::Ctx;
use crate::canvas::{Bounds, Canvas};
use crate::format::{self, Word};
use crate::text::{self, FontFace, TextStyle};
use nexus_config::WeatherProps;

/// Key under which the daemon stores weather for `props`: the widget's own
/// location, or the global one (empty key).
pub fn weather_key(props: &WeatherProps) -> String {
    props
        .location
        .as_ref()
        .map(|loc| loc.name.clone())
        .unwrap_or_default()
}

pub fn draw(canvas: &mut Canvas, b: Bounds, props: &WeatherProps, ctx: &Ctx) {
    let palette = ctx.palette;
    let locale = ctx.live.locale;
    let Some(weather) = ctx.live.weather.get(&weather_key(props)) else {
        canvas.icon("cloud-sun", b.x + 10.0, 12.0, 24.0, palette.text_dim, 1.0);
        let style = TextStyle::new(FontFace::SemiBold, 13.0, palette.text_dim);
        canvas.text_fit(
            b.x + 42.0,
            29.0,
            format::word(Word::SetLocation, locale),
            &style,
            b.w - 48.0,
        );
        return;
    };
    let (icon, description) = format::weather(weather.code, weather.is_day, locale);
    let icon_color = match icon {
        "sun" | "cloud-sun" => palette.series[1],
        "moon" | "cloud-moon" => palette.series[2],
        "cloud-lightning" => palette.warn,
        _ => palette.series[3],
    };
    canvas.icon(icon, b.x + 8.0, 9.0, 30.0, icon_color, 1.0);
    let temp = if ctx.live.fahrenheit {
        weather.temp_c * 9.0 / 5.0 + 32.0
    } else {
        weather.temp_c
    };
    let big = TextStyle::new(FontFace::Bold, 28.0, palette.text).tabular();
    let temp_text = format!("{temp:.0}°");
    let temp_w = canvas.text(b.x + 44.0, 34.0, &temp_text, &big);
    let detail_x = b.x + 44.0 + temp_w + 10.0;
    let room = b.right() - detail_x - 6.0;
    if room < 30.0 {
        return;
    }
    canvas.text_fit(
        detail_x,
        19.0,
        &description.to_uppercase(),
        &ctx.caption(),
        room,
    );
    let mut line = String::new();
    if props.show_range {
        if let (Some(high), Some(low)) = (weather.high_c, weather.low_c) {
            let conv = |c: f32| {
                if ctx.live.fahrenheit {
                    c * 9.0 / 5.0 + 32.0
                } else {
                    c
                }
            };
            line = format!("{:.0}° / {:.0}°", conv(high), conv(low));
        }
    }
    if !weather.location.is_empty() {
        if !line.is_empty() {
            line.push_str("  ·  ");
        }
        line.push_str(&weather.location);
    }
    let style = TextStyle::new(FontFace::SemiBold, 12.0, palette.text_soft);
    let line = text::ellipsize(&line, &style, room);
    canvas.text(detail_x, 35.0, &line, &style);
}
