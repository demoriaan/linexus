use super::Ctx;
use crate::canvas::{Bounds, Canvas};
use crate::format::{self, Word};
use crate::text::{FontFace, TextStyle};
use nexus_config::{EntityProps, Widget};

pub fn draw(canvas: &mut Canvas, b: Bounds, widget: &Widget, props: &EntityProps, ctx: &Ctx) {
    let palette = ctx.palette;
    let accent = ctx.accent(widget, palette.series[1]);
    let entity = ctx.live.entities.get(&props.entity_id);
    let on = entity.and_then(|e| e.on);
    let tile = b.inset(3.0, 4.0);
    match on {
        Some(true) => canvas.round_rect(tile, 8.0, accent, 0.16),
        _ => canvas.round_rect(tile, 8.0, palette.surface, 1.0),
    }
    let pressed = ctx.pressed(widget);
    if pressed > 0.0 {
        canvas.round_rect(tile, 8.0, accent, 0.25 * pressed);
    }
    let icon_color = match on {
        Some(true) => accent,
        _ => palette.text_dim,
    };
    let icon = props
        .icon
        .as_deref()
        .unwrap_or_else(|| default_icon(&props.entity_id));
    canvas.icon(icon, tile.x + 8.0, 14.0, 20.0, icon_color, 1.0);
    let text_x = tile.x + 36.0;
    let room = tile.right() - text_x - 6.0;
    let name = props
        .label
        .clone()
        .or_else(|| entity.map(|e| e.name.clone()).filter(|n| !n.is_empty()))
        .unwrap_or_else(|| props.entity_id.clone());
    canvas.text_fit(
        text_x,
        22.0,
        &name,
        &TextStyle::new(FontFace::SemiBold, 13.0, palette.text),
        room,
    );
    let locale = ctx.live.locale;
    let state = match (entity, on) {
        (None, _) => format::word(Word::Unavailable, locale).to_string(),
        (Some(_), Some(true)) => format::word(Word::On, locale).to_string(),
        (Some(_), Some(false)) => format::word(Word::Off, locale).to_string(),
        (Some(e), None) => match &e.unit {
            Some(unit) => format!("{} {unit}", e.state),
            None => e.state.clone(),
        },
    };
    let state_color = if on == Some(true) {
        accent
    } else {
        palette.text_dim
    };
    canvas.text_fit(
        text_x,
        36.0,
        &state.to_uppercase(),
        &TextStyle::new(FontFace::SemiBold, 10.0, state_color).tracking(0.6),
        room,
    );
}

fn default_icon(entity_id: &str) -> &'static str {
    match entity_id.split('.').next().unwrap_or_default() {
        "light" => "lightbulb",
        "switch" | "input_boolean" => "toggle-right",
        "fan" => "fan",
        "climate" => "thermometer",
        "cover" => "blinds",
        "lock" => "lock",
        "media_player" => "speaker",
        "sensor" => "activity",
        "binary_sensor" => "radio",
        "scene" => "sparkles",
        "script" => "scroll-text",
        _ => "house",
    }
}
