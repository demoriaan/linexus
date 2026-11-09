//! The Discord voice channel you are in: its name and the connection's
//! ping on the left, then everyone in it with bigger pictures than the
//! voice widget. A tap on someone swaps the row for their controls: mute
//! them for you, make them quieter or louder, close. A tap on yourself
//! opens your microphone's: mute, quieter, louder.

use super::{press_glow, Ctx};
use crate::canvas::{Bounds, Canvas};
use crate::format::{self, Word};
use crate::live::{DiscordInfo, DiscordLink, DiscordMember};
use crate::text::{self, FontFace, TextStyle};
use crate::Rgb;
use nexus_config::{DiscordCallProps, Widget};

const AVATAR: f32 = 30.0;
const SLOT_NAMED: f32 = 48.0;
const SLOT_BARE: f32 = 36.0;
const HEADER_MAX: f32 = 112.0;
const BUTTON: f32 = 34.0;
const BUTTON_GAP: f32 = 4.0;
/// Narrow widgets get smaller buttons, so the picture and value still fit.
const BUTTON_NARROW: f32 = 30.0;
const NARROW: f32 = 260.0;
const SPEAKING: Rgb = Rgb::hex(0x23a55a);
const BADGE: Rgb = Rgb::hex(0xf23f43);
const FAIR: Rgb = Rgb::hex(0xf0b232);

/// What a tap on the widget means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallHit {
    /// Opens this member's controls.
    Member(String),
    /// Mutes or unmutes the member for you; for yourself, your microphone.
    Mute(String),
    /// The member quieter for you; for yourself, your microphone.
    Quieter(String),
    Louder(String),
    /// Closes the member's controls.
    Close,
}

fn header_width(b: Bounds) -> f32 {
    (b.w * 0.3).clamp(60.0, HEADER_MAX)
}

fn slot_width(props: &DiscordCallProps) -> f32 {
    if props.show_names {
        SLOT_NAMED
    } else {
        SLOT_BARE
    }
}

/// The controls of the focused member, right to left: close, louder,
/// quieter, mute.
fn buttons(b: Bounds) -> [Bounds; 4] {
    let (size, gap) = if b.w < NARROW {
        (BUTTON_NARROW, 3.0)
    } else {
        (BUTTON, BUTTON_GAP)
    };
    let top = (b.h - size) / 2.0;
    let at = |n: f32| {
        Bounds::new(
            b.right() - 4.0 - (n + 1.0) * size - n * gap,
            top,
            size,
            size,
        )
    };
    [at(0.0), at(1.0), at(2.0), at(3.0)]
}

/// The member shown with controls, if they are still in your channel.
fn focused(info: &DiscordInfo) -> Option<&DiscordMember> {
    let id = info.focus.as_deref()?;
    info.current_channel()?
        .members
        .iter()
        .find(|member| member.user_id == id)
}

/// How many pictures fit, and whether the last slot shows "+N" instead.
fn fitting(room: f32, slot: f32, members: usize) -> (usize, bool) {
    let slots = (room / slot).floor().max(0.0) as usize;
    if members <= slots {
        (members, false)
    } else {
        (slots.saturating_sub(1), slots > 0)
    }
}

/// What a finger at `x` (absolute strip X) touches.
pub fn discord_call_hit(
    widget_x: u16,
    widget_w: u16,
    props: &DiscordCallProps,
    info: &DiscordInfo,
    x: u16,
) -> Option<CallHit> {
    let b = Bounds::new(
        f32::from(widget_x),
        0.0,
        f32::from(widget_w),
        f32::from(nexus_config::STRIP_HEIGHT),
    );
    let x = f32::from(x);
    if let Some(member) = focused(info) {
        let [_close, louder, quieter, mute] = buttons(b);
        let inside = |button: Bounds| {
            x >= button.x - BUTTON_GAP / 2.0 && x < button.right() + BUTTON_GAP / 2.0
        };
        let id = member.user_id.clone();
        return Some(if inside(mute) {
            CallHit::Mute(id)
        } else if inside(quieter) {
            CallHit::Quieter(id)
        } else if inside(louder) {
            CallHit::Louder(id)
        } else {
            // The close button and the rest of the row both close.
            CallHit::Close
        });
    }
    let channel = info.current_channel()?;
    let left = b.x + header_width(b);
    let slot = slot_width(props);
    let (shown, _) = fitting(b.right() - left - 2.0, slot, channel.members.len());
    let index = ((x - left) / slot).floor();
    if index < 0.0 || index as usize >= shown {
        return None;
    }
    Some(CallHit::Member(
        channel.members[index as usize].user_id.clone(),
    ))
}

pub fn draw(canvas: &mut Canvas, b: Bounds, widget: &Widget, props: &DiscordCallProps, ctx: &Ctx) {
    let palette = ctx.palette;
    let accent = ctx.accent(widget, Rgb::hex(0x5865f2));
    let locale = ctx.live.locale;
    let message = |canvas: &mut Canvas, icon: &str, word: Word| {
        canvas.icon(icon, b.x + 12.0, 14.0, 20.0, palette.text_dim, 1.0);
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
        message(canvas, "headphones", Word::DiscordOffline);
        return;
    };
    if info.channel.is_none() {
        message(canvas, "phone-off", Word::NotInVoice);
        return;
    }
    press_glow(canvas, b, accent, ctx.pressed(widget));
    // The controls take the whole row, header included.
    if let Some(member) = focused(info) {
        draw_controls(canvas, ctx, b, info, member);
        return;
    }
    let channel = info.current_channel();

    // Header: channel name, then the connection.
    let header = header_width(b);
    let name = channel.map_or("…", |c| c.name.as_str());
    let name_style = TextStyle::new(FontFace::SemiBold, 13.0, palette.text);
    let link = info.link.filter(|_| props.show_ping);
    let name_y = if link.is_some() { 20.0 } else { 29.0 };
    canvas.text_fit(b.x + 8.0, name_y, name, &name_style, header - 12.0);
    if let Some(link) = link {
        draw_link(canvas, ctx, b.x + 8.0, link);
    }
    canvas.fill(
        Bounds::new(b.x + header - 4.0, 9.0, 1.0, 30.0),
        palette.text_dim,
        0.35,
    );

    let Some(channel) = channel else {
        return;
    };
    let left = b.x + header;
    let slot = slot_width(props);
    let (shown, more) = fitting(b.right() - left - 2.0, slot, channel.members.len());
    for (n, member) in channel.members[..shown].iter().enumerate() {
        let x = left + n as f32 * slot;
        draw_member(canvas, ctx, x, slot, props.show_names, member);
    }
    if more {
        let extra = channel.members.len() - shown;
        let x = left + shown as f32 * slot + slot / 2.0;
        let style = TextStyle::new(FontFace::Bold, 13.0, palette.text_soft);
        let y = if props.show_names { 24.0 } else { 29.0 };
        canvas.text_centered(x, y, &format!("+{extra}"), &style);
    }
}

/// Signal bars and the ping, below the channel name.
fn draw_link(canvas: &mut Canvas, ctx: &Ctx, x: f32, link: DiscordLink) {
    let palette = ctx.palette;
    let (icon, color) = match link.bars() {
        3 => ("signal-high", SPEAKING),
        2 => ("signal-medium", FAIR),
        1 => ("signal-low", BADGE),
        _ => ("signal-zero", palette.text_dim),
    };
    canvas.icon(icon, x - 1.0, 27.0, 13.0, color, 1.0);
    let label = match (link.connected, link.ping_ms) {
        (true, Some(ping)) => format!("{ping} ms"),
        _ => "…".to_string(),
    };
    let style = TextStyle::new(FontFace::Medium, 11.0, palette.text_soft).tabular();
    canvas.text(x + 15.0, 38.0, &label, &style);
}

fn draw_member(
    canvas: &mut Canvas,
    ctx: &Ctx,
    x: f32,
    slot: f32,
    named: bool,
    member: &DiscordMember,
) {
    let palette = ctx.palette;
    let top = if named { 3.0 } else { 9.0 };
    let ax = x + (slot - AVATAR) / 2.0;
    draw_avatar(canvas, ctx, ax, top, member);
    if named {
        let style = TextStyle::new(FontFace::SemiBold, 9.5, palette.text_soft);
        let name = text::printable(&member.name, style.face);
        if text::measure(&name, &style) <= slot - 4.0 {
            canvas.text_centered(x + slot / 2.0, 45.0, &name, &style);
        } else {
            canvas.text_fit(x + 2.0, 45.0, &name, &style, slot - 4.0);
        }
    }
}

/// A round picture with a green ring while speaking, faded while muted for
/// you or deafened, and a badge for muted, deafened or muted-for-you.
fn draw_avatar(canvas: &mut Canvas, ctx: &Ctx, x: f32, y: f32, member: &DiscordMember) {
    let palette = ctx.palette;
    let r = AVATAR / 2.0;
    let (cx, cy) = (x + r, y + r);
    if member.speaking && !member.local_muted {
        canvas.circle(cx, cy, r + 2.5, SPEAKING, 1.0);
        canvas.circle(cx, cy, r + 0.8, palette.bg_bottom, 1.0);
    }
    let dim = if member.deafened || member.local_muted {
        0.5
    } else {
        1.0
    };
    match &member.avatar {
        Some(image) => canvas.draw_image(
            Bounds::new(x, y, AVATAR, AVATAR),
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
            let style = TextStyle::new(FontFace::Bold, 15.0, palette.text);
            canvas.text_centered(cx, cy + 5.5, &initial, &style);
        }
    }
    let badge = if member.local_muted {
        Some(("volume-x", palette.text_dim))
    } else if member.deafened {
        Some(("headphone-off", BADGE))
    } else if member.muted {
        Some(("mic-off", BADGE))
    } else {
        None
    };
    if let Some((icon, color)) = badge {
        let (bx, by) = (x + AVATAR - 4.0, y + AVATAR - 4.0);
        canvas.circle(bx, by, 7.0, palette.bg_bottom, 1.0);
        canvas.circle(bx, by, 6.0, color, 1.0);
        canvas.icon(icon, bx - 4.5, by - 4.5, 9.0, Rgb::hex(0xffffff), 1.0);
    }
}

/// The focused member: picture, name, their volume for you (your
/// microphone's for yourself), and buttons.
fn draw_controls(
    canvas: &mut Canvas,
    ctx: &Ctx,
    b: Bounds,
    info: &DiscordInfo,
    member: &DiscordMember,
) {
    let palette = ctx.palette;
    let [close, louder, quieter, mute] = buttons(b);
    let x = b.x + 6.0;
    if mute.x - x >= AVATAR + 4.0 {
        draw_avatar(canvas, ctx, x, 9.0, member);
    }
    let text_x = x + AVATAR + 8.0;
    let room = (mute.x - 6.0 - text_x).max(0.0);
    let name_style = TextStyle::new(FontFace::SemiBold, 13.0, palette.text);
    canvas.text_fit(text_x, 21.0, &member.name, &name_style, room);
    let yourself = info.self_id.as_deref() == Some(member.user_id.as_str());
    let (muted, volume) = if yourself {
        (info.self_muted, u16::from(info.input_volume))
    } else {
        (member.local_muted, member.volume)
    };
    // For yourself, the microphone icon on the first button says what
    // the value is.
    let value = if muted {
        format::word(Word::Muted, ctx.live.locale).to_string()
    } else {
        format!("{volume} %")
    };
    let value_style = TextStyle::new(FontFace::Medium, 12.0, palette.text_soft).tabular();
    canvas.text_fit(text_x, 38.0, &value, &value_style, room);

    let tile = |canvas: &mut Canvas, bounds: Bounds, icon: &str, lit: Option<Rgb>| {
        match lit {
            Some(color) => canvas.round_rect(bounds, 8.0, color, 0.9),
            None => canvas.round_rect(bounds, 8.0, palette.surface, 0.9),
        }
        let color = if lit.is_some() {
            Rgb::hex(0xffffff)
        } else {
            palette.text
        };
        let size = (bounds.w * 0.53).round();
        let inset = (bounds.w - size) / 2.0;
        canvas.icon(icon, bounds.x + inset, bounds.y + inset, size, color, 1.0);
    };
    let (mute_icon, mute_lit) = match (yourself, muted) {
        (true, true) => ("mic-off", Some(BADGE)),
        (true, false) => ("mic", None),
        (false, true) => ("volume-x", Some(BADGE)),
        (false, false) => ("volume-2", None),
    };
    tile(canvas, mute, mute_icon, mute_lit);
    tile(canvas, quieter, "minus", None);
    tile(canvas, louder, "plus", None);
    tile(canvas, close, "x", None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::DiscordVoiceChannel;
    use std::collections::HashMap;

    fn info(members: usize) -> DiscordInfo {
        DiscordInfo {
            channel: Some("2".into()),
            guild: Some("g".into()),
            guilds: HashMap::from([(
                "g".to_string(),
                vec![DiscordVoiceChannel {
                    id: "2".into(),
                    name: "Gaming".into(),
                    members: (0..members)
                        .map(|n| DiscordMember {
                            user_id: n.to_string(),
                            name: format!("user {n}"),
                            volume: 100,
                            ..DiscordMember::default()
                        })
                        .collect(),
                }],
            )]),
            ..DiscordInfo::default()
        }
    }

    #[test]
    fn taps_pick_members_then_their_controls() {
        let props = DiscordCallProps::default();
        let mut info = info(3);
        // Header is 30 % of 320 = 96 px; slots are 48 px from there.
        assert_eq!(discord_call_hit(0, 320, &props, &info, 20), None);
        assert_eq!(
            discord_call_hit(0, 320, &props, &info, 100),
            Some(CallHit::Member("0".into()))
        );
        assert_eq!(
            discord_call_hit(0, 320, &props, &info, 150),
            Some(CallHit::Member("1".into()))
        );
        assert_eq!(discord_call_hit(0, 320, &props, &info, 300), None);

        info.focus = Some("1".into());
        let [close, louder, quieter, mute] = buttons(Bounds::new(0.0, 0.0, 320.0, 48.0));
        let at = |b: Bounds| b.center_x() as u16;
        assert_eq!(
            discord_call_hit(0, 320, &props, &info, at(mute)),
            Some(CallHit::Mute("1".into()))
        );
        assert_eq!(
            discord_call_hit(0, 320, &props, &info, at(quieter)),
            Some(CallHit::Quieter("1".into()))
        );
        assert_eq!(
            discord_call_hit(0, 320, &props, &info, at(louder)),
            Some(CallHit::Louder("1".into()))
        );
        assert_eq!(
            discord_call_hit(0, 320, &props, &info, at(close)),
            Some(CallHit::Close)
        );
        // Someone who left takes their controls along.
        info.focus = Some("9".into());
        assert_eq!(
            discord_call_hit(0, 320, &props, &info, 100),
            Some(CallHit::Member("0".into()))
        );
    }

    #[test]
    fn a_full_channel_ends_with_a_counter() {
        assert_eq!(fitting(200.0, 48.0, 3), (3, false));
        assert_eq!(fitting(200.0, 48.0, 9), (3, true));
        assert_eq!(fitting(20.0, 48.0, 2), (0, false));
        let props = DiscordCallProps::default();
        let info = info(9);
        // Only the shown pictures answer taps; the counter does not. The
        // header takes 30 % of 300 px.
        let last = 90 + 3 * 48 + 10;
        assert_eq!(discord_call_hit(0, 300, &props, &info, last), None);
    }

    #[test]
    fn the_link_rates_the_ping() {
        let link = |ping| DiscordLink {
            connected: true,
            ping_ms: Some(ping),
        };
        assert_eq!(link(40).bars(), 3);
        assert_eq!(link(180).bars(), 2);
        assert_eq!(link(400).bars(), 1);
        assert_eq!(DiscordLink::default().bars(), 0);
    }
}
