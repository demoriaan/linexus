//! Discord voice channels: one tile per channel with its name and the
//! pictures of who sits in it. Whoever speaks gets a green ring; a red badge
//! marks a muted or deafened member. The channel you are in comes first and
//! is lit up, and a tap on a tile joins that channel. When the channels do
//! not all fit, they take turns a page at a time, with dots below.

use super::{press_glow, Ctx};
use crate::canvas::{Bounds, Canvas};
use crate::format::{self, Word};
use crate::live::{DiscordInfo, DiscordMember, DiscordVoiceChannel};
use crate::text::{self, FontFace, TextStyle};
use crate::Rgb;
use nexus_config::{DiscordVoiceProps, Widget};

const AVATAR: f32 = 22.0;
const AVATAR_GAP: f32 = 3.0;
const AVATAR_TOP: f32 = 20.0;
const PAD: f32 = 7.0;
const TILE_GAP: f32 = 5.0;
/// A channel name gets the room of its pictures, or this much when that is
/// less; longer names are cut, so a long name does not crowd out the
/// other channels.
const NAME_ROOM: f32 = 56.0;
const TILE_TOP: f32 = 3.0;
const TILE_H: f32 = 40.0;
const SPEAKING: Rgb = Rgb::hex(0x23a55a);
const BADGE: Rgb = Rgb::hex(0xf23f43);

fn name_style(color: Rgb) -> TextStyle {
    TextStyle::new(FontFace::SemiBold, 11.0, color)
}

/// One channel's tile.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Tile {
    x: f32,
    w: f32,
    /// Index into the channel list.
    index: usize,
    /// Members whose pictures fit; the rest are counted as "+N".
    shown: usize,
}

/// The channels the widget lists: the ones people are in, the one you are
/// in, or all of them with `show_empty`.
fn listed<'a>(
    channels: &'a [DiscordVoiceChannel],
    props: &DiscordVoiceProps,
    current: Option<&'a str>,
) -> impl Iterator<Item = (usize, &'a DiscordVoiceChannel)> {
    let show_empty = props.show_empty;
    channels.iter().enumerate().filter(move |(_, channel)| {
        show_empty || !channel.members.is_empty() || current == Some(channel.id.as_str())
    })
}

fn avatars_width(n: usize) -> f32 {
    if n == 0 {
        0.0
    } else {
        n as f32 * AVATAR + (n - 1) as f32 * AVATAR_GAP
    }
}

/// Width of the "+N" counter.
fn extra_width(extra: usize) -> f32 {
    if extra == 0 {
        0.0
    } else {
        AVATAR_GAP + text::measure(&format!("+{extra}"), &name_style(Rgb::hex(0))) + 2.0
    }
}

/// The listed channels split into pages that fit the widget, your own
/// channel first.
fn pages(b: Bounds, info: &DiscordInfo, props: &DiscordVoiceProps) -> Vec<Vec<Tile>> {
    let channels = info.channels_for(props.guild_id.as_deref());
    let current = info.channel.as_deref();
    let mut order: Vec<(usize, &DiscordVoiceChannel)> = listed(channels, props, current).collect();
    // Stable: the others keep Discord's order.
    order.sort_by_key(|(_, channel)| Some(channel.id.as_str()) != current);
    let left = b.x + 4.0;
    let right = b.right() - 4.0;
    let mut pages = Vec::new();
    let mut page: Vec<Tile> = Vec::new();
    let mut x = left;
    for (index, channel) in order {
        let name_w = text::measure(&channel.name, &name_style(Rgb::hex(0)));
        let total = channel.members.len();
        let width_for = |shown: usize| {
            let content = avatars_width(shown) + extra_width(total - shown);
            PAD * 2.0 + content.max(name_w.min(NAME_ROOM)).max(AVATAR)
        };
        // A channel that does not fit next to the others starts a page.
        if !page.is_empty() && x + width_for(total) > right {
            pages.push(std::mem::take(&mut page));
            x = left;
        }
        let room = right - x;
        let mut shown = total;
        while shown > 0 && width_for(shown) > room {
            shown -= 1;
        }
        let w = width_for(shown).min(room);
        page.push(Tile { x, w, index, shown });
        x += w + TILE_GAP;
    }
    if !page.is_empty() {
        pages.push(page);
    }
    pages
}

/// The page shown now: `info.rotation` steps through them.
fn layout(b: Bounds, info: &DiscordInfo, props: &DiscordVoiceProps) -> Vec<Tile> {
    let mut pages = pages(b, info, props);
    if pages.is_empty() {
        return Vec::new();
    }
    let at = (info.rotation % pages.len() as u64) as usize;
    pages.swap_remove(at)
}

/// The voice channel under `x` (absolute strip X), if any.
pub fn discord_channel_at(
    widget_x: u16,
    widget_w: u16,
    props: &DiscordVoiceProps,
    info: &DiscordInfo,
    x: u16,
) -> Option<String> {
    let b = Bounds::new(
        f32::from(widget_x),
        0.0,
        f32::from(widget_w),
        f32::from(nexus_config::STRIP_HEIGHT),
    );
    let x = f32::from(x);
    let channels = info.channels_for(props.guild_id.as_deref());
    layout(b, info, props)
        .into_iter()
        .find(|tile| x >= tile.x - TILE_GAP / 2.0 && x < tile.x + tile.w + TILE_GAP / 2.0)
        .map(|tile| channels[tile.index].id.clone())
}

pub fn draw(canvas: &mut Canvas, b: Bounds, widget: &Widget, props: &DiscordVoiceProps, ctx: &Ctx) {
    let palette = ctx.palette;
    let accent = ctx.accent(widget, Rgb::hex(0x5865f2));
    let locale = ctx.live.locale;
    let message = |canvas: &mut Canvas, word: Word| {
        canvas.icon("headphones", b.x + 12.0, 14.0, 20.0, palette.text_dim, 1.0);
        let style = TextStyle::new(FontFace::SemiBold, 14.0, palette.text_dim);
        canvas.text_fit(
            b.x + 40.0,
            29.0,
            format::word(word, locale),
            &style,
            b.w - 46.0,
        );
    };
    let Some(info) = ctx.live.discord.as_ref() else {
        message(canvas, Word::DiscordOffline);
        return;
    };
    let pages = pages(b, info, props).len();
    let tiles = layout(b, info, props);
    if tiles.is_empty() {
        message(canvas, Word::NobodyInVoice);
        return;
    }
    press_glow(canvas, b, accent, ctx.pressed(widget));
    if pages > 1 {
        // Which page shows, as dots below the tiles.
        let current = (info.rotation % pages as u64) as usize;
        let step = 7.0;
        let start = b.center_x() - step * (pages - 1) as f32 / 2.0;
        for n in 0..pages {
            let (color, alpha) = if n == current {
                (accent, 1.0)
            } else {
                (palette.text_dim, 0.6)
            };
            canvas.circle(start + n as f32 * step, 46.0, 1.6, color, alpha);
        }
    }
    let channels = info.channels_for(props.guild_id.as_deref());
    for tile in tiles {
        let channel = &channels[tile.index];
        let current = info.channel.as_deref() == Some(channel.id.as_str());
        let bounds = Bounds::new(tile.x, TILE_TOP, tile.w, TILE_H);
        if current {
            canvas.round_rect(bounds, 8.0, accent, 0.28);
            canvas.round_rect_stroke(bounds, 8.0, accent, 0.9, 1.5);
        } else {
            canvas.round_rect(bounds, 8.0, palette.surface, 0.7);
        }
        let name_color = if current {
            palette.text
        } else {
            palette.text_soft
        };
        canvas.text_fit(
            tile.x + PAD,
            15.0,
            &channel.name,
            &name_style(name_color),
            tile.w - PAD * 2.0,
        );
        if channel.members.is_empty() {
            canvas.icon(
                "users",
                tile.x + PAD,
                AVATAR_TOP + 2.0,
                16.0,
                palette.text_dim,
                0.6,
            );
            continue;
        }
        let mut x = tile.x + PAD;
        for member in &channel.members[..tile.shown] {
            draw_member(canvas, ctx, x, member);
            x += AVATAR + AVATAR_GAP;
        }
        let extra = channel.members.len() - tile.shown;
        if extra > 0 {
            let style = name_style(palette.text_soft);
            let x = if tile.shown == 0 {
                tile.x + PAD
            } else {
                x - AVATAR_GAP + 3.0
            };
            canvas.text(x, AVATAR_TOP + 15.0, &format!("+{extra}"), &style);
        }
    }
}

fn draw_member(canvas: &mut Canvas, ctx: &Ctx, x: f32, member: &DiscordMember) {
    let palette = ctx.palette;
    let (cx, cy, r) = (x + AVATAR / 2.0, AVATAR_TOP + AVATAR / 2.0, AVATAR / 2.0);
    if member.speaking {
        canvas.circle(cx, cy, r + 2.0, SPEAKING, 1.0);
        canvas.circle(cx, cy, r + 0.5, palette.bg_bottom, 1.0);
    }
    let dim = if member.deafened { 0.55 } else { 1.0 };
    match &member.avatar {
        Some(image) => canvas.draw_image(
            Bounds::new(x, AVATAR_TOP, AVATAR, AVATAR),
            image.as_ref().as_ref(),
            r,
            dim,
        ),
        None => {
            canvas.circle(cx, cy, r, ctx.series_for(&member.user_id), dim);
            let initial: String = member
                .name
                .chars()
                .take(1)
                .collect::<String>()
                .to_uppercase();
            let style = TextStyle::new(FontFace::Bold, 12.0, palette.text);
            canvas.text_centered(cx, cy + 4.5, &initial, &style);
        }
    }
    if member.muted || member.deafened {
        let (bx, by) = (x + AVATAR - 3.0, AVATAR_TOP + AVATAR - 3.0);
        canvas.circle(bx, by, 5.5, palette.bg_bottom, 1.0);
        canvas.circle(bx, by, 4.5, BADGE, 1.0);
        let icon = if member.deafened {
            "headphone-off"
        } else {
            "mic-off"
        };
        canvas.icon(icon, bx - 3.5, by - 3.5, 7.0, Rgb::hex(0xffffff), 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn member(id: &str) -> DiscordMember {
        DiscordMember {
            user_id: id.into(),
            name: format!("user {id}"),
            ..DiscordMember::default()
        }
    }

    fn info(channels: Vec<DiscordVoiceChannel>) -> DiscordInfo {
        DiscordInfo {
            channel: Some("2".into()),
            guild: Some("g".into()),
            guilds: HashMap::from([("g".to_string(), channels)]),
            ..DiscordInfo::default()
        }
    }

    fn channel(id: &str, members: usize) -> DiscordVoiceChannel {
        DiscordVoiceChannel {
            id: id.into(),
            name: format!("Channel {id}"),
            members: (0..members).map(|n| member(&n.to_string())).collect(),
        }
    }

    #[test]
    fn lists_occupied_channels_and_yours_and_finds_taps() {
        let info = info(vec![
            channel("1", 2),
            channel("2", 0),
            channel("3", 0),
            channel("4", 1),
        ]);
        let props = DiscordVoiceProps::default();
        let b = Bounds::new(100.0, 0.0, 400.0, 48.0);
        let ids: Vec<_> = layout(b, &info, &props)
            .iter()
            .map(|tile| info.channels_for(None)[tile.index].id.clone())
            .collect();
        // Your channel ("2") comes first.
        assert_eq!(ids, ["2", "1", "4"]);
        assert_eq!(
            discord_channel_at(100, 400, &props, &info, 106).as_deref(),
            Some("2")
        );
        assert_eq!(discord_channel_at(100, 400, &props, &info, 499), None);
        let all = DiscordVoiceProps {
            show_empty: true,
            ..props
        };
        assert_eq!(layout(b, &info, &all).len(), 4);
    }

    #[test]
    fn crowded_channels_fit_with_a_counter() {
        let info = info(vec![channel("1", 30)]);
        let b = Bounds::new(0.0, 0.0, 200.0, 48.0);
        let tiles = layout(b, &info, &DiscordVoiceProps::default());
        assert_eq!(tiles.len(), 1);
        assert!(tiles[0].shown < 30 && tiles[0].shown > 0);
        assert!(tiles[0].x + tiles[0].w <= 196.0);
    }

    #[test]
    fn channels_that_do_not_fit_take_turns() {
        let mut info = info(vec![
            channel("1", 2),
            channel("2", 1),
            channel("3", 3),
            channel("4", 2),
        ]);
        info.guilds.get_mut("g").unwrap()[0].name = "A channel with a very long name indeed".into();
        let props = DiscordVoiceProps::default();
        let b = Bounds::new(0.0, 0.0, 184.0, 48.0);
        let pages = pages(b, &info, &props);
        assert!(pages.len() >= 2, "{pages:?}");
        // The long name does not take more than its share: two channels
        // share the first page.
        assert_eq!(pages[0].len(), 2, "{pages:?}");
        let ids = |rotation: u64| {
            let info = DiscordInfo {
                rotation,
                ..info.clone()
            };
            layout(b, &info, &props)
                .iter()
                .map(|tile| info.channels_for(None)[tile.index].id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(0)[0], "2");
        assert_ne!(ids(0), ids(1));
        assert_eq!(ids(0), ids(pages.len() as u64));
        for page in &pages {
            assert!(page.iter().all(|tile| tile.x + tile.w <= 180.5));
        }
    }

    #[test]
    fn a_widget_follows_your_server_unless_it_names_one() {
        let info = info(vec![channel("1", 1)]);
        assert_eq!(info.channels_for(None).len(), 1);
        assert!(info.channels_for(Some("other")).is_empty());
    }
}
