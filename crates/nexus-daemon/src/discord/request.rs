//! Requests from the strip as Discord RPC commands.

use super::tracked::Tracked;
use super::{Request, VOLUME_STEP};
use nexus_config::DiscordCommand;
use serde_json::{json, Value};

/// The RPC command for a request; None when there is nothing to do
/// (joining the channel you are already in, push to talk, which the engine
/// turns into mute and unmute).
pub(super) fn request(request: &Request, tracked: &Tracked) -> Option<(&'static str, Value)> {
    let (command, channel, step) = match request {
        Request::Command {
            command,
            channel,
            step,
        } => (*command, channel.as_deref(), *step),
        Request::SetMute(mute) => return Some(("SET_VOICE_SETTINGS", json!({ "mute": mute }))),
        Request::InputVolume(volume) => {
            return Some((
                "SET_VOICE_SETTINGS",
                json!({ "input": { "volume": (*volume).min(100) } }),
            ))
        }
        Request::OutputVolume(volume) => {
            return Some((
                "SET_VOICE_SETTINGS",
                json!({ "output": { "volume": (*volume).min(200) } }),
            ))
        }
        Request::UserVolume { user, volume } => {
            return Some((
                "SET_USER_VOICE_SETTINGS",
                json!({ "user_id": user, "volume": (*volume).min(200) }),
            ))
        }
        Request::UserMute { user, mute } => {
            return Some((
                "SET_USER_VOICE_SETTINGS",
                json!({ "user_id": user, "mute": mute }),
            ))
        }
    };
    let step = i32::from(step.unwrap_or(VOLUME_STEP).clamp(1, 100));
    let input = |delta: i32| {
        let volume = (i32::from(tracked.input_volume) + delta).clamp(0, 100);
        (
            "SET_VOICE_SETTINGS",
            json!({ "input": { "volume": volume } }),
        )
    };
    let output = |delta: i32| {
        let volume = (i32::from(tracked.output_volume) + delta).clamp(0, 200);
        (
            "SET_VOICE_SETTINGS",
            json!({ "output": { "volume": volume } }),
        )
    };
    Some(match command {
        DiscordCommand::ToggleMute => {
            ("SET_VOICE_SETTINGS", json!({ "mute": !tracked.self_muted }))
        }
        DiscordCommand::ToggleDeafen => (
            "SET_VOICE_SETTINGS",
            json!({ "deaf": !tracked.self_deafened }),
        ),
        DiscordCommand::JoinVoice => {
            let channel = channel?;
            if tracked.channel.as_deref() == Some(channel) {
                return None;
            }
            // `force` moves you out of another channel; the tap on the
            // strip is the user's approval to do so.
            (
                "SELECT_VOICE_CHANNEL",
                json!({ "channel_id": channel, "force": true }),
            )
        }
        DiscordCommand::LeaveVoice => ("SELECT_VOICE_CHANNEL", json!({ "channel_id": null })),
        DiscordCommand::PushToTalk => return None,
        DiscordCommand::InputVolumeUp => input(step),
        DiscordCommand::InputVolumeDown => input(-step),
        DiscordCommand::OutputVolumeUp => output(step),
        DiscordCommand::OutputVolumeDown => output(-step),
        DiscordCommand::ToggleVoiceMode => {
            let mode = if tracked.push_to_talk {
                "VOICE_ACTIVITY"
            } else {
                "PUSH_TO_TALK"
            };
            ("SET_VOICE_SETTINGS", json!({ "mode": { "type": mode } }))
        }
        DiscordCommand::ToggleNoiseSuppression => (
            "SET_VOICE_SETTINGS",
            json!({ "noise_suppression": !tracked.noise_suppression }),
        ),
        DiscordCommand::ToggleEchoCancellation => (
            "SET_VOICE_SETTINGS",
            json!({ "echo_cancellation": !tracked.echo_cancellation }),
        ),
        DiscordCommand::ToggleAutomaticGain => (
            "SET_VOICE_SETTINGS",
            json!({ "automatic_gain_control": !tracked.automatic_gain }),
        ),
    })
}
