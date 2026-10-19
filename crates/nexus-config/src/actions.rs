use crate::model::{ActionSpec, HttpMethod, MediaKey, VolumeAction};
use nexus_actions::{Action, MediaCommand, VolumeCommand};

impl ActionSpec {
    /// The desktop-side action for this spec, or `None` for actions the
    /// daemon handles itself (page and profile navigation, brightness,
    /// Home Assistant calls, sequences).
    pub fn to_external_action(&self) -> Option<Action> {
        Some(match self {
            Self::OpenUrl { url } => Action::OpenUrl(url.clone()),
            Self::OpenPath { path } => Action::OpenPath(path.clone()),
            Self::LaunchApp { target } => Action::Launch(target.clone()),
            Self::Media { command, player } => Action::Media {
                player: player.clone(),
                command: match command {
                    MediaKey::PlayPause => MediaCommand::PlayPause,
                    MediaKey::Next => MediaCommand::Next,
                    MediaKey::Previous => MediaCommand::Previous,
                    MediaKey::Stop => MediaCommand::Stop,
                },
            },
            Self::Volume {
                command,
                step,
                percent,
            } => Action::Volume(match command {
                VolumeAction::Up => VolumeCommand::Up(step.unwrap_or(5)),
                VolumeAction::Down => VolumeCommand::Down(step.unwrap_or(5)),
                VolumeAction::Set => VolumeCommand::Set(percent.unwrap_or(50)),
                VolumeAction::ToggleMute => VolumeCommand::ToggleMute,
            }),
            Self::MicrophoneMute => Action::MicrophoneMute,
            Self::KeyboardShortcut { keys } => Action::KeyboardShortcut { keys: keys.clone() },
            Self::TextInput { text } => Action::TextInput(text.clone()),
            Self::ShellCommand { program, args } => Action::ShellCommand {
                program: program.clone(),
                args: args.clone(),
            },
            Self::HttpRequest {
                method,
                url,
                body,
                content_type,
            } => Action::HttpRequest {
                method: match method {
                    HttpMethod::Get => "GET",
                    HttpMethod::Post => "POST",
                    HttpMethod::Put => "PUT",
                }
                .into(),
                url: url.clone(),
                body: body.clone(),
                content_type: content_type.clone(),
            },
            Self::LockScreen => Action::LockScreen,
            Self::HomeAssistant { .. }
            | Self::NextPage
            | Self::PreviousPage
            | Self::GoToPage { .. }
            | Self::GoBack
            | Self::SwitchProfile { .. }
            | Self::NextProfile
            | Self::PreviousProfile
            | Self::Brightness { .. }
            | Self::DisplayOff
            | Self::Sequence { .. } => return None,
        })
    }
}
