//! The sound widgets and the privacy pill through the engine: touches in,
//! PipeWire commands out.

use super::*;
use nexus_config::{
    AudioOutputProps, ButtonProps, MicrophoneProps, MixerProps, Page, PageTemplate,
};
use nexus_ipc::TouchPhase;
use nexus_sensors::audio::{AudioCommand, AudioSnapshot, Stream};
use std::sync::{mpsc, Arc, Mutex};

type Sent = Arc<Mutex<Vec<AudioCommand>>>;

fn page(id: &str, widgets: Vec<Widget>) -> Page {
    Page {
        id: id.into(),
        name: id.into(),
        hidden: false,
        auto_return_seconds: None,
        background: None,
        widgets,
    }
}

fn engine(pages: Vec<Page>, snapshot: AudioSnapshot) -> (Engine, Sent) {
    let mut config = nexus_config::default_config();
    config.profiles[0].pages = pages;
    let dir = std::env::temp_dir().join(format!("nexus-daemon-sound-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let logger = nexus_events::EventLogger::with_path("test", dir.join("events.jsonl"));
    let (feedback_tx, feedback) = mpsc::channel();
    let (audio, sent) = crate::audio::Audio::fake(snapshot);
    let mut engine = Engine::new(EngineParts {
        runtime: Runtime::from_config(config, None),
        metrics: MetricsHandle::default(),
        providers: ProviderHandle::default(),
        dispatcher: Dispatcher::new(true, false, logger.clone(), feedback_tx),
        feedback,
        notifications: crate::notifications::Notifications::off(),
        obs: crate::obs::Obs::off(),
        discord: crate::discord::Discord::off(),
        ping: crate::ping::Ping::off(),
        audio,
        focus: crate::focus::Focus::off(),
        logger,
        state_path: None,
        mode: nexus_ipc::DaemonMode::Emulator,
    });
    // As main sets it for the emulator: a strip that shows.
    engine.device = nexus_ipc::DeviceStatus::emulator();
    (engine, sent)
}

fn tap(engine: &mut Engine, x: u16, at: Instant) {
    engine.touch_event(TouchPhase::Began, x, at);
    engine.touch_event(TouchPhase::Ended, x, at + Duration::from_millis(60));
}

fn taken(sent: &Sent) -> Vec<AudioCommand> {
    std::mem::take(&mut *sent.lock().unwrap())
}

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

#[test]
fn output_tiles_and_slider_send_commands() {
    let outputs = Widget::new(
        "out",
        0,
        640,
        WidgetKind::AudioOutput(AudioOutputProps::default()),
    );
    let (mut engine, sent) = engine(vec![page("p", vec![outputs])], crate::sound::tests::desk());
    let t = Instant::now();
    engine.frame(t);
    tap(&mut engine, 260, t);
    assert_eq!(
        taken(&sent),
        [
            AudioCommand::SetDefault { id: 62 },
            AudioCommand::MoveStream {
                stream: 201,
                serial: Some(1062)
            },
        ]
    );
    // The slider follows the finger and sets the volume on release.
    let t = t + ms(1_000);
    engine.touch_event(TouchPhase::Began, 450, t);
    engine.touch_event(TouchPhase::Moved, 520, t + ms(30));
    assert!(engine.scene.sliding());
    engine.frame(t + ms(40));
    assert!(taken(&sent).is_empty());
    engine.touch_event(TouchPhase::Ended, 629, t + ms(80));
    assert_eq!(
        taken(&sent),
        [AudioCommand::SetVolume {
            id: 91,
            percent: 100
        }]
    );
    assert!(!engine.scene.sliding());
}

#[test]
fn mixer_mutes_and_swipes_turn_its_pages() {
    let mut snapshot = crate::sound::tests::desk();
    for (n, name) in ["Steam", "Valheim"].iter().enumerate() {
        snapshot.playback.push(Stream {
            id: 300 + n as u32,
            app: name.to_string(),
            volume: 50,
            active: true,
            ..Stream::default()
        });
    }
    let mixer = Widget::new("mix", 0, 640, WidgetKind::Mixer(MixerProps::default()));
    let other = Widget::new("b", 0, 640, WidgetKind::Button(ButtonProps::default()));
    let (mut engine, sent) = engine(
        vec![page("p", vec![mixer]), page("q", vec![other])],
        snapshot,
    );
    let t = Instant::now();
    engine.frame(t);
    tap(&mut engine, 170, t);
    assert_eq!(
        taken(&sent),
        [AudioCommand::SetMute {
            id: 202,
            muted: Some(true)
        }]
    );
    // Five apps, four a page: a swipe turns the mixer's page, not the strip's.
    let t = t + ms(1_000);
    engine.touch_event(TouchPhase::Began, 400, t);
    for (n, x) in [370, 300, 200].iter().enumerate() {
        engine.touch_event(TouchPhase::Moved, *x, t + ms(20 * (n as u64 + 1)));
    }
    engine.touch_event(TouchPhase::Ended, 200, t + ms(100));
    assert_eq!(engine.runtime.page().id, "p");
    engine.frame(t + ms(120));
    tap(&mut engine, 20, t + ms(500));
    assert_eq!(
        taken(&sent),
        [AudioCommand::SetMute {
            id: 301,
            muted: Some(true)
        }]
    );
    // On the last page the same swipe changes the strip's page.
    let t = t + ms(2_000);
    engine.touch_event(TouchPhase::Began, 400, t);
    for (n, x) in [370, 300, 200].iter().enumerate() {
        engine.touch_event(TouchPhase::Moved, *x, t + ms(20 * (n as u64 + 1)));
    }
    engine.touch_event(TouchPhase::Ended, 200, t + ms(100));
    assert_eq!(engine.runtime.page().id, "q");
}

#[test]
fn the_output_button_and_the_microphone() {
    let switch = Widget::new("switch", 0, 100, WidgetKind::Button(ButtonProps::default()))
        .with_tap(ActionSpec::AudioOutput {
            devices: vec!["game".into(), "chat".into()],
        });
    let mic = Widget::new(
        "mic",
        100,
        540,
        WidgetKind::Microphone(MicrophoneProps::default()),
    );
    let (mut engine, sent) = engine(
        vec![page("p", vec![switch, mic])],
        crate::sound::tests::desk(),
    );
    let t = Instant::now();
    engine.tick(t);
    engine.frame(t);
    // The meter records the microphone in use while it shows.
    assert_eq!(engine.audio.wanted(), (true, true, Some("mic".into())));
    tap(&mut engine, 50, t);
    let commands = taken(&sent);
    assert_eq!(commands[0], AudioCommand::SetDefault { id: 68 });
    // Mute for all apps.
    tap(&mut engine, 100 + 12 + 125 + 11 + 5, t + ms(500));
    assert!(taken(&sent).is_empty(), "the meter is no button");
    let layout_mute = 100 + 12 + 125 + 11 + 170 + 11 + 30;
    tap(&mut engine, layout_mute, t + ms(1_000));
    assert_eq!(
        taken(&sent),
        [AudioCommand::SetMute {
            id: 84,
            muted: None
        }]
    );
    // While the level moves, frames come quickly.
    engine.audio.set_level(Some(60));
    assert!(engine.next_frame_in(t + ms(1_100)) <= Duration::from_millis(66));
    // A muted microphone is not recorded.
    let mut muted = crate::sound::tests::desk();
    muted.inputs[0].muted = true;
    engine.audio.set_snapshot(muted);
    engine.tick(t + ms(1_200));
    assert_eq!(engine.audio.wanted(), (true, true, None));
    assert!(engine.next_frame_in(t + ms(1_300)) > Duration::from_millis(66));

    // A blank strip records nothing and lets PipeWire be.
    engine.runtime.touch_wake_blank = true;
    engine.tick(t + ms(2_000));
    assert_eq!(engine.audio.wanted(), (false, false, None));
}

#[test]
fn the_meter_records_only_while_it_is_on_a_strip_that_shows() {
    let mic = Widget::new(
        "mic",
        0,
        640,
        WidgetKind::Microphone(MicrophoneProps::default()),
    );
    let other = Widget::new("b", 0, 640, WidgetKind::Button(ButtonProps::default()));
    let (mut engine, _) = engine(
        vec![page("p", vec![mic]), page("q", vec![other])],
        crate::sound::tests::desk(),
    );
    engine.mode = nexus_ipc::DaemonMode::Hardware;
    engine.device = nexus_ipc::DeviceStatus::connected("/dev/hidraw9");
    let t = Instant::now();
    let recording = (true, true, Some("mic".to_string()));
    let following = (true, true, None);
    let nothing = (false, false, None);
    engine.tick(t);
    assert_eq!(engine.audio.wanted(), recording);
    // Another page: PipeWire is still followed for the pill, the
    // microphone no longer recorded; back again, it is.
    let _ = engine.runtime.step(Direction::Forward);
    engine.tick(t + ms(100));
    assert_eq!(engine.audio.wanted(), following);
    let _ = engine.runtime.step(Direction::Backward);
    engine.tick(t + ms(200));
    assert_eq!(engine.audio.wanted(), recording);
    // Unplugged, the strip shows nothing: nothing is followed or recorded.
    engine.device = nexus_ipc::DeviceStatus::unavailable("no Nexus found");
    engine.tick(t + ms(300));
    assert_eq!(engine.audio.wanted(), nothing);
    engine.device = nexus_ipc::DeviceStatus::connected("/dev/hidraw9");
    engine.tick(t + ms(400));
    assert_eq!(engine.audio.wanted(), recording);
    // Off, likewise.
    engine.runtime.touch_wake_blank = true;
    engine.tick(t + ms(500));
    assert_eq!(engine.audio.wanted(), nothing);
}

#[test]
fn a_sound_slider_never_moves_the_system_volume() {
    let outputs = Widget::new(
        "out",
        0,
        640,
        WidgetKind::AudioOutput(AudioOutputProps::default()),
    );
    let (mut engine, sent) = engine(vec![page("p", vec![outputs])], crate::sound::tests::desk());
    let t = Instant::now();
    engine.frame(t);
    let quiet = engine.own_volume_until;
    // A slide the sound widget did not grab (its device went away under
    // the finger) ends without a command.
    let widget = WidgetRef {
        index: 0,
        generation: engine.runtime.generation,
    };
    for done in [false, true] {
        engine.interact(
            Interaction::Slide {
                widget,
                x: 600,
                done,
            },
            t + ms(10),
        );
    }
    assert_eq!(engine.own_volume_until, quiet);
    assert!(taken(&sent).is_empty());
    assert!(!engine.scene.sliding());
}

#[test]
fn following_pipewire_depends_on_the_profile_and_the_pill() {
    let overview = nexus_config::template_page(PageTemplate::Overview, "o");
    let (mut engine, _) = engine(vec![overview], crate::sound::tests::desk());
    let t = Instant::now();
    engine.tick(t);
    // The privacy pill alone needs PipeWire, but no meter.
    assert_eq!(engine.audio.wanted(), (true, true, None));
    let mut config = engine.runtime.config.clone();
    config.settings.overlays.privacy = false;
    engine.runtime = Runtime::from_config(config, None);
    engine.tick(t + ms(1_000));
    assert_eq!(engine.audio.wanted(), (false, false, None));
}

#[test]
fn the_privacy_pill_takes_its_touches() {
    let mut snapshot = crate::sound::tests::desk();
    snapshot.recording.push(Stream {
        id: 400,
        app: "Discord".into(),
        active: true,
        peers: vec![84],
        ..Stream::default()
    });
    let outputs = Widget::new(
        "out",
        0,
        640,
        WidgetKind::AudioOutput(AudioOutputProps::default()),
    );
    let (mut engine, sent) = engine(vec![page("p", vec![outputs])], snapshot);
    let t = Instant::now();
    engine.frame(t);
    // Under the pill sits the volume slider: the pill wins.
    tap(&mut engine, 620, t);
    assert!(taken(&sent).is_empty());
    engine.frame(t + ms(100));
    // The open list takes every touch, until its button closes it.
    tap(&mut engine, 20, t + ms(500));
    assert!(taken(&sent).is_empty());
    tap(&mut engine, 600, t + ms(1_000));
    engine.frame(t + ms(1_100));
    tap(&mut engine, 140, t + ms(1_500));
    assert_eq!(
        taken(&sent).first(),
        Some(&AudioCommand::SetDefault { id: 68 })
    );

    // The named pill covers the slider's right end: a tap there opens the
    // list, which its button closes.
    engine.frame(t + ms(2_000));
    tap(&mut engine, 560, t + ms(2_000));
    assert!(taken(&sent).is_empty());
    engine.frame(t + ms(2_100));
    tap(&mut engine, 600, t + ms(2_500));
    engine.frame(t + ms(2_600));
    // Five seconds after Discord began it shrinks to its icon, and the
    // slider is free again.
    let shrunk = t + crate::sound::PILL_NAMES;
    assert!(engine.next_frame_in(t + ms(4_900)) <= ms(100));
    engine.frame(shrunk);
    tap(&mut engine, 560, shrunk);
    assert!(matches!(
        taken(&sent)[..],
        [AudioCommand::SetVolume { id: 91, .. }]
    ));
    // The icon still opens the list, which takes every touch.
    tap(&mut engine, 615, shrunk + ms(500));
    engine.frame(shrunk + ms(600));
    tap(&mut engine, 140, shrunk + ms(1_000));
    assert!(taken(&sent).is_empty());
}
