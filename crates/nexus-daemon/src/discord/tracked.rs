//! What the worker follows while signed in, and what each RPC event changes.

use super::images::Images;
use super::{Wanted, PRIVATE};
use nexus_render::{DiscordInfo, DiscordLink, DiscordMember, DiscordVoiceChannel};
use nexus_sensors::{DiscordEvent, DiscordVoiceMember};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Instant;

/// One followed voice channel.
#[derive(Debug, Clone, Default)]
pub(super) struct Channel {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) members: Vec<DiscordVoiceMember>,
}

/// Everything the worker follows while signed in.
#[derive(Debug, Default)]
pub(super) struct Tracked {
    pub(super) channel: Option<String>,
    pub(super) guild: Option<String>,
    pub(super) self_muted: bool,
    pub(super) self_deafened: bool,
    /// Voice channels per followed server.
    pub(super) guilds: HashMap<String, Vec<Channel>>,
    /// When each server's channel list was read.
    pub(super) listed: HashMap<String, Instant>,
    /// Channels whose events are subscribed.
    pub(super) subscribed: HashSet<String>,
    pub(super) speaking: HashSet<String>,
    pub(super) input_volume: u8,
    pub(super) output_volume: u16,
    pub(super) push_to_talk: bool,
    pub(super) noise_suppression: bool,
    pub(super) echo_cancellation: bool,
    pub(super) automatic_gain: bool,
    pub(super) link: Option<DiscordLink>,
    /// You, as Discord knows you.
    pub(super) self_id: Option<String>,
}

impl Tracked {
    pub(super) fn info(&self, images: &Images) -> DiscordInfo {
        let guilds = self
            .guilds
            .iter()
            .map(|(guild, channels)| {
                let channels = channels
                    .iter()
                    .map(|channel| DiscordVoiceChannel {
                        id: channel.id.clone(),
                        name: channel.name.clone(),
                        members: channel
                            .members
                            .iter()
                            .map(|member| DiscordMember {
                                user_id: member.user_id.clone(),
                                name: member.name.clone(),
                                avatar: images.get(&member.avatar_url),
                                speaking: self.speaking.contains(&member.user_id),
                                muted: member.muted,
                                deafened: member.deafened,
                                volume: member.volume,
                                local_muted: member.local_muted,
                            })
                            .collect(),
                    })
                    .collect();
                (guild.clone(), channels)
            })
            .collect();
        DiscordInfo {
            channel: self.channel.clone(),
            guild: self.guild.clone(),
            self_muted: self.self_muted,
            self_deafened: self.self_deafened,
            guilds,
            input_volume: self.input_volume,
            output_volume: self.output_volume,
            push_to_talk: self.push_to_talk,
            noise_suppression: self.noise_suppression,
            echo_cancellation: self.echo_cancellation,
            automatic_gain: self.automatic_gain,
            link: self.link.filter(|_| self.channel.is_some()),
            focus: None,
            mic_badge: false,
            rotation: 0,
            self_id: self.self_id.clone(),
        }
    }

    /// Reads voice settings: a GET_VOICE_SETTINGS or SET_VOICE_SETTINGS
    /// answer, or a VOICE_SETTINGS_UPDATE event. Missing parts stay.
    pub(super) fn apply_settings(&mut self, data: &Value) {
        if let Some(mute) = data["mute"].as_bool() {
            self.self_muted = mute;
        }
        if let Some(deaf) = data["deaf"].as_bool() {
            self.self_deafened = deaf;
        }
        if let Some(volume) = data["input"]["volume"].as_f64() {
            self.input_volume = volume.round().clamp(0.0, 100.0) as u8;
        }
        if let Some(volume) = data["output"]["volume"].as_f64() {
            self.output_volume = volume.round().clamp(0.0, 200.0) as u16;
        }
        if let Some(mode) = data["mode"]["type"].as_str() {
            self.push_to_talk = mode == "PUSH_TO_TALK";
        }
        for (key, value) in [
            ("noise_suppression", &mut self.noise_suppression),
            ("echo_cancellation", &mut self.echo_cancellation),
            ("automatic_gain_control", &mut self.automatic_gain),
        ] {
            if let Some(on) = data[key].as_bool() {
                *value = on;
            }
        }
    }

    /// Someone's volume or mute for you changed (a SET_USER_VOICE_SETTINGS
    /// answer).
    pub(super) fn apply_user(&mut self, data: &Value) {
        let Some(user) = data["user_id"].as_str() else {
            return;
        };
        for member in self
            .guilds
            .values_mut()
            .flatten()
            .flat_map(|channel| channel.members.iter_mut())
            .filter(|member| member.user_id == user)
        {
            if let Some(volume) = data["volume"].as_f64() {
                member.volume = volume.round().clamp(0.0, 200.0) as u16;
            }
            if let Some(mute) = data["mute"].as_bool() {
                member.local_muted = mute;
            }
        }
    }

    /// Servers whose channels are followed for `wanted`.
    pub(super) fn guilds_for(&self, wanted: &Wanted) -> BTreeSet<String> {
        let mut guilds = wanted.guilds.clone();
        if wanted.current_guild {
            match &self.guild {
                Some(guild) => {
                    guilds.insert(guild.clone());
                }
                // A call outside any server: a direct or group call.
                None if self.channel.is_some() => {
                    guilds.insert(PRIVATE.to_string());
                }
                None => {}
            }
        }
        guilds
    }

    pub(super) fn channel_mut(&mut self, id: &str) -> Option<&mut Channel> {
        self.guilds
            .values_mut()
            .flat_map(|channels| channels.iter_mut())
            .find(|channel| channel.id == id)
    }

    /// Follows one event. True when someone showed up in a channel the
    /// event does not name, so the channels should be read again.
    pub(super) fn apply(&mut self, event: &DiscordEvent) -> bool {
        let data = &event.data;
        match event.evt.as_str() {
            "VOICE_SETTINGS_UPDATE" => self.apply_settings(data),
            "VOICE_CHANNEL_SELECT" => {
                self.channel = data["channel_id"].as_str().map(str::to_string);
                self.guild = data["guild_id"].as_str().map(str::to_string);
                self.speaking.clear();
                if self.channel.is_none() {
                    self.link = None;
                }
            }
            "VOICE_CONNECTION_STATUS" => {
                let ping = data["last_ping"]
                    .as_f64()
                    .or_else(|| data["average_ping"].as_f64())
                    .filter(|ping| *ping > 0.0)
                    .map(|ping| ping.round() as u32);
                self.link = Some(DiscordLink {
                    connected: data["state"] == "VOICE_CONNECTED",
                    ping_ms: ping,
                });
            }
            "VOICE_STATE_CREATE" | "VOICE_STATE_UPDATE" | "VOICE_STATE_DELETE" => {
                // The channel comes with the subscription, not the event;
                // a member is found by id wherever they sit.
                let Some(member) = DiscordVoiceMember::from_voice_state(data) else {
                    return false;
                };
                let channel_id = data["channel_id"].as_str().map(str::to_string);
                if event.evt == "VOICE_STATE_DELETE" {
                    for channel in self.guilds.values_mut().flat_map(|c| c.iter_mut()) {
                        if channel_id.as_deref().is_none_or(|id| id == channel.id) {
                            channel.members.retain(|m| m.user_id != member.user_id);
                        }
                    }
                    self.speaking.remove(&member.user_id);
                    return false;
                }
                let mut updated = false;
                for channel in self.guilds.values_mut().flat_map(|c| c.iter_mut()) {
                    if let Some(existing) = channel
                        .members
                        .iter_mut()
                        .find(|m| m.user_id == member.user_id)
                    {
                        *existing = member.clone();
                        updated = true;
                    }
                }
                if !updated {
                    match channel_id.as_deref().and_then(|id| self.channel_mut(id)) {
                        Some(channel) => channel.members.push(member),
                        None => return true,
                    }
                }
            }
            "SPEAKING_START" => {
                if let Some(user) = data["user_id"].as_str() {
                    self.speaking.insert(user.to_string());
                }
            }
            "SPEAKING_STOP" => {
                if let Some(user) = data["user_id"].as_str() {
                    self.speaking.remove(user);
                }
            }
            _ => {}
        }
        false
    }
}
