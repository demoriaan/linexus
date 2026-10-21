use super::*;
use nexus_actions::{Action, VolumeCommand};

#[test]
fn default_config_is_valid_and_round_trips() {
    let config = default_config();
    validate(&config).unwrap();
    let json = to_pretty_json(&config).unwrap();
    let (parsed, report) = parse(&json).unwrap();
    assert!(report.is_none());
    assert_eq!(parsed, config);
}

#[test]
fn every_template_is_valid_on_its_own() {
    for template in PageTemplate::ALL {
        let mut config = default_config();
        config.profiles[0].pages = vec![template_page(template, "page")];
        if template == PageTemplate::Blank {
            // A blank page is valid; the profile just needs one visible page.
            validate(&config).unwrap();
            continue;
        }
        validate(&config).unwrap_or_else(|e| panic!("{template:?}: {e}"));
        assert!(!config.profiles[0].pages[0].widgets.is_empty());
    }
}

#[test]
fn widgets_serialize_flat_with_a_type_tag() {
    let widget = button("b", 0, 100, "Web", "lucide:globe").with_tap(ActionSpec::OpenUrl {
        url: "https://example.com".into(),
    });
    let value = serde_json::to_value(&widget).unwrap();
    assert_eq!(value["type"], "button");
    assert_eq!(value["label"], "Web");
    assert_eq!(value["tap"]["type"], "open_url");
    let back: Widget = serde_json::from_value(value).unwrap();
    assert_eq!(back, widget);
}

#[test]
fn widget_props_fall_back_to_defaults() {
    let widget: Widget =
        serde_json::from_str(r#"{"id":"m","x":0,"w":120,"type":"metric","source":"gpu.temp"}"#)
            .unwrap();
    let WidgetKind::Metric(props) = widget.kind else {
        panic!("expected a metric widget");
    };
    assert_eq!(props.source, "gpu.temp");
    assert_eq!(props.style, MetricStyle::Sparkline);
}

#[test]
fn rejects_overlapping_widgets() {
    let mut config = default_config();
    let page = &mut config.profiles[0].pages[0];
    page.widgets[1].x = 90;
    assert!(matches!(
        validate(&config),
        Err(ConfigError::WidgetsOverlap { .. })
    ));
}

#[test]
fn rejects_widgets_outside_the_strip_or_too_narrow() {
    let mut config = default_config();
    config.profiles[0].pages[0].widgets[4].w = 200;
    assert!(matches!(
        validate(&config),
        Err(ConfigError::WidgetOutOfBounds { .. })
    ));
    let mut config = default_config();
    config.profiles[0].pages[0].widgets[0].w = 10;
    assert!(matches!(
        validate(&config),
        Err(ConfigError::WidgetTooNarrow { .. })
    ));
}

#[test]
fn rejects_dangling_page_and_profile_targets() {
    let mut config = default_config();
    config.profiles[0].pages[2].widgets[0].tap = Some(ActionSpec::GoToPage {
        page_id: "nope".into(),
    });
    assert!(matches!(
        validate(&config),
        Err(ConfigError::MissingPageTarget { .. })
    ));
    let mut config = default_config();
    config.profiles[0].pages[2].widgets[0].tap = Some(ActionSpec::Sequence {
        steps: vec![ActionSpec::SwitchProfile {
            profile_id: "nope".into(),
        }],
        delay_ms: 0,
    });
    assert!(matches!(
        validate(&config),
        Err(ConfigError::MissingProfileTarget { .. })
    ));
}

#[test]
fn rejects_shell_programs_with_paths() {
    let mut config = default_config();
    config.profiles[0].pages[2].widgets[0].tap = Some(ActionSpec::ShellCommand {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "true".into()],
    });
    assert!(matches!(
        validate(&config),
        Err(ConfigError::InvalidShellProgram { .. })
    ));
}

#[test]
fn a_profile_needs_one_visible_page() {
    let mut config = default_config();
    for page in &mut config.profiles[0].pages {
        page.hidden = true;
    }
    assert!(matches!(
        validate(&config),
        Err(ConfigError::EmptyProfile(_))
    ));
}

#[test]
fn hit_tests_widgets_by_x() {
    let config = default_config();
    let launcher = &config.profiles[0].pages[2];
    assert_eq!(widget_at_x(launcher, 0).unwrap().id, "browser");
    assert_eq!(widget_at_x(launcher, 107).unwrap().id, "files");
    assert_eq!(widget_at_x(launcher, 639).unwrap().id, "lock");
}

#[test]
fn maps_external_actions() {
    let action = ActionSpec::Volume {
        command: VolumeAction::Up,
        step: Some(7),
        percent: None,
    }
    .to_external_action()
    .unwrap();
    assert_eq!(action, Action::Volume(VolumeCommand::Up(7)));
    assert!(ActionSpec::NextPage.to_external_action().is_none());
    assert!(ActionSpec::HomeAssistant {
        service: "light.toggle".into(),
        entity_id: None
    }
    .to_external_action()
    .is_none());
}

#[test]
fn hex_colors_parse_and_print() {
    let color: HexColor = "#3EE6CA".parse().unwrap();
    assert_eq!(color, HexColor::rgb(0x3e, 0xe6, 0xca));
    assert_eq!(color.to_string(), "#3ee6ca");
    assert_eq!(
        "#fff".parse::<HexColor>().unwrap(),
        HexColor::rgb(255, 255, 255)
    );
    assert!("#12".parse::<HexColor>().is_err());
    // Multi-byte characters must fail, not panic on a char boundary.
    assert!("#aaaéa".parse::<HexColor>().is_err());
    assert!("#€".parse::<HexColor>().is_err());
    assert!("#+f+f+f".parse::<HexColor>().is_err());
    let mut config = serde_json::to_value(default_config()).unwrap();
    config["profiles"][0]["theme"]["accent"] = "#€".into();
    assert!(parse(&config.to_string()).is_err());
}

const V1_CONFIG: &str = r#"{
  "schema_version": 1,
  "active_profile": "desk",
  "profiles": [
    {
      "id": "desk", "name": "Desk", "brightness": 90, "refresh_ms": 1000, "theme": "cyberpunk",
      "pages": [
        {"id": "status", "name": "System", "kind": "status",
         "elements": [{"id": "next", "label": "Perf", "x": 560, "width": 80, "action": {"type": "screen_next"}}]},
        {"id": "sensors", "name": "Sensors", "kind": "sensors", "elements": []},
        {"id": "apps", "name": "Apps", "kind": "button_grid",
         "background": {"image_data_url": "data:image/png;base64,AAAA", "scale": "fit", "dim_percent": 95},
         "elements": [
           {"id": "browser", "label": "Web", "x": 0, "width": 106, "icon": "browser",
            "action": {"type": "open_url", "url": "https://example.com"}},
           {"id": "play", "label": "Play", "x": 106, "width": 106, "icon": "media-playback-start",
            "action": {"type": "media_key", "command": "play_pause"}},
           {"id": "clash", "label": "Clash", "x": 150, "width": 60,
            "action": {"type": "volume", "command": "up", "step": 5}},
           {"id": "media", "label": "Media", "x": 530, "width": 80,
            "action": {"type": "profile_switch", "profile_id": "gone"}},
           {"id": "prev", "label": "Back", "x": 424, "width": 80, "action": {"type": "screen_previous"}}
         ]}
      ]
    }
  ]
}"#;

#[test]
fn migrates_schema_1_configs() {
    let (config, report) = parse(V1_CONFIG).unwrap();
    let report = report.expect("v1 needs a migration");
    assert_eq!(report.from_version, 1);
    validate(&config).unwrap();

    let profile = &config.profiles[0];
    assert_eq!(profile.theme.preset, "neon");
    assert_eq!(profile.brightness, 90);
    let ids: Vec<&str> = profile.pages.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["status", "sensors", "apps"]);
    assert_eq!(profile.pages[0].name, "System");
    assert!(matches!(
        profile.pages[1].widgets[0].kind,
        WidgetKind::News(_)
    ));

    let apps = &profile.pages[2];
    let ids: Vec<&str> = apps.widgets.iter().map(|w| w.id.as_str()).collect();
    // `clash` overlapped `play`, `prev` was pure navigation.
    assert_eq!(ids, ["browser", "play", "media"]);
    let WidgetKind::Button(play) = &apps.widgets[1].kind else {
        panic!("expected a button");
    };
    assert_eq!(play.icon.as_deref(), Some("lucide:play"));
    assert!(matches!(
        apps.widgets[1].tap,
        Some(ActionSpec::Media {
            command: MediaKey::PlayPause,
            ..
        })
    ));
    // The profile it switched to no longer exists.
    assert_eq!(apps.widgets[2].tap, None);
    let background = apps.background.as_ref().unwrap();
    assert_eq!(background.fit, ImageFit::Fit);
    assert_eq!(background.dim_percent, 90);
    assert!(report.notes.len() >= 2);
}

#[test]
fn load_and_upgrade_rewrites_old_files_and_keeps_a_backup() {
    let dir = std::env::temp_dir().join(format!("nexus-config-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.json");
    std::fs::write(&path, V1_CONFIG).unwrap();
    let (config, report) = load_and_upgrade(&path).unwrap();
    assert!(report.is_some());
    assert!(dir.join("config.json.v1.bak").exists());
    let (reloaded, report) = load_with_report(&path).unwrap();
    assert!(report.is_none());
    assert_eq!(reloaded, config);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn all_templates_together_are_valid_and_unchanged_by_loading() {
    let mut config = default_config();
    config.profiles[0].pages = PageTemplate::ALL
        .iter()
        .enumerate()
        .map(|(i, template)| template_page(*template, &format!("page{i}")))
        .collect();
    validate(&config).unwrap();
    let (parsed, _) = parse(&to_pretty_json(&config).unwrap()).unwrap();
    assert_eq!(parsed, config);
}

fn with_tap(action: ActionSpec) -> NexusConfig {
    let mut config = default_config();
    config.profiles[0].pages[2].widgets[0].tap = Some(action);
    config
}

#[test]
fn studio_action_defaults_validate_unless_they_need_a_choice() {
    // The values Studio fills in when an action type is picked.
    for json in [
        r#"{"type":"open_url","url":"https://"}"#,
        r#"{"type":"open_path","path":"~/"}"#,
        r#"{"type":"launch_app","target":"default:browser"}"#,
        r#"{"type":"media","command":"play_pause"}"#,
        r#"{"type":"volume","command":"up","step":5}"#,
        r#"{"type":"keyboard_shortcut","keys":[]}"#,
        r#"{"type":"text_input","text":""}"#,
        r#"{"type":"http_request","method":"POST","url":"https://"}"#,
        r#"{"type":"home_assistant","service":"light.toggle","entity_id":""}"#,
        r#"{"type":"brightness","command":"cycle"}"#,
        r#"{"type":"sequence","steps":[],"delay_ms":0}"#,
    ] {
        let action: ActionSpec = serde_json::from_str(json).unwrap();
        validate(&with_tap(action)).unwrap_or_else(|e| panic!("{json}: {e}"));
    }
    // An empty target or program is not a valid action.
    for json in [
        r#"{"type":"go_to_page","page_id":""}"#,
        r#"{"type":"switch_profile","profile_id":""}"#,
        r#"{"type":"shell_command","program":"","args":[]}"#,
    ] {
        let action: ActionSpec = serde_json::from_str(json).unwrap();
        assert!(validate(&with_tap(action)).is_err(), "{json}");
    }
}

#[test]
fn rejects_out_of_range_numbers() {
    let volume = |percent| ActionSpec::Volume {
        command: VolumeAction::Set,
        step: None,
        percent: Some(percent),
    };
    validate(&with_tap(volume(100))).unwrap();
    let err = validate(&with_tap(volume(150))).unwrap_err();
    assert!(matches!(
        err,
        ConfigError::VolumeSetTooHigh { percent: 150, .. }
    ));
    assert_eq!(
        err.to_string(),
        "widget 'browser' sets the volume to 150%; the most is 100%"
    );
    // A percentage left over from switching to "up" is not used.
    validate(&with_tap(ActionSpec::Volume {
        command: VolumeAction::Up,
        step: Some(5),
        percent: Some(150),
    }))
    .unwrap();
    assert!(matches!(
        validate(&with_tap(ActionSpec::Brightness {
            command: BrightnessAction::Set,
            percent: Some(101),
        })),
        Err(ConfigError::BrightnessSetTooHigh { percent: 101, .. })
    ));

    let mut config = default_config();
    config.settings.idle.dim_percent = 101;
    assert!(matches!(
        validate(&config),
        Err(ConfigError::InvalidIdleDim { dim_percent: 101 })
    ));

    let mut config = default_config();
    config.profiles[0].pages[4].auto_return_seconds = Some(0);
    assert!(matches!(
        validate(&config),
        Err(ConfigError::InvalidAutoReturn { .. })
    ));
    config.profiles[0].pages[4].auto_return_seconds = Some(1);
    validate(&config).unwrap();
}

#[test]
fn bounds_sequences() {
    let sequence = |steps: usize, delay_ms| ActionSpec::Sequence {
        steps: vec![ActionSpec::NextPage; steps],
        delay_ms,
    };
    validate(&with_tap(sequence(
        MAX_SEQUENCE_STEPS,
        MAX_SEQUENCE_DELAY_MS,
    )))
    .unwrap();
    assert!(matches!(
        validate(&with_tap(sequence(MAX_SEQUENCE_STEPS + 1, 0))),
        Err(ConfigError::SequenceTooLong { steps: 17, .. })
    ));
    assert!(matches!(
        validate(&with_tap(sequence(2, MAX_SEQUENCE_DELAY_MS + 1))),
        Err(ConfigError::SequenceDelayTooLong { .. })
    ));
}

#[test]
fn rejects_empty_ids_and_header_injection() {
    let mut config = default_config();
    config.profiles[0].id = String::new();
    config.active_profile = String::new();
    assert!(matches!(
        validate(&config),
        Err(ConfigError::EmptyProfileId)
    ));
    let mut config = default_config();
    config.profiles[0].pages[1].id = " ".into();
    assert!(matches!(
        validate(&config),
        Err(ConfigError::EmptyPageId { .. })
    ));
    let mut config = default_config();
    config.profiles[0].pages[1].widgets[0].id = String::new();
    assert!(matches!(
        validate(&config),
        Err(ConfigError::EmptyWidgetId { .. })
    ));

    let http = |content_type: &str| ActionSpec::HttpRequest {
        method: HttpMethod::Post,
        url: "https://example.com/hook".into(),
        body: Some("@here build done".into()),
        content_type: Some(content_type.into()),
    };
    validate(&with_tap(http("text/plain"))).unwrap();
    for bad in ["text/plain\r\nX-Evil: 1", "text/plain\nX-Evil: 1"] {
        assert!(matches!(
            validate(&with_tap(http(bad))),
            Err(ConfigError::InvalidContentType { .. })
        ));
    }
}

#[test]
fn loading_clamps_values_older_versions_accepted() {
    let mut config = serde_json::to_value(default_config()).unwrap();
    config["settings"]["idle"]["dim_percent"] = 150.into();
    let pages = &mut config["profiles"][0]["pages"];
    pages[4]["auto_return_seconds"] = 0.into();
    pages[2]["widgets"][0]["tap"] = serde_json::json!({"type": "sequence", "delay_ms": 90000,
        "steps": [{"type": "volume", "command": "set", "percent": 150},
                  {"type": "brightness", "command": "set", "percent": 120}]});

    let (config, report) = parse(&config.to_string()).unwrap();
    assert!(report.is_none());
    validate(&config).unwrap();
    assert_eq!(config.settings.idle.dim_percent, 100);
    let pages = &config.profiles[0].pages;
    assert_eq!(pages[4].auto_return_seconds, Some(1));
    let Some(ActionSpec::Sequence { steps, delay_ms }) = &pages[2].widgets[0].tap else {
        panic!("expected a sequence");
    };
    assert_eq!(*delay_ms, MAX_SEQUENCE_DELAY_MS);
    assert!(matches!(
        steps[0],
        ActionSpec::Volume {
            percent: Some(100),
            ..
        }
    ));
    assert!(matches!(
        steps[1],
        ActionSpec::Brightness {
            percent: Some(100),
            ..
        }
    ));
}

#[test]
fn loading_repairs_long_sequences_blank_ids_and_split_headers() {
    let mut config = serde_json::to_value(default_config()).unwrap();
    let steps: Vec<_> = (0..20)
        .map(|_| serde_json::json!({"type": "next_page"}))
        .collect();
    let widgets = &mut config["profiles"][0]["pages"][2]["widgets"];
    widgets[0]["tap"] = serde_json::json!({"type": "sequence", "steps": steps});
    widgets[1]["id"] = "".into();
    widgets[2]["tap"] = serde_json::json!({"type": "http_request", "method": "POST",
        "url": "https://example.com", "content_type": "text/plain\r\nX-Evil: 1"});
    widgets[3]["tap"] = serde_json::json!({"type": "go_to_page", "page_id": " "});
    widgets[4]["tap"] = serde_json::json!({"type": "switch_profile", "profile_id": ""});
    config["profiles"][0]["pages"][1]["id"] = " ".into();
    let mut blank = config["profiles"][0].clone();
    blank["id"] = "".into();
    config["profiles"].as_array_mut().unwrap().push(blank);
    config["active_profile"] = "".into();

    let (config, _) = parse(&config.to_string()).unwrap();
    validate(&config).unwrap();
    let page = &config.profiles[0].pages[2];
    let Some(ActionSpec::Sequence { steps, .. }) = &page.widgets[0].tap else {
        panic!("expected a sequence");
    };
    assert_eq!(steps.len(), MAX_SEQUENCE_STEPS);
    assert_eq!(page.widgets[1].id, "widget-1");
    assert_eq!(config.profiles[0].pages[1].id, "page-1");
    assert_eq!(config.profiles[1].id, "profile-1");
    assert_eq!(config.active_profile, "profile-1");
    assert_eq!(
        page.widgets[3].tap,
        Some(ActionSpec::GoToPage {
            page_id: "page-1".into()
        })
    );
    assert_eq!(
        page.widgets[4].tap,
        Some(ActionSpec::SwitchProfile {
            profile_id: "profile-1".into()
        })
    );
    let Some(ActionSpec::HttpRequest { content_type, .. }) = &page.widgets[2].tap else {
        panic!("expected a web request");
    };
    assert_eq!(content_type.as_deref(), Some("text/plain"));
}

#[test]
fn migration_reports_buttons_that_do_not_fit_a_template_page() {
    let v1 = r#"{"schema_version": 1, "active_profile": "desk", "profiles": [{"id": "desk", "pages": [
        {"id": "media", "kind": "media", "elements": [
          {"id": "launch", "label": "Term", "x": 0, "width": 106,
           "action": {"type": "launch_app", "target": "konsole"}},
          {"id": "next", "x": 560, "width": 80, "action": {"type": "screen_next"}},
          {"label": "No id", "x": 300, "width": 80}]},
        {"id": "grid", "kind": "button_grid", "elements": [
          {"id": "far", "label": "Far", "x": 70000, "width": 70100},
          {"id": "", "label": "Empty", "x": 0, "width": 80}]}]}]}"#;
    let (config, report) = parse(v1).unwrap();
    let report = report.unwrap();
    validate(&config).unwrap();
    let media = &config.profiles[0].pages[0];
    assert_eq!(
        media.widgets,
        template_page(PageTemplate::Media, "media").widgets
    );
    assert!(report.notes.iter().any(|note| note
        == "page 'media': button 'launch' had no room beside the page's built-in widgets and was dropped"));
    // The navigation button is dropped on purpose, without a note.
    assert!(!report.notes.iter().any(|note| note.contains("'next'")));
    assert_eq!(
        report
            .notes
            .iter()
            .filter(|note| note.contains("without an id"))
            .count(),
        2
    );
    // Out-of-range positions are clamped into the strip, not wrapped.
    let far = &config.profiles[0].pages[1].widgets[0];
    assert_eq!(
        (far.x, far.w),
        (STRIP_WIDTH - MIN_WIDGET_WIDTH, MIN_WIDGET_WIDTH)
    );
    assert_eq!(report.buttons, 1);

    let huge = r#"{"schema_version": 4294967297, "active_profile": "x", "profiles": []}"#;
    assert!(parse(huge)
        .unwrap_err()
        .contains("unsupported config schema version 4294967297"));
}

#[test]
fn migrated_buttons_fill_free_gaps_beside_existing_widgets() {
    let page: serde_json::Value = serde_json::from_str(
        r#"{"elements": [
          {"id": "fits", "x": 300, "width": 100},
          {"id": "clock", "x": 500, "width": 100},
          {"id": "blocked", "x": 50, "width": 100}]}"#,
    )
    .unwrap();
    let mut widgets = vec![Widget::new(
        "clock",
        0,
        200,
        WidgetKind::Clock(ClockProps::default()),
    )];
    let mut report = MigrationReport::default();
    migrate::migrate_elements(&page, &mut widgets, "p", &mut report);
    let ids: Vec<&str> = widgets.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, ["clock", "fits"]);
    assert_eq!(report.buttons, 1);
    assert_eq!(report.notes.len(), 2);
    assert!(report.notes[1].contains("reused the id"));
}

#[test]
fn concurrent_saves_never_mix_their_files() {
    let dir = std::env::temp_dir().join(format!("nexus-config-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("config.json");
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let path = path.clone();
            std::thread::spawn(move || {
                let mut config = default_config();
                config.profiles[0].name = format!("writer {i} {}", "x".repeat(i * 4096));
                for _ in 0..10 {
                    save_to_path(&path, &config, true).unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    load_from_path(&path).unwrap();
    let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
    assert_eq!(entries.len(), 1, "temp files left behind");

    // A failed rename (the target is a directory) removes the temp file.
    let blocked = dir.join("blocked.json");
    std::fs::create_dir(&blocked).unwrap();
    std::fs::write(blocked.join("keep"), b"").unwrap();
    assert!(save_to_path(&blocked, &default_config(), true).is_err());
    let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
    assert_eq!(
        entries.len(),
        2,
        "temp file left behind after a failed save"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn obs_actions_name_their_scene_or_source() {
    let action: ActionSpec =
        serde_json::from_str(r#"{"type":"obs","command":"set_scene","target":"Gameplay"}"#)
            .unwrap();
    assert_eq!(
        action,
        ActionSpec::Obs {
            command: ObsCommand::SetScene,
            target: Some("Gameplay".into()),
        }
    );
    assert!(action.to_external_action().is_none());
    let record: ActionSpec =
        serde_json::from_str(r#"{"type":"obs","command":"toggle_record"}"#).unwrap();
    assert_eq!(
        serde_json::to_value(&record).unwrap(),
        serde_json::json!({"type": "obs", "command": "toggle_record"})
    );

    let mut config = default_config();
    config.profiles[0].pages = vec![template_page(PageTemplate::Streaming, "stream")];
    validate(&config).unwrap();
    for target in [None, Some("  ")] {
        config.profiles[0].pages[0].widgets[3].tap = Some(ActionSpec::Obs {
            command: ObsCommand::ToggleMute,
            target: target.map(Into::into),
        });
        assert!(matches!(
            validate(&config),
            Err(ConfigError::ObsTargetMissing { .. })
        ));
    }
    config.profiles[0].pages[0].widgets[3].tap = Some(ActionSpec::Obs {
        command: ObsCommand::ToggleVirtualCam,
        target: None,
    });
    validate(&config).unwrap();
}
