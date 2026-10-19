//! Feed inspection for the Studio "add feed" flow: download an address, accept
//! it when it is an RSS/Atom feed, otherwise look for feeds the page links via
//! `<link rel="alternate">`, and report title plus the latest headlines.

use super::{
    fetch_document, parse_rss_channel, rss_cache_path, sanitize_sensor_id, xml_unescape,
    RssChannel, RssFeedConfig,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::time::Duration;

const INSPECT_TIMEOUT: Duration = Duration::from_secs(8);
const INSPECT_HEADLINES: usize = 5;
/// Pages sometimes link a dozen feeds (per category, comments, podcasts);
/// the first few are the main ones.
const MAX_DISCOVERED: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeedInspection {
    /// The feed address to store: `url` itself, or the feed a page linked.
    pub feed_url: String,
    pub title: Option<String>,
    pub suggested_id: String,
    pub headlines: Vec<String>,
    /// Feed links found when `url` was a web page rather than a feed.
    pub discovered: Vec<String>,
}

/// Downloads `url` and describes the feed behind it, or explains in plain
/// words why there is none.
pub fn inspect_feed_url(url: &str) -> Result<FeedInspection, String> {
    let url = normalize_scheme(url.trim())
        .ok_or("Enter an address starting with http:// or https://.")?;
    let (body, page_url) = fetch_document(&url, INSPECT_TIMEOUT)
        .ok_or_else(|| format!("Could not download {url}."))?;
    if let Some(channel) = parse_feed(&body) {
        return Ok(describe(&url, channel, Vec::new()));
    }
    let discovered = discover_feed_links(&body, &page_url);
    if discovered.is_empty() {
        return Err(
            "This page does not link an RSS or Atom feed. Look for an RSS link on the site \
             and paste that address instead."
                .into(),
        );
    }
    for candidate in &discovered {
        let Some((body, _)) = fetch_document(candidate, INSPECT_TIMEOUT) else {
            continue;
        };
        if let Some(channel) = parse_feed(&body) {
            return Ok(describe(candidate, channel, discovered.clone()));
        }
    }
    Err("The page links feeds, but none of them could be read.".into())
}

/// Feed titles from the local cache, keyed by feed id, so the feed list can
/// show names without downloading anything.
pub fn cached_feed_titles(feeds: &[RssFeedConfig]) -> BTreeMap<String, String> {
    feeds
        .iter()
        .filter_map(|feed| {
            let text = fs::read_to_string(rss_cache_path(feed)).ok()?;
            let value = serde_json::from_str::<serde_json::Value>(&text).ok()?;
            let body = value.get("body")?.as_str()?;
            let title = parse_rss_channel(body, 0)?.title?;
            Some((feed.id.clone(), title))
        })
        .collect()
}

/// A feed only counts when it has entries: the RSS parser also accepts an
/// HTML page's `<title>` as a channel title.
fn parse_feed(body: &str) -> Option<RssChannel> {
    parse_rss_channel(body, INSPECT_HEADLINES).filter(|channel| !channel.items.is_empty())
}

fn describe(url: &str, channel: RssChannel, discovered: Vec<String>) -> FeedInspection {
    let title = channel
        .title
        .map(|title| title.trim().to_string())
        .filter(|title| !title.is_empty());
    let suggested_id = sanitize_sensor_id(site_name(url));
    FeedInspection {
        feed_url: url.to_string(),
        title,
        suggested_id,
        headlines: channel
            .items
            .into_iter()
            .filter_map(|item| item.title)
            .map(|title| title.trim().to_string())
            .filter(|title| !title.is_empty())
            .collect(),
        discovered,
    }
}

/// `url` with a lower-case `http`/`https` scheme, or `None` for anything else.
fn normalize_scheme(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    matches!(scheme.as_str(), "http" | "https").then(|| format!("{scheme}://{rest}"))
}

fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    host.strip_prefix("www.").unwrap_or(host)
}

/// Short, stable name for a feed id: the site's own label without feed
/// subdomains or TLD (`rss.golem.de` → `golem`). Feed titles make poor ids —
/// long, and they change.
fn site_name(url: &str) -> &str {
    let labels: Vec<&str> = host(url).split('.').collect();
    let meaningful: Vec<&str> = labels
        .iter()
        .copied()
        .filter(|label| {
            !matches!(
                *label,
                "www" | "rss" | "feed" | "feeds" | "newsfeed" | "rssfeed"
            )
        })
        .collect();
    match meaningful.as_slice() {
        [] => host(url),
        [only] => only,
        [.., name, _tld] => name,
    }
}

/// `href`s of `<link rel="alternate" type="application/rss+xml|atom+xml">`
/// tags, resolved against `base`, in page order without duplicates.
fn discover_feed_links(html: &str, base: &str) -> Vec<String> {
    // ASCII lower-casing keeps byte offsets, so indices found in `lower`
    // are valid (ASCII) char boundaries in `html`.
    let lower = html.to_ascii_lowercase();
    let mut links = Vec::new();
    let mut from = 0;
    while let Some(offset) = lower[from..].find("<link") {
        let start = from + offset + "<link".len();
        let Some(len) = lower[start..].find('>') else {
            break;
        };
        from = start + len;
        let attrs = attributes(&html[start..start + len]);
        let get = |name: &str| {
            attrs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| *value)
        };
        let rel = get("rel").unwrap_or_default().to_ascii_lowercase();
        let kind = get("type").unwrap_or_default().to_ascii_lowercase();
        if !rel.split_whitespace().any(|rel| rel == "alternate")
            || !(kind.contains("rss+xml") || kind.contains("atom+xml"))
        {
            continue;
        }
        let Some(href) = get("href") else {
            continue;
        };
        if let Some(url) = resolve_url(base, &xml_unescape(href.trim())) {
            if !links.contains(&url) {
                links.push(url);
            }
        }
        if links.len() == MAX_DISCOVERED {
            break;
        }
    }
    links
}

/// Attributes of an HTML start tag body (the text between the tag name and
/// `>`), as `(lower-case name, raw value)`. Values may be double-quoted,
/// single-quoted or unquoted (minified pages: `type=application/rss+xml`).
fn attributes(tag: &str) -> Vec<(String, &str)> {
    let mut attrs = Vec::new();
    let mut rest = tag;
    loop {
        rest = rest.trim_start_matches(|ch: char| ch.is_whitespace() || ch == '/');
        if rest.is_empty() {
            break;
        }
        let name_end = rest
            .find(|ch: char| ch.is_whitespace() || ch == '=' || ch == '/')
            .unwrap_or(rest.len());
        let name = rest[..name_end].to_ascii_lowercase();
        rest = rest[name_end..].trim_start();
        let Some(after_eq) = rest.strip_prefix('=') else {
            attrs.push((name, ""));
            continue;
        };
        rest = after_eq.trim_start();
        let value = match rest.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                let body = &rest[1..];
                let end = body.find(quote).unwrap_or(body.len());
                rest = body.get(end + 1..).unwrap_or("");
                &body[..end]
            }
            _ => {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                let value = &rest[..end];
                rest = &rest[end..];
                value
            }
        };
        attrs.push((name, value));
    }
    attrs
}

fn resolve_url(base: &str, href: &str) -> Option<String> {
    if let Some(absolute) = normalize_scheme(href) {
        return Some(absolute);
    }
    let (scheme, rest) = base.split_once("://")?;
    if let Some(net_path) = href.strip_prefix("//") {
        return Some(format!("{scheme}://{net_path}"));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if href.starts_with('/') {
        return Some(format!("{scheme}://{authority}{href}"));
    }
    let path = rest[authority.len()..]
        .split(['?', '#'])
        .next()
        .unwrap_or("");
    if href.starts_with('?') || href.starts_with('#') {
        return Some(format!("{scheme}://{authority}{path}{href}"));
    }
    let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
    Some(format!("{scheme}://{authority}{dir}/{href}"))
}

/// Decodes a downloaded document by its declared charset: Windows-1252 (the
/// superset of ISO-8859-1 that older feeds such as Golem still declare) when
/// the XML declaration or a meta tag names a Latin-1 family charset,
/// otherwise UTF-8 — lossily, so one stray byte only costs that character.
pub(crate) fn decode_text(bytes: &[u8]) -> String {
    if declares_latin1(bytes) {
        bytes.iter().map(|&byte| windows_1252(byte)).collect()
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn declares_latin1(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_ascii_lowercase();
    let Some(at) = head.find("encoding=").or_else(|| head.find("charset=")) else {
        return false;
    };
    let value = head[at..]
        .split_once('=')
        .map_or("", |(_, value)| value)
        .trim_start_matches(['"', '\'', ' ']);
    let charset: String = value
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect();
    matches!(
        charset.as_str(),
        "iso-8859-1"
            | "iso8859-1"
            | "iso-8859-15"
            | "latin1"
            | "latin-1"
            | "windows-1252"
            | "cp1252"
    )
}

fn windows_1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    match byte {
        0x80..=0x9f => HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_alternate_feed_links_in_any_attribute_order() {
        let html = r#"<html><head>
            <link rel="stylesheet" href="/s.css">
            <LINK type="application/rss+xml" title="Alle" href="/index~rss2.xml" rel="alternate" />
            <link rel='alternate' type='application/atom+xml' href='https://cdn.example.org/atom.xml'>
            <link rel="alternate" type="application/rss+xml" href="/index~rss2.xml">
            <link rel="alternate" hreflang="en" href="/en/">
            <link title="Kommentare type=1" rel=alternate type=application/rss+xml href=/comments/feed/>
        </head></html>"#;
        assert_eq!(
            discover_feed_links(html, "https://www.example.de/inland/"),
            vec![
                "https://www.example.de/index~rss2.xml".to_string(),
                "https://cdn.example.org/atom.xml".to_string(),
                "https://www.example.de/comments/feed/".to_string(),
            ]
        );
    }

    #[test]
    fn resolves_relative_feed_links() {
        let base = "https://example.com/news/today.html?x=1";
        assert_eq!(
            resolve_url(base, "feed.xml").as_deref(),
            Some("https://example.com/news/feed.xml")
        );
        assert_eq!(
            resolve_url(base, "/rss").as_deref(),
            Some("https://example.com/rss")
        );
        assert_eq!(
            resolve_url(base, "//cdn.example.com/a.rss").as_deref(),
            Some("https://cdn.example.com/a.rss")
        );
        assert_eq!(
            resolve_url("https://example.com", "rss.xml").as_deref(),
            Some("https://example.com/rss.xml")
        );
        assert_eq!(
            resolve_url("https://blog.example/index.php?p=1", "?feed=rss2").as_deref(),
            Some("https://blog.example/index.php?feed=rss2")
        );
        assert_eq!(
            resolve_url(base, "HTTPS://Example.com/Feed").as_deref(),
            Some("https://Example.com/Feed")
        );
        assert_eq!(
            normalize_scheme("HTTPS://x.de/a").as_deref(),
            Some("https://x.de/a")
        );
        assert_eq!(normalize_scheme("ftp://x.de/a"), None);
    }

    #[test]
    fn html_pages_are_not_mistaken_for_feeds() {
        assert!(parse_feed("<html><head><title>Startseite</title></head></html>").is_none());
        let rss =
            "<rss><channel><title>News</title><item><title>Eins</title></item></channel></rss>";
        let channel = parse_feed(rss).unwrap();
        let inspection = describe("https://www.example.com/rss", channel, Vec::new());
        assert_eq!(inspection.title.as_deref(), Some("News"));
        assert_eq!(inspection.suggested_id, "example");
        assert_eq!(inspection.headlines, vec!["Eins".to_string()]);
    }

    #[test]
    fn decodes_by_declared_charset() {
        assert_eq!(decode_text("Grüße „neu“".as_bytes()), "Grüße „neu“");
        let mut latin = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><t>".to_vec();
        latin.extend([
            b'G', 0xfc, b'n', b't', b'e', b'r', b' ', 0x84, b'x', 0x93, b' ', 0x96,
        ]);
        assert!(decode_text(&latin).ends_with("<t>Günter „x“ –"));
        // A UTF-8 document with one stray byte keeps its other umlauts.
        let mut stray = "<?xml version=\"1.0\" encoding=\"utf-8\"?>Grüße "
            .as_bytes()
            .to_vec();
        stray.push(0xff);
        assert!(decode_text(&stray).contains("Grüße"));
    }

    #[test]
    fn suggests_short_ids_from_the_site_name() {
        assert_eq!(host("https://www.heise.de/rss/heise-atom.xml"), "heise.de");
        assert_eq!(host("http://example.org?x"), "example.org");
        assert_eq!(
            site_name("https://rss.golem.de/rss.php?feed=RSS2.0"),
            "golem"
        );
        assert_eq!(site_name("https://newsfeed.zeit.de/index"), "zeit");
        assert_eq!(
            site_name("https://www.tagesschau.de/index~atom.xml"),
            "tagesschau"
        );
        assert_eq!(site_name("http://localhost:8080/feed"), "localhost:8080");
    }
}
