//! Breaking news: which new items of an alert feed deserve a banner.
//!
//! The daemon keeps an [`AlertState`] per feed and shows what
//! [`AlertState::observe`] returns. A feed's first fetch is a baseline: all
//! of its items count as seen and nothing fires, so a fresh start or a newly
//! added feed never floods the strip with old news. After that an item fires
//! once, when it is new (seen under none of its keys before), recent (see
//! [`AlertState::observe`]) and, when the feed has keywords, one of them
//! matches the title (see [`keyword_matches`]).

use super::{rss_item_key, write_private_file, SensorRecord};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

/// An item published this recently fires, whatever came before.
pub const ALERT_WINDOW_SECS: u64 = 30 * 60;
/// Alert feeds are fetched at least this often, so the window above and
/// the fetch-to-fetch rule below always overlap.
pub const ALERT_MAX_REFRESH_SECS: u64 = 15 * 60;
/// Two fetches at most this far apart follow each other: what is new in
/// the second one appeared in between.
const CONTINUITY_SECS: u64 = 60 * 60;
/// Slack for item dates a little before the previous fetch.
const GRACE_SECS: u64 = 5 * 60;
/// Nothing older than this is breaking news, whatever its feed says.
const MAX_AGE_SECS: u64 = 6 * 60 * 60;
/// Items of an alert feed checked per fetch, independent of how many the
/// news widget shows.
pub(crate) const ALERT_SCAN: usize = 50;
/// Keywords per feed, and characters per keyword.
const MAX_KEYWORDS: usize = 20;
const MAX_KEYWORD_CHARS: usize = 60;
/// Items and feeds not seen for this long are forgotten.
const KEEP_SECS: u64 = 14 * 86_400;
/// Seen times are renewed at most this often, so a feed that stays the same
/// does not rewrite the state file every second.
const RENEW_SECS: u64 = 86_400;
/// Remembered keys per feed: an item has one or two (with a guid), so this
/// keeps at least the last 1000 items.
const MAX_SEEN_KEYS: usize = 2_000;

/// One alert feed as the collector last read it.
#[derive(Debug, Clone, PartialEq)]
pub struct AlertSource {
    /// Feed id from sensors.json.
    pub id: String,
    pub url: String,
    /// What the news widget calls the feed (its title).
    pub name: String,
    pub keywords: Vec<String>,
    /// A tap opens the item (`tap_behavior` is not `none`).
    pub opens: bool,
    /// The feed marks breaking news itself (the tagesschau API): an item
    /// becomes breaking news when it first appears, whatever its date.
    pub flagged: bool,
    /// When these items were downloaded (unix seconds).
    pub fetched: u64,
    pub items: Vec<AlertItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertItem {
    /// The headline's card id (read and hidden state use it too).
    pub id: String,
    #[serde(default)]
    pub guid: Option<String>,
    pub title: String,
    #[serde(default)]
    pub link: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub published: Option<u64>,
}

/// An item that fired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BreakingNews {
    /// Feed id.
    pub feed: String,
    /// Feed name for the banner.
    pub source: String,
    /// Card id of the headline.
    pub id: String,
    pub title: String,
    /// The item's address.
    pub url: Option<String>,
    /// What a tap opens; None when the feed's taps open nothing.
    pub link: Option<String>,
    pub published: Option<u64>,
}

/// What the daemon remembers between fetches and restarts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertState {
    #[serde(default)]
    sources: BTreeMap<String, SourceState>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct SourceState {
    url: String,
    /// When the feed was last observed (renewed daily).
    #[serde(default)]
    checked: u64,
    /// When the last observed items were downloaded.
    #[serde(default)]
    fetched: u64,
    /// The newest item date of that download, by the feed's own clock.
    #[serde(default)]
    newest: Option<u64>,
    /// Item keys and when they were last seen.
    #[serde(default)]
    seen: BTreeMap<String, u64>,
}

impl AlertState {
    /// The stored state; a missing or unreadable file is a fresh start.
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        write_private_file(path, &serde_json::to_vec(self).unwrap_or_default())
    }

    /// Marks everything in `sources` seen and returns the items that fire,
    /// oldest first, plus whether the state changed (and should be saved).
    ///
    /// A new item is recent when it was published in the last 30 minutes
    /// (or ahead of `now`). When the download before this one is at most an
    /// hour older, it is also recent when it is dated after that download
    /// by our clock, or after the newest item it had by the feed's clock
    /// (a feed whose dates lack a zone or whose clock is behind), each with
    /// five minutes of slack; an item without a date, or one of a feed that
    /// marks breaking news itself, then counts as recent anyway. Nothing
    /// older than six hours fires.
    pub fn observe(&mut self, sources: &[AlertSource], now: u64) -> (Vec<BreakingNews>, bool) {
        let mut fired = Vec::new();
        let mut changed = false;
        for source in sources {
            // A new feed, or a new address under the same id, starts over.
            let baseline = self
                .sources
                .get(&source.id)
                .is_none_or(|state| state.url != source.url);
            let state = self.sources.entry(source.id.clone()).or_default();
            if baseline {
                *state = SourceState {
                    url: source.url.clone(),
                    checked: now,
                    ..SourceState::default()
                };
                changed = true;
            } else if now.saturating_sub(state.checked) >= RENEW_SECS {
                state.checked = now;
                changed = true;
            }
            // What is new is judged against the download before this one.
            let (previous, previous_newest) = (state.fetched, state.newest);
            if source.fetched > state.fetched || baseline {
                state.fetched = source.fetched;
                state.newest = source.items.iter().filter_map(|item| item.published).max();
                changed = true;
            }
            // A download looked at only much later (the daemon stopped right
            // after it) says nothing about now.
            let continuous = !baseline
                && previous > 0
                && source.fetched.saturating_sub(previous) <= CONTINUITY_SECS
                && now.saturating_sub(source.fetched) <= ALERT_WINDOW_SECS;
            for item in &source.items {
                let keys = item_keys(item);
                let new = keys.iter().all(|key| !state.seen.contains_key(key));
                for key in keys {
                    let at = state.seen.entry(key).or_insert(0);
                    if now.saturating_sub(*at) >= RENEW_SECS {
                        *at = now;
                        changed = true;
                    }
                }
                if !new || baseline {
                    continue;
                }
                let recent = match item.published {
                    Some(at) => {
                        let age = now.saturating_sub(at);
                        let after = |mark: u64| at.saturating_add(GRACE_SECS) >= mark;
                        age <= ALERT_WINDOW_SECS
                            || (continuous
                                && age <= MAX_AGE_SECS
                                && (source.flagged
                                    || after(previous)
                                    || previous_newest.is_some_and(after)))
                    }
                    None => continuous,
                };
                let wanted = source.keywords.is_empty()
                    || source
                        .keywords
                        .iter()
                        .any(|keyword| keyword_matches(&item.title, keyword));
                if recent && wanted {
                    fired.push(BreakingNews {
                        feed: source.id.clone(),
                        source: source.name.clone(),
                        id: item.id.clone(),
                        title: item.title.clone(),
                        url: item.link.clone(),
                        link: item.link.clone().filter(|_| source.opens),
                        published: item.published,
                    });
                }
            }
            changed |= prune(&mut state.seen, now);
        }
        let before = self.sources.len();
        self.sources
            .retain(|_, state| now.saturating_sub(state.checked) <= KEEP_SECS);
        changed |= self.sources.len() != before;
        // Feeds list the newest first; the banner queue wants the oldest.
        fired.reverse();
        fired.sort_by_key(|news| news.published.unwrap_or(now));
        (fired, changed)
    }
}

/// An item counts as seen under its card id (title and link) and its guid,
/// so neither a corrected title under the same guid nor a feed that changes
/// guids fires twice.
fn item_keys(item: &AlertItem) -> Vec<String> {
    let mut keys = vec![item.id.clone()];
    if let Some(guid) = item.guid.as_deref().filter(|guid| !guid.trim().is_empty()) {
        keys.push(rss_item_key("guid", guid.trim(), None));
    }
    keys
}

fn prune(seen: &mut BTreeMap<String, u64>, now: u64) -> bool {
    let before = seen.len();
    seen.retain(|_, at| now.saturating_sub(*at) <= KEEP_SECS);
    if seen.len() > MAX_SEEN_KEYS {
        let mut times: Vec<u64> = seen.values().copied().collect();
        times.sort_unstable();
        let cutoff = times[seen.len() - MAX_SEEN_KEYS];
        seen.retain(|_, at| *at >= cutoff);
    }
    seen.len() != before
}

/// Whether two fired items are the same story. The address counts without
/// scheme, `www.`, `utm_` parameters and trailing slash. Within one feed
/// only the address does, fragment included (a title corrected under its
/// link): entries of a live blog share a page and differ in the fragment,
/// and a sports feed can bring "Tor für Bayern" twice for two goals.
/// Between feeds the fragment does not count, and the same title (letters
/// and digits only, see [`keyword_matches`] for case) is the same story
/// too.
pub fn same_story(a: &BreakingNews, b: &BreakingNews) -> bool {
    let same_feed = a.feed == b.feed;
    let link = |news: &BreakingNews| news.url.as_deref().and_then(|url| link_key(url, same_feed));
    let same_link = link(a).is_some_and(|key| link(b) == Some(key));
    same_link
        || !same_feed && {
            let title = title_key(&a.title);
            !title.is_empty() && title == title_key(&b.title)
        }
}

/// `url` without scheme, `www.`, `utm_` parameters and trailing slash, and
/// without the fragment unless `fragment` is set.
fn link_key(url: &str, fragment: bool) -> Option<String> {
    let url = url.trim();
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let (rest, after_hash) = rest.split_once('#').unwrap_or((rest, ""));
    let (place, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (host, path) = place.split_once('/').unwrap_or((place, ""));
    let host = host.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    let query: Vec<&str> = query
        .split('&')
        .filter(|param| !param.is_empty() && !param.to_ascii_lowercase().starts_with("utm_"))
        .collect();
    let mut key = format!("{host}/{}", path.trim_end_matches('/'));
    if !query.is_empty() {
        key.push('?');
        key.push_str(&query.join("&"));
    }
    if fragment && !after_hash.is_empty() {
        key.push('#');
        key.push_str(after_hash);
    }
    (!host.is_empty()).then_some(key)
}

fn title_key(title: &str) -> String {
    let mut key = String::new();
    for ch in fold(title) {
        if is_word_char(ch) {
            key.push(ch);
        } else if !key.is_empty() && !key.ends_with(' ') {
            key.push(' ');
        }
    }
    key.trim_end().to_string()
}

/// Whether `keyword` occurs in `title` as a whole word or phrase. Case does
/// not matter (`KÖLN` matches `Köln`, `STRASSE` matches `Straße`, `İ` and
/// `ı` count as `i`), neither does the Unicode form (a `ö` made of `o` and
/// a combining mark is a `ö`), runs of white space count as one, and the
/// characters next to the match must not be letters, digits or combining
/// marks: `Tor` matches "Tor in der 90. Minute" and "Last-Minute-Tor", but
/// not "Torwart", and `Ko` does not match "Köln". A trailing `*` also allows
/// longer words: `Bayern*` matches "Bayerns". Accents and umlauts must be
/// written as in the headline (`ü` is not `ue`).
pub fn keyword_matches(title: &str, keyword: &str) -> bool {
    let keyword = keyword.trim();
    let (keyword, prefix) = match keyword.strip_suffix('*') {
        Some(stem) => (stem, true),
        None => (keyword, false),
    };
    let needle = fold(keyword);
    let hay = fold(title);
    if needle.is_empty() || needle.len() > hay.len() {
        return false;
    }
    let word = |ch: Option<&char>| ch.is_some_and(|ch| is_word_char(*ch));
    let starts_word = word(needle.first());
    let ends_word = word(needle.last());
    (0..=hay.len() - needle.len()).any(|start| {
        let end = start + needle.len();
        hay[start..end] == needle[..]
            && !(starts_word && start > 0 && word(hay.get(start - 1)))
            && (prefix || !(ends_word && word(hay.get(end))))
    })
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || is_combining_mark(ch)
}

/// Composed (NFC), case-folded, white space runs as one space, trimmed.
fn fold(text: &str) -> Vec<char> {
    let mut out = Vec::new();
    let mut space = false;
    for ch in text.nfc() {
        if ch.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        match ch {
            'ß' | 'ẞ' => out.extend(['s', 's']),
            // Turkish dotted and dotless i, read without the dot question.
            'İ' | 'ı' => out.push('i'),
            'ς' => out.push('σ'),
            _ => out.extend(ch.to_lowercase()),
        }
    }
    out
}

/// Keywords as stored: trimmed, without empty ones and case-insensitive
/// repeats, at most 20 of at most 60 characters.
pub(crate) fn normalize_keywords(keywords: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for keyword in keywords {
        let keyword: String = keyword.trim().chars().take(MAX_KEYWORD_CHARS).collect();
        let keyword = keyword.trim();
        if keyword.is_empty() || keyword == "*" {
            continue;
        }
        if out.iter().all(|kept| fold(kept) != fold(keyword)) {
            out.push(keyword.to_string());
        }
        if out.len() == MAX_KEYWORDS {
            break;
        }
    }
    out
}

/// The alert feeds among the collected records.
pub fn alert_sources(records: &[SensorRecord]) -> Vec<AlertSource> {
    records
        .iter()
        .filter(|record| record.module == "rss" && record.kind == "rss_feed")
        .filter_map(|record| {
            let alert = record.value.get("alert")?;
            let text = |value: &serde_json::Value, key: &str| {
                value.get(key).and_then(|v| v.as_str()).map(str::to_string)
            };
            let flag = |key: &str, default: bool| {
                alert.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
            };
            Some(AlertSource {
                id: text(&record.value, "id")?,
                url: text(&record.value, "url")?,
                name: record.label.clone(),
                keywords: serde_json::from_value(alert.get("keywords")?.clone()).ok()?,
                opens: flag("opens", true),
                flagged: flag("flagged", false),
                fetched: alert.get("fetched").and_then(|v| v.as_u64()).unwrap_or(0),
                items: serde_json::from_value(alert.get("items")?.clone()).ok()?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
