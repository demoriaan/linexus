use super::*;

const NOW: u64 = 1_795_093_380;
const MIN: u64 = 60;
const HOUR: u64 = 60 * MIN;

fn item(title: &str, published: Option<u64>) -> AlertItem {
    AlertItem {
        id: rss_item_key("news", title, None),
        guid: None,
        title: title.into(),
        link: Some(format!("https://example.test/{}", title.replace(' ', "-"))),
        published,
    }
}

/// The feed as downloaded at `fetched`.
fn fetch(items: Vec<AlertItem>, fetched: u64) -> AlertSource {
    AlertSource {
        id: "news".into(),
        url: "https://example.test/feed.xml".into(),
        name: "Example".into(),
        keywords: Vec::new(),
        opens: true,
        flagged: false,
        fetched,
        items,
    }
}

fn titles(fired: &[BreakingNews]) -> Vec<&str> {
    fired.iter().map(|news| news.title.as_str()).collect()
}

/// Observes `source` at the time it was downloaded.
fn observe(state: &mut AlertState, source: AlertSource) -> Vec<BreakingNews> {
    let now = source.fetched;
    state.observe(&[source], now).0
}

#[test]
fn the_first_fetch_is_a_baseline_and_only_new_recent_items_fire() {
    let mut state = AlertState::default();
    let old = item("Already there", Some(NOW - 5 * MIN));
    let (fired, changed) = state.observe(&[fetch(vec![old.clone()], NOW)], NOW);
    assert!(fired.is_empty(), "baseline fired {fired:?}");
    assert!(changed);
    // The same download again: nothing, and nothing to save either.
    let (fired, changed) = state.observe(&[fetch(vec![old.clone()], NOW)], NOW + 1);
    assert!(fired.is_empty());
    assert!(!changed);

    let at = NOW + 2 * MIN;
    let fresh = item("Fresh", Some(at - 10 * MIN));
    let stale = item("Too old to alert", Some(at - 31 * MIN));
    let undated = item("Undated", None);
    let future = item("Clock ahead", Some(at + 5 * MIN));
    let all = vec![
        fresh.clone(),
        stale.clone(),
        undated.clone(),
        future.clone(),
        old.clone(),
    ];
    let (fired, changed) = state.observe(&[fetch(all.clone(), at)], at);
    assert!(changed);
    assert_eq!(titles(&fired), ["Fresh", "Undated", "Clock ahead"]);
    assert_eq!(fired[0].feed, "news");
    assert_eq!(fired[0].source, "Example");
    assert_eq!(fired[0].id, fresh.id);
    assert_eq!(fired[0].link, fresh.link);
    assert_eq!(fired[0].url, fresh.link);
    // Each fires once.
    assert!(observe(&mut state, fetch(all, at + 2 * MIN)).is_empty());
}

#[test]
fn an_undated_item_does_not_fire_at_the_baseline() {
    let mut state = AlertState::default();
    assert!(observe(&mut state, fetch(vec![item("Undated", None)], NOW)).is_empty());
}

#[test]
fn a_new_address_or_a_new_feed_starts_with_a_baseline() {
    let mut state = AlertState::default();
    observe(&mut state, fetch(vec![], NOW));
    let mut moved = fetch(vec![item("Fresh", Some(NOW))], NOW + MIN);
    moved.url = "https://example.test/other.xml".into();
    assert!(observe(&mut state, moved.clone()).is_empty());
    let mut second = fetch(vec![item("Other feed", Some(NOW))], NOW + 2 * MIN);
    second.id = "second".into();
    let later = NOW + 2 * MIN;
    let (fired, _) = state.observe(
        &[
            AlertSource {
                items: vec![item("Fresh", Some(NOW)), item("Newer", Some(later))],
                fetched: later,
                ..moved
            },
            second,
        ],
        later,
    );
    assert_eq!(titles(&fired), ["Newer"]);
}

#[test]
fn an_empty_first_fetch_is_a_baseline_too() {
    let mut state = AlertState::default();
    observe(&mut state, fetch(vec![], NOW));
    let fired = observe(
        &mut state,
        fetch(vec![item("First", Some(NOW))], NOW + 2 * MIN),
    );
    assert_eq!(titles(&fired), ["First"]);
}

#[test]
fn a_corrected_title_with_the_same_guid_does_not_fire_again() {
    let mut state = AlertState::default();
    observe(&mut state, fetch(vec![], NOW));
    let first = AlertItem {
        guid: Some("story-1".into()),
        ..item("Fist version", Some(NOW))
    };
    assert_eq!(observe(&mut state, fetch(vec![first], NOW + MIN)).len(), 1);
    let corrected = AlertItem {
        guid: Some("story-1".into()),
        ..item("First version", Some(NOW))
    };
    assert!(observe(&mut state, fetch(vec![corrected], NOW + 2 * MIN)).is_empty());
}

#[test]
fn between_close_fetches_what_came_after_the_last_one_fires_beyond_half_an_hour() {
    // The previous download is 50 minutes old (a feed that backed off):
    // a story from 45 minutes ago is new since then, one from two hours ago
    // is not.
    let mut state = AlertState::default();
    observe(&mut state, fetch(vec![], NOW - 50 * MIN));
    let fired = observe(
        &mut state,
        fetch(
            vec![
                item("Since the last fetch", Some(NOW - 45 * MIN)),
                item("Long before it", Some(NOW - 2 * HOUR)),
                item("Undated", None),
            ],
            NOW,
        ),
    );
    assert_eq!(titles(&fired), ["Since the last fetch", "Undated"]);
}

#[test]
fn after_a_long_gap_only_the_last_half_hour_counts() {
    // Two hours without a download (strip off, offline): what appeared in
    // between is old news, an item without a date could be anything.
    let mut state = AlertState::default();
    observe(&mut state, fetch(vec![], NOW - 2 * HOUR));
    let fired = observe(
        &mut state,
        fetch(
            vec![
                item("Ten minutes ago", Some(NOW - 10 * MIN)),
                item("Forty minutes ago", Some(NOW - 40 * MIN)),
                item("Undated", None),
            ],
            NOW,
        ),
    );
    assert_eq!(titles(&fired), ["Ten minutes ago"]);
}

#[test]
fn a_download_looked_at_hours_later_only_counts_its_last_half_hour() {
    // Fetched two minutes after the previous one, but the daemon stopped
    // before looking at it and starts again five hours later.
    let mut state = AlertState::default();
    observe(&mut state, fetch(vec![], NOW - 2 * MIN));
    let late = NOW + 5 * HOUR;
    let (fired, _) = state.observe(
        &[fetch(
            vec![
                item("From that download", Some(NOW - MIN)),
                item("Undated", None),
            ],
            NOW,
        )],
        late,
    );
    assert!(fired.is_empty(), "{fired:?}");
}

#[test]
fn dates_without_a_zone_or_from_a_clock_behind_still_fire() {
    // A feed in New York writes local time without a zone, read as UTC:
    // every date is five hours behind. The newest item of the previous
    // download is the feed's own clock.
    let parse = |text: &str| crate::formats::parse_date(text);
    let real_now = parse("2026-11-19T13:03:00Z").unwrap();
    assert_eq!(real_now, NOW);
    let mut state = AlertState::default();
    let earlier = item("Earlier story", parse("2026-11-19T07:50:00"));
    observe(&mut state, fetch(vec![earlier.clone()], NOW - 2 * MIN));
    let fired = observe(
        &mut state,
        fetch(
            vec![
                item("Just now in New York", parse("2026-11-19T08:02:00")),
                item("Backdated", parse("2026-11-19T06:30:00")),
                earlier,
            ],
            NOW,
        ),
    );
    assert_eq!(titles(&fired), ["Just now in New York"]);
    // Dates ahead of our clock (a zone east of UTC read as UTC) are recent.
    let fired = observe(
        &mut state,
        fetch(
            vec![item("Paris, local time", parse("2026-11-19T14:05:00"))],
            NOW + 2 * MIN,
        ),
    );
    assert_eq!(titles(&fired), ["Paris, local time"]);
}

#[test]
fn nothing_hours_old_fires_whatever_the_feed_did_before() {
    let mut state = AlertState::default();
    observe(
        &mut state,
        fetch(vec![item("Days ago", Some(NOW - 72 * HOUR))], NOW - 2 * MIN),
    );
    let fired = observe(
        &mut state,
        fetch(
            vec![
                item("Seven hours", Some(NOW - 7 * HOUR)),
                item("Five hours", Some(NOW - 5 * HOUR)),
            ],
            NOW,
        ),
    );
    assert_eq!(titles(&fired), ["Five hours"]);
}

#[test]
fn a_feed_that_flags_breaking_news_fires_when_the_flag_appears() {
    // tagesschau marks a story that has been out for two hours as breaking:
    // the moment is when it first shows up flagged, not its date.
    let flagged = |items: Vec<AlertItem>, fetched: u64| AlertSource {
        flagged: true,
        ..fetch(items, fetched)
    };
    let mut state = AlertState::default();
    assert!(observe(
        &mut state,
        flagged(
            vec![item("At the baseline", Some(NOW - HOUR))],
            NOW - 2 * MIN
        )
    )
    .is_empty());
    let fired = observe(
        &mut state,
        flagged(
            vec![
                item("Two hours old, flagged now", Some(NOW - 2 * HOUR)),
                item("Seven hours old", Some(NOW - 7 * HOUR)),
            ],
            NOW,
        ),
    );
    assert_eq!(titles(&fired), ["Two hours old, flagged now"]);
    // After a long gap the flag could be hours old as well.
    let fired = observe(
        &mut state,
        flagged(
            vec![
                item("Flagged while we were away", Some(NOW)),
                item("Flagged just now", Some(NOW + 3 * HOUR)),
            ],
            NOW + 3 * HOUR,
        ),
    );
    assert_eq!(titles(&fired), ["Flagged just now"]);
}

#[test]
fn keywords_pick_what_fires_but_everything_counts_as_seen() {
    let mut state = AlertState::default();
    observe(&mut state, fetch(vec![], NOW));
    let mut feed = fetch(
        vec![
            item("Später Ausgleich für den 1. FC KÖLN", Some(NOW)),
            item("Wetter: Sonne satt", Some(NOW)),
            item("Bayerns Torwart verletzt", Some(NOW)),
        ],
        NOW + MIN,
    );
    feed.keywords = vec!["köln".into(), "Bayern*".into()];
    let fired = observe(&mut state, feed.clone());
    // Same time: the feed's order reversed, as feeds list newest first.
    assert_eq!(
        titles(&fired),
        [
            "Bayerns Torwart verletzt",
            "Später Ausgleich für den 1. FC KÖLN"
        ]
    );
    // Dropping the keywords later does not fire the skipped item.
    feed.keywords.clear();
    feed.fetched += MIN;
    assert!(observe(&mut state, feed).is_empty());
}

#[test]
fn keywords_match_whole_words_case_and_umlauts_included() {
    assert!(keyword_matches("Tor in der 90. Minute", "tor"));
    assert!(keyword_matches("Last-Minute-Tor!", "Tor"));
    assert!(!keyword_matches("Torwart patzt", "Tor"));
    assert!(!keyword_matches("Elfmetertor", "Tor"));
    assert!(keyword_matches("ÜBERRASCHUNG in München", "überraschung"));
    assert!(keyword_matches("Überraschung in MÜNCHEN", "münchen"));
    assert!(!keyword_matches("Muenchen", "münchen"));
    assert!(keyword_matches("STRAẞENSPERRE", "straßensperre"));
    assert!(keyword_matches(
        "Borussia   Dortmund siegt",
        "borussia dortmund"
    ));
    assert!(!keyword_matches(
        "Borussia Dortmunds Sieg",
        "Borussia Dortmund"
    ));
    assert!(keyword_matches(
        "Borussia Dortmunds Sieg",
        "Borussia Dortmund*"
    ));
    assert!(keyword_matches("L'Équipe de France", "équipe"));
    assert!(keyword_matches("Élection présidentielle", "ÉLECTION"));
    assert!(keyword_matches("Breaking: storm warning", "breaking:"));
    assert!(!keyword_matches("Anything", ""));
    assert!(!keyword_matches("Anything", "*"));
    assert!(!keyword_matches("", "word"));
}

#[test]
fn keywords_ignore_the_unicode_form_and_fold_case_fully() {
    // "Köln" with a combining diaeresis (NFD), as some feeds send it.
    let decomposed = "1. FC Ko\u{308}ln gewinnt";
    assert!(keyword_matches(decomposed, "Köln"));
    assert!(keyword_matches("1. FC Köln gewinnt", "Ko\u{308}ln"));
    assert!(keyword_matches(decomposed, "KÖLN"));
    // The mark belongs to the word: "Ko" is not a word of "Köln".
    assert!(!keyword_matches(decomposed, "Ko"));
    assert!(!keyword_matches("1. FC Köln gewinnt", "Ko"));
    assert!(!keyword_matches(decomposed, "Ko*"));
    // A combining mark without a composed form still belongs to its word.
    assert!(!keyword_matches("Aq\u{301}b ist da", "Aq"));
    assert!(keyword_matches("Aq\u{301}b ist da", "Aq*"));
    // ß and SS, İ and i.
    assert!(keyword_matches("STRASSE gesperrt", "Straße"));
    assert!(keyword_matches("Straße gesperrt", "strasse"));
    assert!(keyword_matches("İSTANBUL'da deprem", "istanbul"));
    assert!(keyword_matches("Istanbul earthquake", "İstanbul"));
    assert!(keyword_matches("ΣΕΙΣΜΌΣ στην Αθήνα", "σεισμός"));
}

#[test]
fn keywords_are_trimmed_deduplicated_and_capped() {
    let raw: Vec<String> = ["  Köln ", "KÖLN", "Ko\u{308}ln", "", "*", "Bayern*"]
        .into_iter()
        .map(String::from)
        .chain((0..30).map(|i| format!("word{i}")))
        .chain(["x".repeat(80)])
        .collect();
    let kept = normalize_keywords(&raw);
    assert_eq!(kept[..2], ["Köln".to_string(), "Bayern*".to_string()]);
    assert_eq!(kept.len(), MAX_KEYWORDS);
    assert_eq!(normalize_keywords(&["y".repeat(80)])[0].chars().count(), 60);
}

#[test]
fn the_same_story_is_known_by_its_address_or_its_title() {
    let news = |feed: &str, title: &str, url: Option<&str>| BreakingNews {
        feed: feed.into(),
        source: feed.to_uppercase(),
        id: title.into(),
        title: title.into(),
        url: url.map(String::from),
        link: None,
        published: None,
    };
    let story = news(
        "a",
        "Bridge closed after storm",
        Some("https://www.example.test/story/1/?utm_source=rss#top"),
    );
    // Another feed: the address without the trimmings, or the title.
    assert!(same_story(
        &story,
        &news(
            "b",
            "Bridge closed after the storm",
            Some("http://example.test/story/1")
        )
    ));
    assert!(same_story(
        &story,
        &news("b", "BRIDGE CLOSED – after storm!", None)
    ));
    assert!(!same_story(
        &story,
        &news("b", "Bridge reopened", Some("https://example.test/story/2"))
    ));
    assert!(!same_story(
        &news("a", "Live", Some("https://example.test/a?id=1")),
        &news("b", "Other", Some("https://example.test/a?id=2"))
    ));
    // The same feed: only the same address, so a corrected title is a
    // repeat but live-blog entries and repeated titles are not.
    assert!(same_story(
        &story,
        &news(
            "a",
            "Bridge closed after the storm",
            Some(" https://www.example.test/story/1/?utm_source=rss#top")
        )
    ));
    assert!(!same_story(
        &news(
            "a",
            "Explosion in harbour",
            Some("https://example.test/live#entry-1")
        ),
        &news(
            "a",
            "Government resigns",
            Some("https://example.test/live#entry-2")
        )
    ));
    assert!(!same_story(
        &news("a", "Goal for Bayern", Some("https://example.test/goal/1")),
        &news("a", "Goal for Bayern", Some("https://example.test/goal/2"))
    ));
    assert!(!same_story(
        &news("a", "Goal for Bayern", None),
        &news("a", "Goal for Bayern", None)
    ));
}

#[test]
fn state_survives_a_restart() {
    let dir = std::env::temp_dir().join(format!("linexus-alerts-{}", std::process::id()));
    let path = dir.join("state").join("alert-seen.json");
    let mut state = AlertState::default();
    observe(&mut state, fetch(vec![item("Old", Some(NOW))], NOW));
    state.save(&path).unwrap();
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let mut restarted = AlertState::load(&path);
    assert_eq!(restarted, state);
    let feed = fetch(
        vec![item("Old", Some(NOW)), item("New", Some(NOW + MIN))],
        NOW + 2 * MIN,
    );
    assert_eq!(titles(&observe(&mut restarted, feed)), ["New"]);
    // A broken file is a fresh start, which begins with a baseline.
    fs::write(&path, "{").unwrap();
    let mut broken = AlertState::load(&path);
    assert_eq!(broken, AlertState::default());
    assert!(observe(&mut broken, fetch(vec![item("Newest", Some(NOW))], NOW)).is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn forgotten_items_and_feeds_are_pruned() {
    let mut state = AlertState::default();
    observe(&mut state, fetch(vec![item("Old", Some(NOW))], NOW));
    let mut other = fetch(vec![], NOW + KEEP_SECS + 1);
    other.id = "other".into();
    let later = NOW + KEEP_SECS + 1;
    let (_, changed) = state.observe(&[other], later);
    assert!(changed);
    assert_eq!(state.sources.keys().collect::<Vec<_>>(), ["other"]);

    let mut seen: BTreeMap<String, u64> = (0..MAX_SEEN_KEYS as u64 + 10)
        .map(|i| (format!("k{i}"), NOW + i))
        .collect();
    assert!(prune(&mut seen, NOW + MAX_SEEN_KEYS as u64 + 10));
    assert_eq!(seen.len(), MAX_SEEN_KEYS);
    assert!(!seen.contains_key("k0"));
}

#[test]
fn alert_sources_come_from_alert_feed_records_only() {
    let alert_items = vec![item("One", Some(NOW))];
    let records = vec![
        SensorRecord {
            module: "rss".into(),
            kind: "rss_feed".into(),
            label: "Example".into(),
            value: serde_json::json!({
                "id": "news", "url": "https://example.test/feed.xml", "items": [],
                "alert": {"keywords": ["köln"], "opens": false, "flagged": true,
                          "fetched": NOW, "items": alert_items},
            }),
        },
        SensorRecord {
            module: "rss".into(),
            kind: "rss_feed".into(),
            label: "Plain".into(),
            value: serde_json::json!({"id": "plain", "url": "https://example.test/p", "items": []}),
        },
    ];
    let sources = alert_sources(&records);
    assert_eq!(sources.len(), 1);
    assert_eq!(
        sources[0],
        AlertSource {
            keywords: vec!["köln".into()],
            opens: false,
            flagged: true,
            ..fetch(vec![item("One", Some(NOW))], NOW)
        }
    );
}
