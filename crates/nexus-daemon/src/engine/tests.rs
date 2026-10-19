use super::*;
use nexus_config::{ButtonProps, Page, TimerProps};
use nexus_ipc::{Command, TouchPhase};
use std::sync::mpsc;

/// A directory for one test's files, removed afterwards.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("nexus-daemon-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn engine(runtime: Runtime, scratch: &Scratch) -> Engine {
    let (feedback_tx, feedback) = mpsc::channel();
    let logger = nexus_events::EventLogger::with_path("test", scratch.0.join("events.jsonl"));
    Engine::new(EngineParts {
        runtime,
        metrics: MetricsHandle::default(),
        providers: ProviderHandle::default(),
        dispatcher: Dispatcher::new(true, false, logger.clone(), feedback_tx),
        feedback,
        notifications: None,
        focus: None,
        logger,
        state_path: None,
        mode: nexus_ipc::DaemonMode::Emulator,
    })
}

fn brightness(command: BrightnessAction, percent: Option<u8>) -> ActionSpec {
    ActionSpec::Brightness { command, percent }
}

fn set(percent: u8) -> ActionSpec {
    brightness(BrightnessAction::Set, Some(percent))
}

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

fn button(id: &str, tap: ActionSpec) -> Widget {
    Widget::new(id, 0, 200, WidgetKind::Button(ButtonProps::default())).with_tap(tap)
}

fn go_to(engine: &mut Engine, page_id: &str, now: Instant) {
    let page_id = page_id.into();
    let (response, _) = engine.handle_request(Command::GoToPage { page_id }, now);
    assert!(response.ok, "{}", response.message);
}

fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

/// D1: Studio deletes the page the strip showed and the strip follows to
/// another one; the reload lands while that page still slides out.
#[test]
fn reload_that_removes_the_page_moving_out_does_not_panic() {
    let scratch = Scratch::new("reload-transition");
    let path = scratch.0.join("config.json");
    let mut config = nexus_config::default_config();
    nexus_config::save_to_path(&path, &config, true).unwrap();
    let (runtime, _) = Runtime::load(Some(&path)).unwrap();
    let mut engine = engine(runtime, &scratch);
    let t = Instant::now();
    go_to(&mut engine, "focus", t);
    go_to(&mut engine, "overview", t + ms(300));
    config.profiles[0].pages.retain(|p| p.id != "focus");
    nexus_config::save_to_path(&path, &config, true).unwrap();
    let (response, _) = engine.handle_request(Command::ReloadConfig { path: None }, t + ms(350));
    assert!(response.ok, "{}", response.message);
    engine.frame(t + ms(400));
    assert_eq!(engine.runtime.page().id, "overview");
}

/// D2: a touch belongs to the page it started on.
#[test]
fn a_touch_does_not_follow_a_page_change() {
    let scratch = Scratch::new("touch-page");
    let mut config = nexus_config::default_config();
    let mut repeating = button("b", set(90));
    repeating.repeat = true;
    config.profiles[0].pages = vec![
        page("a", vec![button("a", set(10))]),
        page("b", vec![repeating]),
    ];
    let mut engine = engine(Runtime::from_config(config, None), &scratch);
    let start = engine.runtime.brightness;
    let t = Instant::now();
    // Finger down on page a, Studio moves the strip to page b, finger up.
    engine.touch_event(TouchPhase::Began, 50, t);
    go_to(&mut engine, "b", t + ms(20));
    engine.touch_event(TouchPhase::Ended, 50, t + ms(60));
    assert_eq!(engine.runtime.brightness, start);
    // A hold on page b that comes due after a change does not fire on a.
    engine.touch_event(TouchPhase::Began, 50, t + ms(1_000));
    go_to(&mut engine, "a", t + ms(1_020));
    engine.tick(t + ms(2_000));
    engine.touch_event(TouchPhase::Ended, 50, t + ms(2_100));
    assert_eq!(engine.runtime.brightness, start);
    // Interactions stamped with an older generation are ignored.
    let generation = engine.runtime.generation;
    let stale = WidgetRef {
        index: 0,
        generation: generation - 1,
    };
    engine.interact(
        Interaction::Tap {
            widget: Some(stale),
            x: 50,
        },
        t + ms(3_000),
    );
    assert_eq!(engine.runtime.brightness, start);
    // A touch on the page that shows still works.
    engine.touch_event(TouchPhase::Began, 50, t + ms(4_000));
    engine.touch_event(TouchPhase::Ended, 50, t + ms(4_060));
    assert_eq!(engine.runtime.brightness, 10);
}

/// D3: a release that never arrives must not keep a hold repeating, and the
/// next touch must not be swallowed.
#[test]
fn a_lost_release_stops_repeats_and_the_next_touch_counts() {
    let scratch = Scratch::new("lost-release");
    let mut config = nexus_config::default_config();
    let mut up = button("up", brightness(BrightnessAction::Up, None));
    up.repeat = true;
    config.profiles[0].brightness = 20;
    config.profiles[0].pages = vec![page("a", vec![up])];
    let mut engine = engine(Runtime::from_config(config, None), &scratch);
    let t = Instant::now();
    engine.touch_event(TouchPhase::Began, 50, t);
    engine.tick(t + ms(620));
    engine.tick(t + ms(800));
    assert_eq!(engine.runtime.brightness, 40);
    // The device goes away with the finger still down.
    engine.cancel_touch(t + ms(850));
    engine.tick(t + ms(1_000));
    engine.tick(t + ms(5_000));
    assert_eq!(engine.runtime.brightness, 40);

    // The strip is off; the waking touch loses its release.
    engine.runtime.touch_wake_blank = true;
    engine.touch_event(TouchPhase::Began, 50, t + ms(6_000));
    assert!(!engine.runtime.touch_wake_blank);
    engine.touch_event(TouchPhase::Began, 50, t + ms(7_000));
    engine.touch_event(TouchPhase::Ended, 50, t + ms(7_060));
    assert_eq!(engine.runtime.brightness, 50);
}

fn timer_running(engine: &Engine, profile: &str, page: &str, now: Instant) -> bool {
    let info = engine.timers.info(
        &timers::key(profile, page, "timer"),
        &TimerProps::default(),
        now,
    );
    info.phase == nexus_render::TimerPhase::Running
}

/// D4: timers keep their state per profile and page.
#[test]
fn timers_survive_profile_switches_and_do_not_share_ids() {
    let scratch = Scratch::new("timers");
    let mut config = nexus_config::default_config();
    let mut focus = config.profiles[0]
        .pages
        .iter()
        .find(|p| p.id == "focus")
        .unwrap()
        .clone();
    focus.id = "focus2".into();
    config.profiles[0].pages.push(focus);
    let mut second = config.profiles[0].clone();
    second.id = "second".into();
    // An app profile without any timer.
    let mut plain = second.clone();
    plain.id = "plain".into();
    plain.pages.retain(|p| p.id == "overview");
    config.profiles.extend([second, plain]);
    let mut engine = engine(Runtime::from_config(config, None), &scratch);
    let t = Instant::now();
    go_to(&mut engine, "focus", t);
    // A tap on the timer waits out the double-tap window, then starts it.
    engine.touch_event(TouchPhase::Began, 100, t + ms(100));
    engine.touch_event(TouchPhase::Ended, 100, t + ms(150));
    engine.tick(t + ms(500));
    assert!(timer_running(&engine, "default", "focus", t + ms(600)));
    assert!(!timer_running(&engine, "default", "focus2", t + ms(600)));
    assert!(!timer_running(&engine, "second", "focus", t + ms(600)));

    let switch = |profile_id: &str| Command::SwitchProfile {
        profile_id: profile_id.into(),
    };
    engine.handle_request(switch("plain"), t + ms(700));
    engine.tick(t + ms(800));
    engine.handle_request(switch("default"), t + ms(1_000));
    engine.tick(t + ms(1_100));
    assert!(timer_running(&engine, "default", "focus", t + ms(1_200)));
}

/// D8: sequence steps waiting for their delay do not outlive the config
/// they were started under. A profile switch drops only the page steps of
/// other sequences; a sequence that switches profiles keeps all its steps.
#[test]
fn reload_and_profile_switches_drop_waiting_sequence_steps() {
    let scratch = Scratch::new("sequence");
    let mut config = nexus_config::default_config();
    let mut second = config.profiles[0].clone();
    second.id = "second".into();
    second.brightness = 60;
    config.profiles.push(second);
    let mut engine = engine(Runtime::from_config(config, None), &scratch);
    let sequence = |steps, delay_ms| ActionSpec::Sequence { steps, delay_ms };
    let t = Instant::now();

    let switching = vec![
        set(30),
        ActionSpec::SwitchProfile {
            profile_id: "second".into(),
        },
        ActionSpec::NextPage,
    ];
    engine.perform("mode", &sequence(switching, 100), t);
    engine.perform(
        "other",
        &sequence(vec![set(31), ActionSpec::NextPage], 150),
        t,
    );
    let first_page = engine.runtime.page().id.clone();
    engine.tick(t + ms(100));
    assert_eq!(engine.runtime.profile().id, "second");
    assert_eq!(engine.runtime.brightness, 60);
    // The other sequence's page step is gone; the switching one's stays.
    engine.tick(t + ms(160));
    assert_eq!(engine.runtime.page().id, first_page);
    engine.tick(t + ms(250));
    assert_ne!(engine.runtime.page().id, first_page);

    // Reloading (here: the built-in config) drops waiting steps too.
    let t = t + ms(1_000);
    engine.perform("seq", &sequence(vec![set(10), set(20)], 100), t);
    assert_eq!(engine.runtime.brightness, 10);
    engine.handle_request(Command::ReloadConfig { path: None }, t + ms(50));
    engine.tick(t + ms(200));
    assert_eq!(engine.runtime.brightness, 10);
}

/// N1: a macro that launches an app keeps running when the app's profile
/// takes over the strip.
#[test]
fn focus_follow_does_not_cut_a_running_sequence_short() {
    let scratch = Scratch::new("sequence-focus");
    let mut config = nexus_config::default_config();
    let mut terminal = config.profiles[0].clone();
    terminal.id = "terminal".into();
    terminal.brightness = 80;
    terminal.apps = vec!["konsole".into()];
    config.profiles.push(terminal);
    let mut engine = engine(Runtime::from_config(config, None), &scratch);
    let t = Instant::now();
    let steps = vec![set(70), set(77)];
    engine.perform(
        "macro",
        &ActionSpec::Sequence {
            steps,
            delay_ms: 1_500,
        },
        t,
    );
    let konsole = nexus_sensors::FocusedWindow {
        app_id: "org.kde.konsole".into(),
        class: "konsole".into(),
        title: String::new(),
    };
    engine.follow_focus(&konsole, t + ms(500));
    assert_eq!(engine.runtime.profile().id, "terminal");
    engine.tick(t + ms(1_600));
    assert_eq!(engine.runtime.brightness, 77);
}
