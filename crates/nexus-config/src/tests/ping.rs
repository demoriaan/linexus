use super::*;

fn with_ping(props: PingProps) -> NexusConfig {
    let mut config = default_config();
    config.profiles[0].pages[0].widgets[0].kind = WidgetKind::Ping(props);
    config
}

#[test]
fn ping_widget_and_settings_defaults() {
    let kind: WidgetKind = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
    assert_eq!(kind, WidgetKind::Ping(PingProps::default()));
    assert_eq!(kind.type_name(), "ping");
    let props = PingProps::default();
    assert_eq!(
        (props.host, props.warn_ms, props.critical_ms),
        (None, 60, 120)
    );
    // No host is written while the widget follows the settings.
    let json = serde_json::to_value(&kind).unwrap();
    assert_eq!(
        json,
        serde_json::json!({ "type": "ping", "warn_ms": 60, "critical_ms": 120 })
    );

    // Configs from before keep loading: the settings fill in.
    let settings: Settings = serde_json::from_str(r#"{"locale":"de"}"#).unwrap();
    assert_eq!(settings.ping.host, "1.1.1.1");
    assert_eq!(settings.ping.interval_ms, 1000);
    validate(&with_ping(PingProps::default())).unwrap();
}

#[test]
fn ping_hosts_must_be_names_or_addresses() {
    for good in [
        "1.1.1.1",
        "192.168.0.1",
        "::1",
        "2606:4700:4700::1111",
        "::ffff:192.168.0.1",
        "169.254.1.1",
        "localhost",
        "router.fritz.box",
        "example.com.",
        "a-b.c0",
        &format!("{}.com", "a".repeat(63)),
    ] {
        assert!(is_valid_host(good), "{good}");
    }
    let long = format!("{}.com", vec!["a".repeat(60); 5].join("."));
    for bad in [
        "",
        " 1.1.1.1",
        "1.1.1.1 ",
        "0.0.0.0",
        "::",
        "255.255.255.255",
        "224.0.0.1",
        "ff02::1",
        "[::1]",
        "fe80::1%eth0",
        "fe80::1",
        "febf::1",
        "::ffff:255.255.255.255",
        "::ffff:0.0.0.0",
        "1.2.3",
        "999.1.1.1",
        "host..name",
        ".example.com",
        "-host.com",
        "host-.com",
        "under_score.com",
        "exa mple.com",
        "https://example.com",
        "example.com:443",
        &format!("{}.com", "a".repeat(64)),
        &long,
    ] {
        assert!(!is_valid_host(bad), "{bad:?}");
    }

    let mut config = with_ping(PingProps {
        host: Some("9.9.9.9".into()),
        ..PingProps::default()
    });
    validate(&config).unwrap();
    config.profiles[0].pages[0].widgets[0].kind = WidgetKind::Ping(PingProps {
        host: Some("not a host".into()),
        ..PingProps::default()
    });
    assert!(
        matches!(validate(&config), Err(ConfigError::InvalidPingHost { widget }) if widget == "clock")
    );

    let mut config = default_config();
    config.settings.ping.host = String::new();
    assert_eq!(validate(&config), Err(ConfigError::InvalidDefaultPingHost));
}

#[test]
fn ping_thresholds_and_interval_stay_in_range() {
    for (warn_ms, critical_ms, ok) in [
        (60, 120, true),
        (1, 10_000, true),
        (0, 120, false),
        (60, 10_001, false),
        (120, 120, false),
        (200, 100, false),
    ] {
        let result = validate(&with_ping(PingProps {
            host: None,
            warn_ms,
            critical_ms,
        }));
        assert_eq!(result.is_ok(), ok, "{warn_ms}/{critical_ms}: {result:?}");
    }
    let mut config = default_config();
    for (interval_ms, ok) in [(250, true), (60_000, true), (249, false), (60_001, false)] {
        config.settings.ping.interval_ms = interval_ms;
        assert_eq!(validate(&config).is_ok(), ok, "{interval_ms}");
    }
}

#[test]
fn widgets_name_the_host_they_need_pinged() {
    let settings = PingSettings {
        host: "gw.lan".into(),
        interval_ms: 1000,
    };
    let widget = |kind| Widget::new("w", 0, 100, kind);
    let own = PingProps {
        host: Some("9.9.9.9".into()),
        ..PingProps::default()
    };
    assert_eq!(
        widget(WidgetKind::Ping(own)).ping_host(&settings),
        Some("9.9.9.9")
    );
    assert_eq!(
        widget(WidgetKind::Ping(PingProps::default())).ping_host(&settings),
        Some("gw.lan")
    );
    let metric = |source: &str, secondary: Option<&str>| {
        widget(WidgetKind::Metric(MetricProps {
            source: source.into(),
            secondary: secondary.map(Into::into),
            ..MetricProps::default()
        }))
    };
    assert_eq!(
        metric("net.ping", None).ping_host(&settings),
        Some("gw.lan")
    );
    assert_eq!(
        metric("cpu.usage", Some("net.ping.loss")).ping_host(&settings),
        Some("gw.lan")
    );
    assert_eq!(
        metric("cpu.usage", Some("cpu.temp")).ping_host(&settings),
        None
    );
    assert_eq!(metric("net.pingback", None).ping_host(&settings), None);
    let text = |text: &str| {
        widget(WidgetKind::Text(TextProps {
            text: text.into(),
            ..TextProps::default()
        }))
    };
    assert_eq!(
        text("Ping { net.ping.jitter }").ping_host(&settings),
        Some("gw.lan")
    );
    assert_eq!(text("CPU {cpu.temp} net.ping").ping_host(&settings), None);
    assert_eq!(
        widget(WidgetKind::Network(NetworkProps::default())).ping_host(&settings),
        None
    );
    assert_eq!(
        text_tokens("a {x} b { y } {unclosed").collect::<Vec<_>>(),
        ["x", "y"]
    );
}
