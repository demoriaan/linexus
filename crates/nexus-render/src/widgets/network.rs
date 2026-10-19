use super::Ctx;
use crate::canvas::{Bounds, Canvas};
use crate::format;
use crate::text::{self, FontFace, TextStyle};

pub fn draw(canvas: &mut Canvas, b: Bounds, ctx: &Ctx) {
    let palette = ctx.palette;
    let rows = [
        ("net.down", "arrow-down", palette.metric_color("net"), 20.0),
        ("net.up", "arrow-up", palette.series[4], 40.0),
    ];
    let value = TextStyle::new(FontFace::Bold, 17.0, palette.text).tabular();
    let unit_style = TextStyle::new(FontFace::SemiBold, 11.0, palette.text_dim);
    for (id, icon, color, baseline) in rows {
        canvas.icon(icon, b.x + 8.0, baseline - 12.0, 13.0, color, 1.0);
        let (number, unit) = ctx
            .live
            .metric(id)
            .map(|m| format::value(m.value, m.unit, false))
            .unwrap_or_else(|| ("–".into(), String::new()));
        let unit_w = text::measure(&unit, &unit_style);
        let unit_x = b.right() - 8.0 - unit_w;
        canvas.text(unit_x, baseline, &unit, &unit_style);
        canvas.text_right(unit_x - 3.0, baseline, &number, &value);
    }
}
