use super::*;

/// Captured from `busctl --user monitor --json=short` under Plasma 6.
const PLASMA_NOTIFY: &str = r#"{"type":"method_call","endian":"l","flags":0,"version":1,"cookie":9,"timestamp-realtime":1792460300634481,"sender":":1.118337","destination":":1.14990","path":"/org/freedesktop/Notifications","interface":"org.freedesktop.Notifications","member":"Notify","payload":{"type":"susssasa{sv}i","data":["LinexusTest",0,"","Hallo Strip","Testkörper mit Umlaut äöü",[],{"image-path":{"type":"s","data":"dialog-information"},"urgency":{"type":"y","data":1},"sender-pid":{"type":"x","data":3950106}},-1]}}"#;

fn notify(
    app: &str,
    replaces: u32,
    summary: &str,
    body: &str,
    hints: &str,
    timeout: i32,
) -> String {
    format!(
        r#"{{"type":"method_call","sender":":1.5","path":"/org/freedesktop/Notifications","interface":"org.freedesktop.Notifications","member":"Notify","payload":{{"type":"susssasa{{sv}}i","data":[{},{replaces},"app-icon",{},{},["default","Open"],{{{hints}}},{timeout}]}}}}"#,
        serde_json::to_string(app).unwrap(),
        serde_json::to_string(summary).unwrap(),
        serde_json::to_string(body).unwrap(),
    )
}

fn add_notification(interface: &str, app: &str, id: &str, fields: &str) -> String {
    format!(
        r#"{{"type":"method_call","sender":":1.7","path":"/x","interface":"{interface}","member":"AddNotification","payload":{{"type":"ssa{{sv}}","data":["{app}","{id}",{{{fields}}}]}}}}"#
    )
}

fn parse(lines: &[String]) -> Vec<Notification> {
    let mut parser = MonitorParser::default();
    let now = Instant::now();
    lines
        .iter()
        .filter_map(|line| parser.push_line(line, now))
        .collect()
}

#[test]
fn parses_a_captured_notify_call() {
    let found = parse(&[PLASMA_NOTIFY.to_string()]);
    assert_eq!(
        found,
        vec![Notification {
            app_name: "LinexusTest".into(),
            desktop_entry: None,
            kde_service: None,
            summary: "Hallo Strip".into(),
            body: "Testkörper mit Umlaut äöü".into(),
            // notify-send puts its icon name in the image path.
            image: None,
            app_icons: vec![NotificationIcon::Named("dialog-information".into())],
            urgency: Urgency::Normal,
            timeout: None,
            replaces: false,
        }]
    );
}

#[test]
fn reads_hints_markup_timeout_and_updates() {
    let hints = r#""image-data":{"type":"(iiibiiay)","data":[2,1,8,true,8,4,[255,0,0,255,0,255,0,128]]},"desktop-entry":{"type":"s","data":"org.kde.konsole.desktop"},"x-kde-appname":{"type":"s","data":"konsole"},"urgency":{"type":"y","data":2}"#;
    let found = parse(&[notify(
        "Konsole",
        7,
        "Done",
        "<b>make</b> &amp; test \"ok\"\nnext",
        hints,
        5000,
    )]);
    let n = &found[0];
    assert_eq!(n.desktop_entry.as_deref(), Some("org.kde.konsole"));
    assert_eq!(n.kde_service.as_deref(), Some("konsole"));
    assert_eq!(n.body, "make & test \"ok\"\nnext");
    assert_eq!(n.urgency, Urgency::Critical);
    assert_eq!(n.timeout, Some(Duration::from_secs(5)));
    assert!(n.replaces);
    assert_eq!(
        n.image,
        Some(NotificationIcon::Pixels {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 128],
        })
    );
    assert_eq!(
        n.app_icons,
        vec![NotificationIcon::Named("app-icon".into())]
    );
    let path = r#""image-path":{"type":"s","data":"/home/u/avatar.png"}"#;
    let with_path = parse(&[notify("Chat", 0, "Anna", "Hi", path, -1)]);
    assert_eq!(
        with_path[0].image,
        Some(NotificationIcon::Named("/home/u/avatar.png".into()))
    );
    let low = parse(&[notify(
        "a",
        0,
        "s",
        "",
        r#""urgency":{"type":"y","data":0}"#,
        0,
    )]);
    assert_eq!((low[0].urgency, low[0].timeout), (Urgency::Low, None));
}

#[test]
fn parses_gtk_and_portal_notifications() {
    let gtk = add_notification(
        "org.gtk.Notifications",
        "org.gnome.Calendar",
        "event-1",
        r#""title":{"type":"s","data":"Meeting"},"body":{"type":"s","data":"In 5 minutes"},"priority":{"type":"s","data":"urgent"},"icon":{"type":"(sv)","data":["themed",{"type":"as","data":["x-office-calendar-symbolic","x-office-calendar"]}]}"#,
    );
    let update = add_notification(
        "org.gtk.Notifications",
        "org.gnome.Calendar",
        "event-1",
        r#""title":{"type":"s","data":"Meeting"},"body":{"type":"s","data":"Now"},"urgent":{"type":"b","data":false}"#,
    );
    let portal = add_notification(
        "org.freedesktop.impl.portal.Notification",
        "org.telegram.desktop",
        "42",
        r#""title":{"type":"s","data":"Anna"},"markup-body":{"type":"s","data":"<i>Hi</i> there"},"priority":{"type":"s","data":"low"},"icon":{"type":"(sv)","data":["file",{"type":"s","data":"file:///tmp/a.png"}]}"#,
    );
    let found = parse(&[gtk, update, portal]);
    assert_eq!(found.len(), 3);
    assert_eq!(found[0].app_name, "org.gnome.Calendar");
    assert_eq!(
        found[0].desktop_entry.as_deref(),
        Some("org.gnome.Calendar")
    );
    assert_eq!(
        (found[0].summary.as_str(), found[0].body.as_str()),
        ("Meeting", "In 5 minutes")
    );
    assert_eq!(found[0].urgency, Urgency::Critical);
    assert!(!found[0].replaces);
    assert_eq!(found[0].image, None);
    assert_eq!(
        found[0].app_icons,
        vec![
            NotificationIcon::Named("x-office-calendar".into()),
            NotificationIcon::Named("x-office-calendar-symbolic".into()),
        ]
    );
    assert!(found[1].replaces);
    assert_eq!(found[1].urgency, Urgency::Normal);
    assert_eq!(found[2].body, "Hi there");
    assert_eq!(found[2].urgency, Urgency::Low);
    assert_eq!(
        found[2].image,
        Some(NotificationIcon::Named("file:///tmp/a.png".into()))
    );
    assert!(found[2].app_icons.is_empty());
}

#[test]
fn a_notification_passed_on_shows_once() {
    let fields = r#""title":{"type":"s","data":"Anna"},"body":{"type":"s","data":"Hi"}"#;
    let portal = add_notification(
        "org.freedesktop.impl.portal.Notification",
        "org.x.App",
        "1",
        fields,
    );
    let shell = add_notification("org.gtk.Notifications", "org.x.App", "portal-1", fields);
    let mut parser = MonitorParser::default();
    let t = Instant::now();
    assert!(parser.push_line(&portal, t).is_some());
    assert!(parser
        .push_line(&shell, t + Duration::from_millis(5))
        .is_none());
    // The same text later is a new notification, and so is the same text
    // from another app.
    assert!(parser
        .push_line(&shell, t + Duration::from_secs(4))
        .is_some());
    let other = add_notification("org.gtk.Notifications", "org.y.App", "1", fields);
    assert!(parser
        .push_line(&other, t + Duration::from_secs(4))
        .is_some());
}

#[test]
fn ignores_other_messages_and_bad_input() {
    let signal = PLASMA_NOTIFY.replace("\"method_call\"", "\"signal\"");
    let other = PLASMA_NOTIFY.replace("\"Notify\"", "\"CloseNotification\"");
    let short = PLASMA_NOTIFY.replace(",-1]", "]");
    let empty = notify("app", 0, "", "  ", "", -1);
    let lines = [
        signal,
        other,
        short,
        empty,
        "not json".into(),
        String::new(),
    ];
    assert!(parse(&lines).is_empty());
}

#[test]
fn converts_inline_images() {
    // RGB rows padded to a stride of 8.
    let rgb = serde_json::json!([
        2,
        2,
        8,
        false,
        8,
        3,
        [1, 2, 3, 4, 5, 6, 0, 0, 7, 8, 9, 10, 11, 12]
    ]);
    assert_eq!(
        inline_image(&rgb),
        Some(NotificationIcon::Pixels {
            width: 2,
            height: 2,
            rgba: vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255],
        })
    );
    let short = serde_json::json!([2, 2, 8, false, 8, 3, [1, 2, 3]]);
    let sixteen_bit = serde_json::json!([1, 1, 8, true, 16, 4, [0, 0, 0, 0, 0, 0, 0, 0]]);
    let huge = serde_json::json!([5000, 1, 20000, true, 8, 4, []]);
    let overflowing = serde_json::json!([u64::MAX / 2, 3, u64::MAX / 2, true, 8, 4, [0, 0, 0, 0]]);
    for bad in [short, sixteen_bit, huge, overflowing] {
        assert_eq!(inline_image(&bad), None);
    }
}

#[test]
fn reads_serialized_icons() {
    let bytes = serde_json::json!(["bytes", {"type": "ay", "data": [137, 80]}]);
    assert_eq!(
        serialized_icon(&bytes),
        vec![NotificationIcon::Encoded(vec![137, 80])]
    );
    assert_eq!(
        serialized_icon(&serde_json::json!("mail-unread")),
        vec![NotificationIcon::Named("mail-unread".into())]
    );
    assert!(
        serialized_icon(&serde_json::json!(["file-descriptor", {"type": "h", "data": 3}]))
            .is_empty()
    );
}

#[test]
fn strips_notification_markup() {
    assert_eq!(
        strip_markup("<b>Bold</b> <i>it</i> <a href=\"https://x.test\">link</a><br/>next"),
        "Bold it link\nnext"
    );
    assert_eq!(strip_markup("a < b and c > d"), "a < b and c > d");
    assert_eq!(strip_markup("<img src=\"x.png\" alt=\"\"/>text"), "text");
    assert_eq!(
        strip_markup("&lt;tag&gt; &#228;&#xE4; &bogus; AT&T"),
        "<tag> ää &bogus; AT&T"
    );
    assert_eq!(
        strip_markup("<unknown>kept</unknown>"),
        "<unknown>kept</unknown>"
    );
}

#[test]
fn long_text_is_cut_and_stays_fast() {
    // Text full of `<` and `&` used to take quadratic time.
    let body = "<x &".repeat(250_000);
    let started = Instant::now();
    let found = parse(&[notify("app", 0, &"s".repeat(10_000), &body, "", -1)]);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(found[0].summary.chars().count(), MAX_SUMMARY_CHARS);
    assert!(found[0].body.chars().count() <= MAX_BODY_CHARS);
    assert_eq!(clip("äöü", 2), "äö");
    assert_eq!(plain_body("<b>a</b>"), "a");
}

#[test]
fn huge_inline_images_are_dropped_but_the_notification_stays() {
    let pixels = vec!["7"; MAX_BYTE_ARRAY + 4].join(",");
    let hints = format!(
        r#""image-data":{{"type":"(iiibiiay)","data":[1,1,4,true,8,4,[{pixels}]]}},"note":{{"type":"s","data":"[1,2,3]"}}"#
    );
    let line = notify("Chat", 0, "Anna", "Hi", &hints, -1);
    let shrunk = shrink_line(&line).unwrap();
    assert!(shrunk.len() < 1000);
    // Small arrays and arrays inside strings stay.
    assert!(shrunk.contains(r#""data":"[1,2,3]""#));
    assert!(matches!(
        shrink_line(PLASMA_NOTIFY),
        Some(std::borrow::Cow::Borrowed(_))
    ));
    let found = parse(&[line]);
    assert_eq!(found[0].summary, "Anna");
    assert_eq!(found[0].image, None);
    // A picture just under the limit is kept and counts as one value.
    let pixels = vec!["7"; 4 * 100 * 100].join(",");
    let hints =
        format!(r#""image-data":{{"type":"(iiibiiay)","data":[100,100,400,true,8,4,[{pixels}]]}}"#);
    let found = parse(&[notify("Chat", 0, "Anna", "Photo", &hints, -1)]);
    assert!(matches!(
        found[0].image,
        Some(NotificationIcon::Pixels { width: 100, .. })
    ));
}

#[test]
fn lines_with_too_many_values_are_dropped() {
    let many = MAX_JSON_VALUES + 10;
    for array in [
        vec!["true"; many].join(","),
        vec!["\"\""; many].join(","),
        vec!["[]"; many].join(","),
        vec!["[0]"; many].join(","),
    ] {
        let hints = format!(r#""junk":{{"type":"av","data":[{array}]}}"#);
        let line = notify("Spam", 0, "S", "B", &hints, -1);
        assert!(shrink_line(&line).is_none());
        assert!(parse(&[line]).is_empty());
    }
}

#[test]
fn many_byte_arrays_share_one_budget() {
    // Each array is just under the limit; together they are far over it.
    let array = format!("[{}]", vec!["0"; MAX_BYTE_ARRAY - 1].join(","));
    let arrays = vec![array; 3].join(",");
    let hints = format!(r#""junk":{{"type":"aay","data":[{arrays}]}}"#);
    let line = notify("Spam", 0, "S", "B", &hints, -1);
    let shrunk = shrink_line(&line).unwrap();
    assert!(shrunk.len() < line.len() / 2);
    assert_eq!(shrunk.matches("[]").count(), 2);
}

#[test]
fn file_icons_count_as_pictures() {
    // Browsers pass a web notification's icon (often an avatar) as a file
    // in app_icon; private mode must be able to leave it out.
    let line = |icon: &str| {
        notify("Brave", 0, "Anna", "Hi", "", -1).replace("\"app-icon\"", &format!("\"{icon}\""))
    };
    let file = parse(&[line("/tmp/.org.chromium.Chromium.abc/avatar.png")]);
    assert_eq!(
        file[0].image,
        Some(NotificationIcon::Named(
            "/tmp/.org.chromium.Chromium.abc/avatar.png".into()
        ))
    );
    assert!(file[0].app_icons.is_empty());
    let named = parse(&[line("brave-browser")]);
    assert_eq!(named[0].image, None);
    assert_eq!(
        named[0].app_icons,
        vec![NotificationIcon::Named("brave-browser".into())]
    );
    // Long picture paths are kept whole.
    let long = format!("/home/u/{}/cover.jpg", "d".repeat(300));
    let hints = format!(r#""image-path":{{"type":"s","data":"{long}"}}"#);
    let found = parse(&[notify("Music", 0, "Song", "", &hints, -1)]);
    assert_eq!(found[0].image, Some(NotificationIcon::Named(long)));
}

#[test]
fn monitor_forwards_restarts_and_stops_on_drop() {
    // A stand-in for busctl that prints two notifications per run and
    // exits, so no session bus is involved. Each run numbers its texts,
    // since a repeat within seconds would be dropped as passed on.
    let dir = std::env::temp_dir().join(format!("nexus-notify-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let first = notify("first", 0, "run RUN", "", "", -1);
    let second = notify("second", 0, "run RUN", "b", "", -1);
    let script = dir.join("fake-busctl");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\ncd '{dir}'\nrun=$(( $(cat runs 2>/dev/null || echo 0) + 1 ))\necho $run > runs\nprintf '%s\\n%s\\n' '{first}' '{second}' | sed \"s/RUN/$run/\"\n",
            dir = dir.display(),
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let (monitor, rx) = NotificationMonitor::start_with(script).unwrap();
    let wait = Duration::from_secs(10);
    let seen: Vec<(String, String)> = (0..3)
        .map(|_| {
            let n = rx.recv_timeout(wait).expect("notification");
            (n.app_name, n.summary)
        })
        .collect();
    // The third one comes from the restarted child.
    assert_eq!(
        seen,
        vec![
            ("first".to_string(), "run 1".to_string()),
            ("second".to_string(), "run 1".to_string()),
            ("first".to_string(), "run 2".to_string()),
        ]
    );

    drop(monitor);
    let started = Instant::now();
    while rx.recv_timeout(Duration::from_millis(50)).is_ok() {}
    assert!(started.elapsed() < Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&dir);
}
