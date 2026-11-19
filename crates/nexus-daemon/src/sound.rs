//! The sound widgets and the privacy pill, seen from the engine: what
//! they show, and what their touches and the `audio_output` action change.
//!
//! Devices are named, hidden and ordered by `settings.audio`; the device
//! in use always shows unless it is hidden by name. Apps show while one of
//! their streams plays and stay five seconds after the last one stopped,
//! so a pause between tracks does not make them jump.

use nexus_config::{ActionSpec, AudioSettings, Profile, Widget, WidgetKind};
use nexus_render::{
    AudioApp, AudioDevice, AudioInfo, AudioKind, PrivacyEntry, PrivacyHit, PrivacyInfo, Slider,
    SoundHit,
};
use nexus_sensors::audio::{AudioCommand, AudioSnapshot, Device, DeviceKind, PrivacyUse};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long an app stays after its sound stopped.
pub(crate) const APP_LINGER: Duration = Duration::from_secs(5);
/// The open privacy list closes by itself after this long.
pub(crate) const PRIVACY_LIST: Duration = Duration::from_secs(20);
/// The pill names the apps this long after they changed, then shrinks to
/// its icons.
pub(crate) const PILL_NAMES: Duration = Duration::from_secs(5);
/// A slider let go shows its value this long at most while PipeWire has
/// not reported the new volume.
pub(crate) const SLIDER_HOLD: Duration = Duration::from_millis(1_500);

/// A slider under the finger.
#[derive(Debug, Clone)]
struct Grab {
    widget: String,
    hit: SoundHit,
    slider: Slider,
    /// The device or app it moves, as the renderer knows it.
    key: String,
}

/// A slider let go, until PipeWire reports the volume it set.
#[derive(Debug, Clone)]
struct Held {
    key: String,
    fraction: f32,
    /// The volume shown when it was let go.
    before: Option<u8>,
    until: Instant,
}

#[derive(Debug, Clone)]
struct Lingering {
    app: AudioApp,
    /// Last time one of its streams played.
    seen: Instant,
}

#[derive(Debug, Default)]
pub(crate) struct SoundUi {
    apps: Vec<Lingering>,
    /// The page of tiles each widget shows, by page id and widget id.
    pages: HashMap<(String, String), usize>,
    /// The page the last frame showed.
    page: String,
    grab: Option<Grab>,
    held: Option<Held>,
    /// The sound widget a touch went down on, and whether its drag turns
    /// the widget's pages (decided by the drag's first move).
    touched: Option<(Widget, Option<bool>)>,
    /// What the last frame showed, for hit tests.
    shown: Option<AudioInfo>,
    privacy_shown: Option<(PrivacyInfo, bool)>,
    privacy_open: Option<Instant>,
    /// The apps the pill names (and whether by camera), and since when.
    privacy_apps: Option<(Vec<(String, bool)>, Instant)>,
    /// The touch in progress belongs to the pill or the list.
    privacy_touch: Option<PrivacyHit>,
}

pub(crate) fn is_sound(widget: &Widget) -> bool {
    matches!(
        widget.kind,
        WidgetKind::AudioOutput(_) | WidgetKind::Mixer(_) | WidgetKind::Microphone(_)
    )
}

/// The profile shows sound widgets or switches outputs somewhere.
pub(crate) fn profile_uses_sound(profile: &Profile) -> bool {
    fn switches(action: &ActionSpec) -> bool {
        match action {
            ActionSpec::AudioOutput { .. } => true,
            ActionSpec::Sequence { steps, .. } => steps.iter().any(switches),
            _ => false,
        }
    }
    profile
        .pages
        .iter()
        .flat_map(|page| &page.widgets)
        .any(|widget| {
            is_sound(widget)
                || [&widget.tap, &widget.double_tap, &widget.hold]
                    .into_iter()
                    .flatten()
                    .any(switches)
        })
}

/// The page shows a microphone's level meter.
pub(crate) fn shows_meter(page: &nexus_config::Page) -> bool {
    page.widgets
        .iter()
        .any(|w| matches!(&w.kind, WidgetKind::Microphone(p) if p.show_level))
}

/// The microphone a level meter records: the one in use, while the
/// widget shows it.
pub(crate) fn meter_source(snapshot: &AudioSnapshot, settings: &AudioSettings) -> Option<String> {
    let node = snapshot.default_input.as_deref()?;
    let input = shown_devices(&snapshot.inputs, settings, Some(node))
        .into_iter()
        .find(|device| device.node == node)?;
    // A muted microphone is not recorded: nothing to see.
    (!input.muted).then(|| input.node.clone())
}

fn kind(kind: DeviceKind) -> AudioKind {
    match kind {
        DeviceKind::Headset => AudioKind::Headset,
        DeviceKind::Monitor => AudioKind::Monitor,
        DeviceKind::Speaker => AudioKind::Speaker,
        DeviceKind::Microphone => AudioKind::Microphone,
    }
}

/// Shown devices with the names and order from the settings. Virtual
/// devices hide unless shown by name or in use (`default`); a device
/// hidden by name hides even in use.
pub(crate) fn shown_devices<'a>(
    devices: &'a [Device],
    settings: &AudioSettings,
    default: Option<&str>,
) -> Vec<&'a Device> {
    let mut shown: Vec<(usize, usize, &Device)> = devices
        .iter()
        .enumerate()
        .filter(|(_, device)| {
            let choice = settings.device(&device.node).and_then(|d| d.hidden);
            let in_use = default == Some(device.node.as_str());
            !choice.unwrap_or(device.virtual_device && !in_use)
        })
        .map(|(order, device)| {
            let listed = settings
                .devices
                .iter()
                .position(|d| d.node == device.node)
                .unwrap_or(usize::MAX);
            (listed, order, device)
        })
        .collect();
    shown.sort_by_key(|(listed, order, _)| (*listed, *order));
    shown.into_iter().map(|(_, _, device)| device).collect()
}

fn display_name(device: &Device, settings: &AudioSettings) -> String {
    settings
        .device(&device.node)
        .and_then(|d| d.name.clone())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| device.name.clone())
}

fn device(device: &Device, settings: &AudioSettings) -> AudioDevice {
    AudioDevice {
        node: device.node.clone(),
        name: display_name(device, settings),
        detail: device.detail.clone(),
        kind: kind(device.kind),
        volume: device.volume,
        muted: device.muted,
    }
}

/// Streams by app, and whether any of them plays. Volume and mute are
/// those of the playing streams (of all while none plays): the loudest,
/// and muted when every one is.
pub(crate) fn group_apps(snapshot: &AudioSnapshot) -> Vec<(AudioApp, bool)> {
    let mut names: Vec<&str> = Vec::new();
    for stream in &snapshot.playback {
        if !names.contains(&stream.app.as_str()) {
            names.push(&stream.app);
        }
    }
    names
        .into_iter()
        .map(|name| {
            let streams: Vec<_> = snapshot.playback.iter().filter(|s| s.app == name).collect();
            let active = streams.iter().any(|s| s.active);
            let counted = || streams.iter().filter(move |s| s.active || !active);
            let app = AudioApp {
                name: name.to_string(),
                volume: counted().map(|s| s.volume).max().unwrap_or_default(),
                muted: counted().all(|s| s.muted),
            };
            (app, active)
        })
        .collect()
}

impl SoundUi {
    /// Folds in which apps play now and returns those to show.
    pub(crate) fn update_apps(
        &mut self,
        playing: Vec<(AudioApp, bool)>,
        now: Instant,
    ) -> Vec<AudioApp> {
        for (app, active) in &playing {
            match self.apps.iter_mut().find(|l| l.app.name == app.name) {
                Some(lingering) => {
                    lingering.app = app.clone();
                    if *active {
                        lingering.seen = now;
                    }
                }
                None if *active => self.apps.push(Lingering {
                    app: app.clone(),
                    seen: now,
                }),
                None => {}
            }
        }
        self.apps
            .retain(|l| now.saturating_duration_since(l.seen) < APP_LINGER);
        self.apps.iter().map(|l| l.app.clone()).collect()
    }

    /// What the sound widgets on page `page` show now.
    pub(crate) fn info(
        &mut self,
        snapshot: Option<&AudioSnapshot>,
        settings: &AudioSettings,
        page: &str,
        level: Option<u8>,
        now: Instant,
    ) -> Option<AudioInfo> {
        if self.page != page {
            self.page = page.to_string();
        }
        let Some(snapshot) = snapshot else {
            self.shown = None;
            self.held = None;
            return None;
        };
        let apps = self.update_apps(group_apps(snapshot), now);
        let shown = |devices: &[Device], default: &Option<String>| -> Vec<AudioDevice> {
            shown_devices(devices, settings, default.as_deref())
                .into_iter()
                .map(|d| device(d, settings))
                .collect()
        };
        let mut info = AudioInfo {
            outputs: shown(&snapshot.outputs, &snapshot.default_output),
            inputs: shown(&snapshot.inputs, &snapshot.default_input),
            default_output: snapshot.default_output.clone(),
            default_input: snapshot.default_input.clone(),
            apps,
            level,
            pages: self
                .pages
                .iter()
                .filter(|((on, _), _)| on == page)
                .map(|((_, widget), at)| (widget.clone(), *at))
                .collect(),
            grabbed: self.grab.as_ref().map(|grab| grab.key.clone()),
            held: None,
        };
        // A slider let go keeps its value until PipeWire reports a new
        // volume, or a little while.
        if let Some(held) = &self.held {
            if now >= held.until || volume_of(&info, &held.key) != held.before {
                self.held = None;
            }
        }
        info.held = self
            .held
            .as_ref()
            .map(|held| (held.key.clone(), held.fraction));
        self.shown = Some(info.clone());
        Some(info)
    }

    /// How long until the strip changes by itself: the pill shrinking, a
    /// let-go slider showing PipeWire's value.
    pub(crate) fn next_change(&self, now: Instant) -> Option<Duration> {
        let shrink = self
            .privacy_shown
            .as_ref()
            .filter(|(info, _)| !info.compact && !info.open)
            .and(self.privacy_apps.as_ref())
            .map(|(_, since)| *since + PILL_NAMES);
        let held = self.held.as_ref().map(|held| held.until);
        [shrink, held]
            .into_iter()
            .flatten()
            .min()
            .map(|at| at.saturating_duration_since(now))
    }

    /// The privacy pill's state, naming microphones as the widgets do.
    pub(crate) fn privacy(
        &mut self,
        uses: Vec<PrivacyUse>,
        snapshot: Option<&AudioSnapshot>,
        settings: &AudioSettings,
        mic_badge: bool,
        now: Instant,
    ) -> Option<PrivacyInfo> {
        if uses.is_empty()
            || self
                .privacy_open
                .is_some_and(|at| now.saturating_duration_since(at) >= PRIVACY_LIST)
        {
            self.privacy_open = None;
        }
        if uses.is_empty() {
            self.privacy_shown = None;
            self.privacy_apps = None;
            return None;
        }
        // The pill names the apps for a while after they change.
        let mut apps: Vec<(String, bool)> = uses
            .iter()
            .map(|found| (found.app.clone(), found.camera))
            .collect();
        apps.sort();
        apps.dedup();
        let since = match self.privacy_apps.take() {
            Some((before, since)) if before == apps => since,
            _ => now,
        };
        self.privacy_apps = Some((apps, since));
        let uses = uses
            .into_iter()
            .map(|found| {
                let device = match snapshot.and_then(|s| s.input(&found.device)) {
                    Some(input) if !found.camera => display_name(input, settings),
                    _ => found.device,
                };
                PrivacyEntry {
                    app: found.app,
                    camera: found.camera,
                    device,
                }
            })
            .collect();
        let info = PrivacyInfo {
            uses,
            open: self.privacy_open.is_some(),
            compact: now.saturating_duration_since(since) >= PILL_NAMES,
        };
        self.privacy_shown = Some((info.clone(), mic_badge));
        Some(info)
    }

    /// Without the pill, nothing of it stays.
    pub(crate) fn hide_privacy(&mut self) {
        self.privacy_shown = None;
        self.privacy_open = None;
        self.privacy_apps = None;
    }

    /// A touch going down: true when the pill or its list takes it.
    pub(crate) fn privacy_began(&mut self, palette: &nexus_render::Palette, x: u16) -> bool {
        self.privacy_touch = self
            .privacy_shown
            .as_ref()
            .and_then(|(info, badge)| nexus_render::privacy_hit(info, *badge, palette, x));
        self.privacy_touch.is_some()
    }

    /// The touch the pill took lifts: true when the strip changes.
    pub(crate) fn privacy_ended(&mut self, now: Instant) -> bool {
        match self.privacy_touch.take() {
            Some(PrivacyHit::Pill) => {
                self.privacy_open = Some(now);
                true
            }
            Some(PrivacyHit::Close) => {
                self.privacy_open = None;
                true
            }
            _ => false,
        }
    }

    pub(crate) fn privacy_touching(&self) -> bool {
        self.privacy_touch.is_some()
    }

    /// A touch going down on a sound widget; for a slider its X range.
    pub(crate) fn began(&mut self, widget: &Widget, x: u16) -> Option<(u16, u16)> {
        self.grab = None;
        self.touched = Some((widget.clone(), None));
        let info = self.shown.as_ref()?;
        let (hit, slider) = nexus_render::sound_hit(widget, info, x)?;
        let slider = slider?;
        let key = match &hit {
            SoundHit::OutputVolume => info.default_output.clone()?,
            SoundHit::InputVolume => info.default_input.clone()?,
            SoundHit::AppVolume(name) => name.clone(),
            _ => return None,
        };
        self.grab = Some(Grab {
            widget: widget.id.clone(),
            hit,
            slider,
            key,
        });
        Some(slider.span())
    }

    /// Forgets the touch in progress.
    pub(crate) fn reset_touch(&mut self) {
        self.grab = None;
        self.touched = None;
        self.privacy_touch = None;
    }

    /// The grabbed slider's value for a finger at `x`.
    pub(crate) fn slide(&self, widget: &Widget, x: u16) -> Option<f32> {
        let grab = self.grab.as_ref().filter(|g| g.widget == widget.id)?;
        Some(grab.slider.fraction_at(f32::from(x)))
    }

    /// The slider is let go at `fraction`: what to change.
    pub(crate) fn slide_done(
        &mut self,
        snapshot: Option<&AudioSnapshot>,
        fraction: f32,
        now: Instant,
    ) -> Vec<AudioCommand> {
        let Some(grab) = self.grab.take() else {
            return Vec::new();
        };
        let Some(snapshot) = snapshot else {
            return Vec::new();
        };
        self.held = Some(Held {
            key: grab.key.clone(),
            fraction,
            before: self
                .shown
                .as_ref()
                .and_then(|info| volume_of(info, &grab.key)),
            until: now + SLIDER_HOLD,
        });
        let percent = (fraction * 100.0).round().clamp(0.0, 100.0) as u8;
        let device = |devices: &[Device]| {
            devices
                .iter()
                .find(|d| d.node == grab.key)
                .map(|d| vec![AudioCommand::SetVolume { id: d.id, percent }])
                .unwrap_or_default()
        };
        match &grab.hit {
            SoundHit::OutputVolume => device(&snapshot.outputs),
            SoundHit::InputVolume => device(&snapshot.inputs),
            SoundHit::AppVolume(name) => snapshot
                .playback
                .iter()
                .filter(|s| s.app == *name)
                .map(|s| AudioCommand::SetVolume { id: s.id, percent })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// A tap on a sound widget at `x`: what to change.
    pub(crate) fn tap(
        &self,
        widget: &Widget,
        x: u16,
        snapshot: Option<&AudioSnapshot>,
        settings: &AudioSettings,
    ) -> Vec<AudioCommand> {
        let (Some(info), Some(snapshot)) = (self.shown.as_ref(), snapshot) else {
            return Vec::new();
        };
        let Some((hit, _)) = nexus_render::sound_hit(widget, info, x) else {
            return Vec::new();
        };
        match hit {
            SoundHit::Output(node) => select_output(snapshot, &node),
            SoundHit::App(name) => {
                let muted = info.apps.iter().any(|app| app.name == name && app.muted);
                snapshot
                    .playback
                    .iter()
                    .filter(|s| s.app == name)
                    .map(|s| AudioCommand::SetMute {
                        id: s.id,
                        muted: Some(!muted),
                    })
                    .collect()
            }
            SoundHit::NextInput => {
                let default = snapshot.default_input.as_deref();
                let inputs = shown_devices(&snapshot.inputs, settings, default);
                let at = inputs
                    .iter()
                    .position(|d| Some(&d.node) == snapshot.default_input.as_ref());
                let next = match at {
                    Some(at) => inputs.get((at + 1) % inputs.len()),
                    None => inputs.first(),
                };
                next.filter(|d| Some(&d.node) != snapshot.default_input.as_ref())
                    .map(|d| vec![AudioCommand::SetDefault { id: d.id }])
                    .unwrap_or_default()
            }
            SoundHit::InputMute => snapshot
                .default_input
                .as_deref()
                .and_then(|node| snapshot.input(node))
                .map(|d| {
                    vec![AudioCommand::SetMute {
                        id: d.id,
                        muted: None,
                    }]
                })
                .unwrap_or_default(),
            // A tap on a slider sets it where the finger was; the touch
            // machine reports that as a slide.
            SoundHit::OutputVolume | SoundHit::AppVolume(_) | SoundHit::InputVolume => Vec::new(),
        }
    }

    /// The finger drags: true when the widget under it turns its pages
    /// instead of the strip.
    pub(crate) fn drag_pages(&mut self, dx: i32) -> bool {
        let Some((widget, decided)) = self.touched.as_mut() else {
            return false;
        };
        if let Some(pages) = decided {
            return *pages;
        }
        let Some(info) = self.shown.as_ref() else {
            *decided = Some(false);
            return false;
        };
        let (pages, page) = nexus_render::sound_pages(widget, info);
        let turns = if dx > 0 { page + 1 < pages } else { page > 0 };
        *decided = Some(turns);
        turns
    }

    /// A drag ended; true when it turned the widget's pages.
    pub(crate) fn drag_end(&mut self, forward: Option<bool>) -> bool {
        let Some((widget, Some(true))) = self.touched.take() else {
            return false;
        };
        let (Some(forward), Some(info)) = (forward, self.shown.as_ref()) else {
            return true;
        };
        let (pages, page) = nexus_render::sound_pages(&widget, info);
        let next = if forward {
            (page + 1).min(pages - 1)
        } else {
            page.saturating_sub(1)
        };
        self.pages
            .insert((self.page.clone(), widget.id.clone()), next);
        true
    }
}

/// The volume `info` shows for a device or an app.
fn volume_of(info: &AudioInfo, key: &str) -> Option<u8> {
    info.outputs
        .iter()
        .chain(&info.inputs)
        .find(|d| d.node == key)
        .map(|d| d.volume)
        .or_else(|| info.apps.iter().find(|a| a.name == key).map(|a| a.volume))
}

/// Makes `node` the output in use. WirePlumber moves the streams that
/// follow the default by itself; streams sent to the output before by
/// name are sent to the new one. Streams sent to other outputs stay.
pub(crate) fn select_output(snapshot: &AudioSnapshot, node: &str) -> Vec<AudioCommand> {
    let Some(target) = snapshot.output(node) else {
        return Vec::new();
    };
    let previous = snapshot
        .default_output
        .as_deref()
        .and_then(|node| snapshot.output(node));
    if previous.is_some_and(|p| p.id == target.id) {
        return Vec::new();
    }
    let mut commands = vec![AudioCommand::SetDefault { id: target.id }];
    if let Some(previous) = previous {
        commands.extend(
            snapshot
                .playback
                .iter()
                .filter(|s| s.target == Some(previous.id))
                .map(|s| AudioCommand::MoveStream {
                    stream: s.id,
                    serial: target.serial,
                }),
        );
    }
    commands
}

/// The `audio_output` action: the named device, or the next of several
/// (of all shown ones when none are named).
pub(crate) fn output_action(
    devices: &[String],
    snapshot: &AudioSnapshot,
    settings: &AudioSettings,
) -> Result<Vec<AudioCommand>, String> {
    let present: Vec<&str> = if devices.is_empty() {
        shown_devices(
            &snapshot.outputs,
            settings,
            snapshot.default_output.as_deref(),
        )
        .into_iter()
        .map(|d| d.node.as_str())
        .collect()
    } else {
        devices
            .iter()
            .map(String::as_str)
            .filter(|node| snapshot.output(node).is_some())
            .collect()
    };
    if present.is_empty() {
        return Err(match devices {
            [one] => format!("{one} is not connected"),
            _ => "None of the outputs is connected".into(),
        });
    }
    let current = snapshot.default_output.as_deref();
    let next = match present.iter().position(|node| Some(*node) == current) {
        Some(at) => present[(at + 1) % present.len()],
        None => present[0],
    };
    Ok(select_output(snapshot, next))
}

#[cfg(test)]
#[path = "sound_tests.rs"]
pub(crate) mod tests;
