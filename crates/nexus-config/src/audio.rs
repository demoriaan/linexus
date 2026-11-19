//! Sound on the strip: the output switcher, the mixer of playing apps and
//! the microphone, plus how their devices are named and ordered.

use serde::{Deserialize, Serialize};

/// Most devices `settings.audio.devices` may list.
pub const MAX_AUDIO_DEVICES: usize = 64;
/// Most apps a mixer may pin, and most devices one `audio_output` action
/// may step through.
pub const MAX_AUDIO_LIST: usize = 16;
/// Longest name the user may give a device or pin an app by.
pub const MAX_AUDIO_NAME_CHARS: usize = 64;

/// Output devices as tiles: a tap sends the sound there, the slider sets
/// the volume of the device in use.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AudioOutputProps {
    /// The volume slider of the device in use, right of the tiles.
    pub show_volume: bool,
}

impl Default for AudioOutputProps {
    fn default() -> Self {
        Self { show_volume: true }
    }
}

/// One slider per app that plays sound; a tap on its letter mutes it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MixerProps {
    /// App names that come first whenever they play, in this order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pinned: Vec<String>,
}

/// The microphone: which input is in use, its live level, mute for all
/// apps and the input volume.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MicrophoneProps {
    /// The live level meter. It records from the microphone while it shows.
    pub show_level: bool,
    /// The input volume slider.
    pub show_volume: bool,
}

impl Default for MicrophoneProps {
    fn default() -> Self {
        Self {
            show_level: true,
            show_volume: true,
        }
    }
}

/// How the sound widgets name, order and hide devices.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AudioSettings {
    /// Devices in the order the widgets show them; devices not listed
    /// follow in PipeWire's order of preference.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<AudioDeviceSettings>,
}

impl AudioSettings {
    pub fn device(&self, node: &str) -> Option<&AudioDeviceSettings> {
        self.devices.iter().find(|device| device.node == node)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AudioDeviceSettings {
    /// PipeWire's `node.name`, which stays the same across restarts.
    pub node: String,
    /// Shown instead of the name Linexus derives from the device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Hidden from the widgets. Without a choice, virtual devices (loopbacks,
    /// null sinks) are hidden and real ones shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}
