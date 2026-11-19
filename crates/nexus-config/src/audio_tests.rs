use super::*;

#[test]
fn sound_widgets_action_and_settings_parse_and_validate() {
    let output: WidgetKind = serde_json::from_str(r#"{"type":"audio_output"}"#).unwrap();
    assert_eq!(
        output,
        WidgetKind::AudioOutput(AudioOutputProps { show_volume: true })
    );
    assert_eq!(output.type_name(), "audio_output");
    let mixer: WidgetKind =
        serde_json::from_str(r#"{"type":"mixer","pinned":["Spotify","Discord"]}"#).unwrap();
    assert!(matches!(&mixer, WidgetKind::Mixer(p) if p.pinned == ["Spotify", "Discord"]));
    let mic: WidgetKind =
        serde_json::from_str(r#"{"type":"microphone","show_level":false}"#).unwrap();
    assert_eq!(
        mic,
        WidgetKind::Microphone(MicrophoneProps {
            show_level: false,
            show_volume: true,
        })
    );
    // Defaults stay out of the file.
    assert_eq!(
        serde_json::to_value(WidgetKind::Mixer(MixerProps::default())).unwrap(),
        serde_json::json!({ "type": "mixer" })
    );

    let one: ActionSpec =
        serde_json::from_str(r#"{"type":"audio_output","devices":["alsa_output.hdmi"]}"#).unwrap();
    assert_eq!(
        one,
        ActionSpec::AudioOutput {
            devices: vec!["alsa_output.hdmi".into()]
        }
    );
    let all: ActionSpec = serde_json::from_str(r#"{"type":"audio_output"}"#).unwrap();
    assert_eq!(
        all,
        ActionSpec::AudioOutput {
            devices: Vec::new()
        }
    );
    assert!(all.to_external_action().is_none());

    let mut config = default_config();
    assert!(config.settings.overlays.privacy);
    assert!(config.settings.audio.devices.is_empty());
    let json = serde_json::to_value(&config.settings).unwrap();
    assert!(json
        .get("audio")
        .is_some_and(|audio| audio == &serde_json::json!({})));
    config.settings.audio.devices = vec![AudioDeviceSettings {
        node: "alsa_output.usb-Astro_Gaming_Astro_A50-00.stereo-game".into(),
        name: Some("Kopfhörer".into()),
        hidden: Some(false),
    }];
    config.settings.overlays.privacy = false;
    let page = &mut config.profiles[0].pages[0];
    page.widgets = vec![
        Widget::new("out", 0, 320, output),
        Widget::new("mix", 320, 320, mixer).with_tap(one),
    ];
    validate(&config).unwrap();
    let back: NexusConfig = serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
    assert_eq!(back, config);
    assert_eq!(
        back.settings
            .audio
            .device("alsa_output.usb-Astro_Gaming_Astro_A50-00.stereo-game")
            .and_then(|d| d.name.as_deref()),
        Some("Kopfhörer")
    );
}

#[test]
fn bad_sound_settings_are_refused() {
    let base = default_config();
    let device = |node: &str, name: Option<&str>| AudioDeviceSettings {
        node: node.into(),
        name: name.map(str::to_string),
        hidden: None,
    };
    for devices in [
        vec![device("", None)],
        vec![device("a", Some("  "))],
        vec![device("a", None), device("a", Some("again"))],
        vec![device("a", Some(&"x".repeat(MAX_AUDIO_NAME_CHARS + 1)))],
    ] {
        let mut config = base.clone();
        config.settings.audio.devices = devices;
        assert!(matches!(
            validate(&config),
            Err(ConfigError::InvalidAudioDevice { .. })
        ));
    }
    let mut config = base.clone();
    config.profiles[0].pages[0].widgets[0].kind = WidgetKind::Mixer(MixerProps {
        pinned: vec![String::new()],
    });
    assert!(matches!(
        validate(&config),
        Err(ConfigError::InvalidAudioList { .. })
    ));
    let mut config = base;
    let widget = &mut config.profiles[0].pages[0].widgets[0];
    widget.tap = Some(ActionSpec::AudioOutput {
        devices: vec!["a".into(); MAX_AUDIO_LIST + 1],
    });
    assert!(matches!(
        validate(&config),
        Err(ConfigError::InvalidAudioList { .. })
    ));
}
