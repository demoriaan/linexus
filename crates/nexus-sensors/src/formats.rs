//! Item dates, the one feed format besides RSS and Atom (the tagesschau
//! JSON API), and how many items a feed brings a day.

use super::{RssChannel, RssItem};
use serde::{Deserialize, Deserializer, Serialize};

/// What a feed address returns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedFormat {
    /// RSS or Atom.
    #[default]
    Rss,
    /// The tagesschau JSON API (`/api2u/homepage/`): only the items it
    /// marks as breaking news count.
    Tagesschau,
}

/// An unknown format reads as RSS, so one typo does not make the whole
/// sensors.json unreadable.
impl<'de> Deserialize<'de> for FeedFormat {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(match name.trim().to_ascii_lowercase().as_str() {
            "tagesschau" => Self::Tagesschau,
            _ => Self::Rss,
        })
    }
}

impl FeedFormat {
    pub fn is_rss(&self) -> bool {
        *self == Self::Rss
    }
}

/// Unix seconds of a feed date: RFC 822/2822 as RSS writes it
/// (`Tue, 19 Nov 2026 14:03:00 +0100`) or ISO 8601 as Atom and JSON APIs do
/// (`2026-11-19T14:03:00+01:00`). A date without a zone counts as UTC.
pub(crate) fn parse_date(text: &str) -> Option<u64> {
    let text = text.trim();
    let iso = text.len() >= 10
        && text.as_bytes()[..4].iter().all(u8::is_ascii_digit)
        && text.as_bytes()[4] == b'-';
    if iso {
        parse_iso8601(text)
    } else {
        parse_rfc2822(text)
    }
}

fn parse_iso8601(text: &str) -> Option<u64> {
    let year: i64 = text.get(0..4)?.parse().ok()?;
    let month: u32 = text.get(5..7)?.parse().ok()?;
    let day: u32 = text.get(8..10)?.parse().ok()?;
    let rest = &text[10..];
    let Some(rest) = rest.strip_prefix(['T', 't', ' ']) else {
        return unix(year, month, day, (0, 0, 0), 0);
    };
    let time_end = rest
        .find(|ch: char| !(ch.is_ascii_digit() || ch == ':' || ch == '.' || ch == ','))
        .unwrap_or(rest.len());
    let (time, zone) = rest.split_at(time_end);
    let time = time.split(['.', ',']).next().unwrap_or(time);
    let mut parts = time.split(':');
    let hour = parts.next()?.parse().ok()?;
    let minute = parts.next()?.parse().ok()?;
    let second = parts.next().map_or(Some(0), |s| s.parse().ok())?;
    unix(year, month, day, (hour, minute, second), zone_offset(zone)?)
}

fn parse_rfc2822(text: &str) -> Option<u64> {
    let (mut day, mut month, mut year, mut time, mut offset) = (None, None, None, None, 0);
    for token in text
        .split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|token| !token.is_empty())
    {
        if token.contains(':') && time.is_none() {
            let mut parts = token.split(':');
            let hour = parts.next()?.parse().ok()?;
            let minute = parts.next()?.parse().ok()?;
            let second = parts.next().map_or(Some(0), |s| s.parse().ok())?;
            time = Some((hour, minute, second));
        } else if let Some(number) = token
            .parse::<i64>()
            .ok()
            .filter(|_| token.bytes().all(|b| b.is_ascii_digit()))
        {
            if day.is_none() && month.is_none() && token.len() <= 2 {
                day = Some(number as u32);
            } else if year.is_none() {
                // RFC 822 allowed two-digit years.
                year = Some(match (token.len(), number) {
                    (2, 0..=69) => 2000 + number,
                    (2, _) => 1900 + number,
                    _ => number,
                });
            }
        } else if let Some(m) = month_number(token).filter(|_| month.is_none()) {
            month = Some(m);
        } else if time.is_some() {
            offset = zone_offset(token).unwrap_or(0);
        }
    }
    unix(year?, month?, day?, time.unwrap_or((0, 0, 0)), offset)
}

fn month_number(token: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let lower = token.get(..3)?.to_ascii_lowercase();
    MONTHS
        .iter()
        .position(|name| *name == lower)
        .map(|i| i as u32 + 1)
}

/// Seconds east of UTC: `Z`, `+01:00`, `+0100`, `+01`, or a zone name.
fn zone_offset(zone: &str) -> Option<i64> {
    let zone = zone.trim();
    if let Some(sign) = zone.chars().next().filter(|ch| matches!(ch, '+' | '-')) {
        let digits: String = zone[1..].chars().filter(char::is_ascii_digit).collect();
        let (hours, minutes) = match digits.len() {
            2 => (digits.parse::<i64>().ok()?, 0),
            4 => (digits[..2].parse().ok()?, digits[2..].parse().ok()?),
            _ => return None,
        };
        let seconds = hours * 3600 + minutes * 60;
        return Some(if sign == '-' { -seconds } else { seconds });
    }
    let hours = match zone.to_ascii_uppercase().as_str() {
        "" | "Z" | "UT" | "UTC" | "GMT" => 0,
        "EDT" => -4,
        "EST" | "CDT" => -5,
        "CST" | "MDT" => -6,
        "MST" | "PDT" => -7,
        "PST" => -8,
        "CET" | "MEZ" => 1,
        "CEST" | "MESZ" => 2,
        _ => return None,
    };
    Some(hours * 3600)
}

fn unix(year: i64, month: u32, day: u32, time: (u32, u32, u32), offset: i64) -> Option<u64> {
    let (hour, minute, second) = time;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    let seconds = days_from_civil(year, month, day) * 86_400
        + i64::from(hour) * 3600
        + i64::from(minute) * 60
        + i64::from(second.min(60))
        - offset;
    u64::try_from(seconds).ok()
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_from_march = i64::from((month + 9) % 12);
    let day_of_year = (153 * month_from_march + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Whether `body` is an RSS, Atom or RSS 1.0 document: after the XML
/// declaration, processing instructions, comments and a doctype, its root
/// element is `rss`, `feed` or `rdf:RDF` (with any prefix). True for a feed
/// without items too; false for an HTML page.
pub(crate) fn is_feed_document(body: &str) -> bool {
    let mut rest = body.trim_start_matches('\u{feff}').trim_start();
    loop {
        let skipped = if let Some(after) = rest.strip_prefix("<?") {
            after.split_once("?>").map(|(_, rest)| rest)
        } else if let Some(after) = rest.strip_prefix("<!--") {
            after.split_once("-->").map(|(_, rest)| rest)
        } else if let Some(after) = rest.strip_prefix("<!") {
            // A doctype, with or without an internal subset in brackets.
            let close = after.find('>');
            match after.find('[') {
                Some(open) if close.is_none_or(|close| open < close) => after[open..]
                    .split_once("]")
                    .and_then(|(_, r)| r.split_once('>')),
                _ => after.split_once('>'),
            }
            .map(|(_, rest)| rest)
        } else {
            break;
        };
        let Some(after) = skipped else {
            return false;
        };
        rest = after.trim_start();
    }
    let Some(tag) = rest.strip_prefix('<') else {
        return false;
    };
    let name = tag
        .split(|ch: char| ch.is_whitespace() || ch == '>' || ch == '/')
        .next()
        .unwrap_or_default();
    let local = name.rsplit(':').next().unwrap_or(name);
    ["rss", "feed", "rdf"]
        .iter()
        .any(|root| local.eq_ignore_ascii_case(root))
}

/// The breaking news on the tagesschau homepage API: `{"news": [...]}` with
/// a `breakingNews` flag per item. A page without breaking news is a channel
/// without items; None when the body is not that API's answer.
pub(crate) fn parse_tagesschau(body: &str, limit: usize) -> Option<RssChannel> {
    #[derive(Deserialize)]
    struct Page {
        news: Vec<serde_json::Value>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Item {
        title: Option<String>,
        date: Option<String>,
        #[serde(rename = "shareURL")]
        share_url: Option<String>,
        detailsweb: Option<String>,
        breaking_news: Option<bool>,
        first_sentence: Option<String>,
        sophora_id: Option<String>,
        external_id: Option<String>,
    }
    let page: Page = serde_json::from_str(body.trim()).ok()?;
    let web = |url: Option<String>| {
        url.map(|url| url.trim().to_string())
            .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
    };
    let items = page
        .news
        .into_iter()
        .filter_map(|value| serde_json::from_value::<Item>(value).ok())
        .filter(|item| item.breaking_news == Some(true))
        .filter_map(|item| {
            let title = item.title?.trim().to_string();
            (!title.is_empty()).then(|| RssItem {
                published: item.date.as_deref().and_then(parse_date),
                link: web(item.share_url).or_else(|| web(item.detailsweb)),
                guid: item.sophora_id.or(item.external_id),
                summary: item.first_sentence,
                title: Some(title),
                ..RssItem::default()
            })
        })
        .take(limit)
        .collect();
    Some(RssChannel {
        title: Some("tagesschau".into()),
        items,
    })
}

/// About how many items a feed brings a day, from its item dates: those of
/// the last 24 hours when the feed reaches back that far, otherwise the rate
/// over the time it covers (at least an hour). None with fewer than two
/// dated items.
pub fn items_per_day(dates: &[u64], now: u64) -> Option<f32> {
    if dates.len() < 2 {
        return None;
    }
    let oldest = *dates.iter().min()?;
    let newest = *dates.iter().max()?;
    let day = 86_400;
    if now.saturating_sub(oldest) >= day {
        let recent = dates.iter().filter(|&&at| now.saturating_sub(at) < day);
        return Some(recent.count() as f32);
    }
    let span = newest.saturating_sub(oldest).max(3600);
    Some((dates.len() - 1) as f32 * day as f32 / span as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-11-19 13:03:00 UTC.
    const T: u64 = 1_795_093_380;

    #[test]
    fn parses_rss_and_iso_dates_with_their_zones() {
        assert_eq!(parse_date("Thu, 19 Nov 2026 14:03:00 +0100"), Some(T));
        assert_eq!(parse_date("Thu, 19 Nov 2026 13:03:00 GMT"), Some(T));
        assert_eq!(parse_date("19 Nov 2026 08:03:00 EST"), Some(T));
        assert_eq!(parse_date("Thu, 19 Nov 26 14:03 CET"), Some(T));
        assert_eq!(parse_date("2026-11-19T14:03:00+01:00"), Some(T));
        assert_eq!(parse_date("2026-11-19T14:03:00.250+0100"), Some(T));
        assert_eq!(parse_date("2026-11-19T13:03:00Z"), Some(T));
        assert_eq!(parse_date("2026-11-19 13:03:00"), Some(T));
        assert_eq!(parse_date("2026-11-19"), Some(T - 13 * 3600 - 180));
        assert_eq!(parse_date("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_date("2024-02-29T12:00:00Z"), Some(1_709_208_000));
        for broken in ["", "yesterday", "2026-13-01T00:00:00Z", "Thu, 32 Nov 2026"] {
            assert_eq!(parse_date(broken), None, "{broken}");
        }
    }

    #[test]
    fn unknown_formats_read_as_rss() {
        let format = |json: &str| serde_json::from_str::<FeedFormat>(json).unwrap();
        assert_eq!(format("\"tagesschau\""), FeedFormat::Tagesschau);
        assert_eq!(format("\"Tagesschau\""), FeedFormat::Tagesschau);
        assert_eq!(format("\"atom\""), FeedFormat::Rss);
        assert_eq!(
            serde_json::to_string(&FeedFormat::Tagesschau).unwrap(),
            "\"tagesschau\""
        );
    }

    /// Shaped like the documented API answer; the texts are made up.
    const TAGESSCHAU: &str = r#"{
      "news": [
        {"sophoraId": "beispiel-100", "externalId": "tagesschau_fm-ts-1",
         "title": "Beispielmeldung ohne Eile", "date": "2026-11-19T13:40:00.000+01:00",
         "shareURL": "https://www.tagesschau.de/inland/beispiel-100.html", "breakingNews": false},
        {"sophoraId": "eilmeldung-200", "title": "Erste erfundene Eilmeldung",
         "date": "2026-11-19T14:03:00.000+01:00", "firstSentence": "Ein Satz.",
         "shareURL": "https://www.tagesschau.de/eilmeldung/eilmeldung-200.html",
         "detailsweb": "https://www.tagesschau.de/eilmeldung/eilmeldung-200.html", "breakingNews": true},
        {"externalId": "tagesschau_fm-ts-3", "title": "Zweite erfundene Eilmeldung",
         "detailsweb": "https://www.tagesschau.de/ausland/beispiel-300.html", "breakingNews": true},
        {"title": "  ", "breakingNews": true},
        {"title": "Ohne Flag"},
        "kein Objekt",
        {"sophoraId": "regional-400", "title": "Regionales", "breakingNews": null}
      ],
      "regional": [], "newStoriesCountLink": "https://example.test"
    }"#;

    #[test]
    fn tagesschau_keeps_only_breaking_news() {
        let channel = parse_tagesschau(TAGESSCHAU, 50).unwrap();
        assert_eq!(channel.title.as_deref(), Some("tagesschau"));
        let titles: Vec<_> = channel
            .items
            .iter()
            .map(|item| item.title.as_deref().unwrap())
            .collect();
        assert_eq!(
            titles,
            ["Erste erfundene Eilmeldung", "Zweite erfundene Eilmeldung"]
        );
        let first = &channel.items[0];
        assert_eq!(first.published, Some(T));
        assert_eq!(first.guid.as_deref(), Some("eilmeldung-200"));
        assert_eq!(
            first.link.as_deref(),
            Some("https://www.tagesschau.de/eilmeldung/eilmeldung-200.html")
        );
        assert_eq!(first.summary.as_deref(), Some("Ein Satz."));
        let second = &channel.items[1];
        assert_eq!(second.published, None);
        assert_eq!(second.guid.as_deref(), Some("tagesschau_fm-ts-3"));
        assert_eq!(
            second.link.as_deref(),
            Some("https://www.tagesschau.de/ausland/beispiel-300.html")
        );
        assert_eq!(parse_tagesschau(TAGESSCHAU, 1).unwrap().items.len(), 1);
        // A quiet day is an answer without items, not a failure.
        let quiet = parse_tagesschau(
            r#"{"news": [{"title": "Ruhig", "breakingNews": false}]}"#,
            5,
        );
        assert_eq!(quiet.map(|c| c.items.len()), Some(0));
        assert!(parse_tagesschau("<rss></rss>", 5).is_none());
        assert!(parse_tagesschau(r#"{"items": []}"#, 5).is_none());
    }

    #[test]
    fn feed_documents_are_known_by_their_root_even_without_items() {
        let empty_rss = r#"<?xml version="1.0" encoding="UTF-8"?>
            <?xml-stylesheet type="text/xsl" href="/rss.xsl"?>
            <!-- generated -->
            <rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom"><channel>
            <title>Beispiel Eilmeldungen</title>
            <atom:link href="https://example.test/eil.rss" rel="self" type="application/rss+xml"/>
            </channel></rss>"#;
        assert!(is_feed_document(empty_rss));
        assert!(is_feed_document(
            "\u{feff}<feed xmlns=\"http://www.w3.org/2005/Atom\"/>"
        ));
        assert!(is_feed_document(
            "<!DOCTYPE rss [<!ENTITY nbsp \"&#160;\">]><rss><channel/></rss>"
        ));
        assert!(is_feed_document("<rdf:RDF xmlns:rdf=\"x\"></rdf:RDF>"));
        assert!(is_feed_document("<atom:feed></atom:feed>"));
        for page in [
            "<!DOCTYPE html><html><head><title>Hotel WLAN</title></head></html>",
            "<html><title>Login</title></html>",
            "{\"news\": []}",
            "",
            "<?xml version=\"1.0\"",
            "<rssfeed/>",
        ] {
            assert!(!is_feed_document(page), "{page}");
        }
    }

    #[test]
    fn estimates_items_per_day() {
        let hour = 3600;
        assert_eq!(items_per_day(&[], T), None);
        assert_eq!(items_per_day(&[T], T), None);
        // A feed reaching back two days: the items of the last 24 hours.
        let week: Vec<u64> = (0..12).map(|i| T - i * 4 * hour).collect();
        assert_eq!(items_per_day(&week, T), Some(6.0));
        // 41 items over the last 10 hours: about 96 a day.
        let busy: Vec<u64> = (0..41).map(|i| T - i * 15 * 60).collect();
        assert_eq!(items_per_day(&busy, T), Some(96.0));
        // Two items a minute apart count over at least an hour.
        assert_eq!(items_per_day(&[T, T - 60], T), Some(24.0));
    }
}
