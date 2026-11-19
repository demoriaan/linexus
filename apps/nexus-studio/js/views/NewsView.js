// News: the feeds behind news widgets and how headlines run on the strip.
// Edits save on their own through /api/sensors/news, which replaces only
// feeds and headline pacing.
import { html, useEffect, useRef, useState } from "../lib/html.js";
import { api } from "../lib/api.js";
import { lang, t } from "../lib/i18n.js";
import { Icon } from "../lib/icons.js";
import { sensors, sensorsError, loadSensors, saveNews } from "../lib/sensors.js";
import { toast, uniqueId } from "../lib/store.js";
import { ConfirmButton, Switch } from "../components/Dialog.js";

// Feed addresses checked against the live sites, by language. `alert`
// sources mark breaking news themselves (the tagesschau API is not RSS);
// `refresh` follows what the site allows.
const SUGGESTIONS = {
  de: [
    { name: "tagesschau Eilmeldungen", url: "https://www.tagesschau.de/api2u/homepage/", alert: true, refresh: 120 },
    { name: "SPIEGEL Eilmeldungen", url: "https://www.spiegel.de/schlagzeilen/eilmeldungen/index.rss", alert: true, refresh: 300 },
    { name: "tagesschau", url: "https://www.tagesschau.de/index~rss2.xml" },
    { name: "kicker", url: "https://newsfeed.kicker.de/news/aktuell", perDay: 80 },
    { name: "sportschau", url: "https://www.sportschau.de/index~rss2.xml" },
    { name: "SPIEGEL", url: "https://www.spiegel.de/schlagzeilen/index.rss" },
    { name: "ZEIT", url: "https://newsfeed.zeit.de/index" },
    { name: "FAZ", url: "https://www.faz.net/rss/aktuell/" },
    { name: "Süddeutsche", url: "https://rss.sueddeutsche.de/rss/Topthemen" },
    { name: "n-tv", url: "https://www.n-tv.de/rss" },
    { name: "Deutschlandfunk", url: "https://www.deutschlandfunk.de/nachrichten-100.rss" },
    { name: "heise", url: "https://www.heise.de/rss/heise-atom.xml" },
    { name: "golem", url: "https://rss.golem.de/rss.php?feed=RSS2.0" },
    { name: "t3n", url: "https://t3n.de/rss.xml" },
    { name: "ComputerBase", url: "https://www.computerbase.de/rss/news.xml" },
    { name: "netzpolitik.org", url: "https://netzpolitik.org/feed/" },
  ],
  en: [
    { name: "BBC News", url: "https://feeds.bbci.co.uk/news/rss.xml" },
    { name: "Sky News", url: "https://feeds.skynews.com/feeds/rss/home.xml" },
    { name: "The Guardian World", url: "https://www.theguardian.com/world/rss" },
    { name: "BBC Sport", url: "https://feeds.bbci.co.uk/sport/rss.xml" },
    { name: "Sky Sports", url: "https://www.skysports.com/rss/12040" },
    { name: "The Verge", url: "https://www.theverge.com/rss/index.xml" },
    { name: "Ars Technica", url: "https://feeds.arstechnica.com/arstechnica/index" },
    { name: "Hacker News", url: "https://hnrss.org/frontpage" },
    { name: "Phoronix", url: "https://www.phoronix.com/rss.php" },
  ],
  fr: [
    { name: "franceinfo Les titres", url: "https://www.francetvinfo.fr/titres.rss" },
    { name: "Le Monde À la une", url: "https://www.lemonde.fr/rss/une.xml" },
    { name: "France 24", url: "https://www.france24.com/fr/rss" },
    { name: "franceinfo Sports", url: "https://www.francetvinfo.fr/sports.rss" },
    { name: "L'Équipe", url: "https://dwh.lequipe.fr/api/edito/rss?path=/", perDay: 130 },
  ],
};
// Each group is headed by its language's own name.
const LANGUAGES = { de: "Deutsch", en: "English", fr: "Français" };
// More breaking news a day than this, without keywords, gets a warning.
const BUSY_PER_DAY = 20;

const DISPLAY_DEFAULTS = { scroll_speed_px_per_sec: 50, start_pause_ms: 2000, end_pause_ms: 1500, min_pair_ms: 5000, hold_to_dismiss_ms: 600 };
const seconds = (ms) => `${(ms / 1000).toFixed(ms % 1000 ? 2 : 1)} s`;
const DISPLAY_FIELDS = [
  ["scroll_speed_px_per_sec", 20, 150, 5, (v) => t("news.speed.value", { px: v, chars: (v / 8.5).toFixed(1) })],
  ["start_pause_ms", 0, 6000, 250, seconds],
  ["end_pause_ms", 0, 6000, 250, seconds],
  ["min_pair_ms", 2000, 30000, 500, seconds],
  ["hold_to_dismiss_ms", 300, 2000, 50, (v) => `${(v / 1000).toFixed(2)} s`],
];
const REFRESH_MINUTES = [1, 2, 5, 10, 15, 30, 60, 120, 240];
// The daemon checks breaking-news feeds at least this often.
const ALERT_MAX_REFRESH_MINUTES = 15;

const host = (url) => {
  try {
    return new URL(url).host.replace(/^www\./, "");
  } catch {
    return url;
  }
};
const normalize = (address) => {
  const a = address.trim();
  if (!a) return "";
  return /^https?:\/\//i.test(a) ? a : `https://${a}`;
};

export function NewsView() {
  const [rss, setRss] = useState(null);
  const [display, setDisplay] = useState(DISPLAY_DEFAULTS);
  const [state, setState] = useState("saved");
  const timer = useRef(null);
  const latest = useRef({ rss: null, display: null });

  useEffect(() => {
    loadSensors().then((data) => {
      if (!data) return;
      setRss(data.config.rss ?? []);
      setDisplay({ ...DISPLAY_DEFAULTS, ...(data.config.display ?? {}) });
    });
    return () => {
      if (timer.current) {
        clearTimeout(timer.current);
        saveNews(latest.current.rss, latest.current.display).catch(() => {});
      }
    };
  }, []);

  const commit = (nextRss, nextDisplay) => {
    setRss(nextRss);
    setDisplay(nextDisplay);
    latest.current = { rss: nextRss, display: nextDisplay };
    setState("pending");
    clearTimeout(timer.current);
    timer.current = setTimeout(async () => {
      timer.current = null;
      setState("saving");
      try {
        await saveNews(nextRss, nextDisplay);
        setState("saved");
      } catch (error) {
        setState("error");
        toast(error.message, "error");
      }
    }, 500);
  };

  if (sensorsError.value && !rss) {
    return html`<div class="view"><div class="banner"><${Icon} name="alert-triangle" /><div class="grow">${t("news.config.broken")}<br /><span class="hint">${sensorsError.value}</span></div></div></div>`;
  }
  if (!rss) return html`<div class="view"><div class="empty"><${Icon} name="loader" />${t("common.loading")}</div></div>`;

  const titles = sensors.value?.feed_titles ?? {};
  const rates = sensors.value?.feed_rates ?? {};
  const setFeed = (i, patch) => commit(rss.map((f, j) => (j === i ? { ...f, ...patch } : f)), display);
  const moveFeed = (i, d) => {
    const next = [...rss];
    const [item] = next.splice(i, 1);
    next.splice(i + d, 0, item);
    commit(next, display);
  };

  return html`
    <div class="view">
      <div class="view-head">
        <div>
          <h1>${t("nav.news")}</h1>
          <p>${t("news.lead")}</p>
        </div>
        <div class="spacer"></div>
        <span class="hint">${t(`save.${state}`)}</span>
      </div>
      <div class="cols news-cols">
        <div class="stack" style=${{ gap: "18px" }}>
          <${AddSource} rss=${rss} onAdd=${(feed) => commit([...rss, feed], display)} />
          <section class="panel">
            <header><h3>${t("news.sources")}</h3><span class="hint">${t("news.sources.count", { n: rss.length })}</span></header>
            ${!rss.length && html`<div class="empty"><${Icon} name="newspaper" />${t("news.sources.empty")}</div>`}
            ${rss.map((feed, i) => html`
              <${FeedRow} key=${feed.id} feed=${feed} title=${titles[feed.id] || host(feed.url)} rate=${rates[feed.id]} first=${i === 0} last=${i === rss.length - 1}
                onChange=${(patch) => setFeed(i, patch)} onMove=${(d) => moveFeed(i, d)} onRemove=${() => commit(rss.filter((_, j) => j !== i), display)} />`)}
          </section>
        </div>
        <div class="stack" style=${{ gap: "18px" }}>
          <section class="panel">
            <header><h3>${t("news.display")}</h3>
              <button class="btn small ghost" onClick=${() => commit(rss, DISPLAY_DEFAULTS)}>${t("common.reset")}</button></header>
            <div class="stack">
              ${DISPLAY_FIELDS.map(([key, min, max, step, format]) => html`
                <label class="field">
                  <span class="row"><span class="grow">${t(`news.display.${key}`)}</span><span class="num muted">${format(display[key])}</span></span>
                  <input type="range" min=${min} max=${max} step=${step} value=${display[key]}
                    onInput=${(e) => commit(rss, { ...display, [key]: Number(e.currentTarget.value) })} />
                </label>`)}
            </div>
          </section>
          <${ReadState} />
          <section class="panel">
            <header><h3>${t("news.gestures")}</h3></header>
            <ul class="headlines">
              <li>${t("news.gesture.tap")}</li>
              <li>${t("news.gesture.double")}</li>
              <li>${t("news.gesture.hold")}</li>
              <li>${t("news.gesture.alert")}</li>
            </ul>
          </section>
        </div>
      </div>
    </div>`;
}

function AddSource({ rss, onAdd }) {
  const [address, setAddress] = useState("");
  const [result, setResult] = useState(null);
  const seq = useRef(0);
  const check = async (raw, preset = {}) => {
    const url = normalize(raw);
    if (!url) return;
    const mine = ++seq.current;
    setResult({ loading: true, url });
    try {
      const data = await api.post("/api/sensors/rss/inspect", { url });
      if (mine === seq.current) setResult({ feed: data.feed, preset });
    } catch (error) {
      if (mine === seq.current) setResult({ error: error.body?.error || error.message });
    }
  };
  const add = (feed, preset) => {
    const id = uniqueId(feed.suggested_id || "feed", new Set(rss.map((f) => f.id)));
    // The tagesschau API brings only breaking news and allows a request a
    // minute; two minutes keep well within that.
    const api2u = feed.format === "tagesschau";
    onAdd({
      id, url: feed.feed_url, limit: 3, refresh_seconds: preset.refresh ?? (api2u ? 120 : 900), show_mode: "unread_first", priority: 0, tap_behavior: "open_url",
      ...(api2u ? { format: "tagesschau" } : {}), alert: preset.alert ?? api2u, alert_keywords: [],
    });
    setResult({ added: feed.title || host(feed.feed_url) });
    setAddress("");
  };
  const exists = (url) => rss.some((f) => f.url === url);
  // The UI language's sources first; all of them stay visible.
  const groups = [lang.value, ...Object.keys(SUGGESTIONS).filter((code) => code !== lang.value)].filter((code) => SUGGESTIONS[code]);
  return html`
    <section class="panel">
      <header><h3>${t("news.add")}</h3></header>
      <form class="row" onSubmit=${(e) => { e.preventDefault(); check(address); }}>
        <input type="text" class="grow" placeholder=${t("news.add.placeholder")} value=${address} onInput=${(e) => setAddress(e.currentTarget.value)} />
        <button class="btn primary" type="submit">${t("news.add.find")}</button>
      </form>
      <p class="hint" style=${{ margin: "10px 0 8px" }}>${t("news.add.suggestions")}</p>
      ${groups.map((code) => html`
        <div class="stack" style=${{ gap: "6px", marginBottom: "10px" }}>
          <span class="hint"><b>${LANGUAGES[code]}</b></span>
          <div class="suggestions">
            ${SUGGESTIONS[code].map((s) => html`
              <button class="btn small" disabled=${exists(s.url)} onClick=${() => check(s.url, s)}
                title=${s.alert ? t("news.suggest.alert") : s.perDay ? t("news.suggest.busy", { n: s.perDay }) : undefined}>
                ${s.alert && html`<${Icon} name="siren" size=${14} />`}${s.name}
              </button>`)}
          </div>
          ${code !== "de" && html`<small class="hint">${t(`news.suggest.hint.${code}`)}</small>`}
        </div>`)}
      ${result && html`<div class="gesture" style=${{ marginTop: "14px" }}>
        ${result.loading && html`<span class="row tight muted"><${Icon} name="loader" size=${15} />${t("news.add.checking", { url: result.url })}</span>`}
        ${result.error && html`<span class="bad-text">${result.error}</span>`}
        ${result.added && html`<span class="ok-text">${t("news.add.added", { name: result.added })}</span>`}
        ${result.feed && html`
          <div class="stack" style=${{ gap: "8px" }}>
            <div><b>${result.feed.title || host(result.feed.feed_url)}</b><br /><span class="hint">${result.feed.feed_url}</span></div>
            ${result.feed.items_per_day != null && html`<span class="hint">${t("news.rate", { n: result.feed.items_per_day })}</span>`}
            ${!(result.feed.headlines ?? []).length && html`<span class="hint">${t("news.add.quiet")}</span>`}
            <ol class="headlines">${(result.feed.headlines ?? []).map((h) => html`<li>${h}</li>`)}</ol>
            ${(result.feed.discovered ?? []).filter((u) => u !== result.feed.feed_url).length > 0 && html`
              <div class="row tight hint">${t("news.add.more")}
                ${result.feed.discovered.filter((u) => u !== result.feed.feed_url).map((u) => html`<button class="btn small ghost" onClick=${() => check(u)}>${host(u)}${new URL(u).pathname}</button>`)}
              </div>`}
            <div><button class="btn primary" disabled=${exists(result.feed.feed_url)} onClick=${() => add(result.feed, result.preset ?? {})}>
              ${exists(result.feed.feed_url) ? t("news.add.exists") : t("news.add.confirm")}
            </button></div>
          </div>`}
      </div>`}
    </section>`;
}

function FeedRow({ feed, title, rate, first, last, onChange, onMove, onRemove }) {
  const [check, setCheck] = useState(null);
  // Typed text stays as it is; the feed gets the cleaned-up list.
  const [keywords, setKeywords] = useState((feed.alert_keywords ?? []).join(", "));
  const enabled = feed.show_mode !== "hidden";
  const perDay = check?.feed?.items_per_day ?? rate;
  const busy = feed.alert && !(feed.alert_keywords ?? []).length && perDay > BUSY_PER_DAY;
  const longest = feed.alert ? ALERT_MAX_REFRESH_MINUTES : Infinity;
  const minutes = Math.min(Math.round(feed.refresh_seconds / 60), longest);
  const test = async () => {
    setCheck({ loading: true });
    try {
      const data = await api.post("/api/sensors/rss/inspect", { url: feed.url });
      setCheck({ feed: data.feed });
    } catch (error) {
      setCheck({ error: error.body?.error || error.message });
    }
  };
  return html`
    <div class="feed">
      <div class="row tight" style=${{ flexDirection: "column" }}>
        <button class="icon-btn" disabled=${first} onClick=${() => onMove(-1)} aria-label=${t("news.feed.up")}><${Icon} name="chevron-up" size=${16} /></button>
        <button class="icon-btn" disabled=${last} onClick=${() => onMove(1)} aria-label=${t("news.feed.down")}><${Icon} name="chevron-down" size=${16} /></button>
      </div>
      <div style=${{ minWidth: 0 }}>
        <div class="title">${title}</div>
        <div class="url">${feed.url}</div>
      </div>
      <div class="row tight">
        <${Switch} checked=${enabled} onChange=${(on) => onChange({ show_mode: on ? "unread_first" : "hidden" })} />
        <button class="btn small" onClick=${test}>${t("news.feed.test")}</button>
        <${ConfirmButton} class="btn small ghost danger" title=${t("common.delete")} onConfirm=${onRemove}><${Icon} name="trash-2" size=${15} /><//>
      </div>
      <div class="settings">
        <label>${t("news.feed.limit")}
          <select value=${feed.limit} onChange=${(e) => onChange({ limit: Number(e.currentTarget.value) })}>
            ${[...new Set([1, 2, 3, 5, 8, 10, 15, 20, feed.limit])].sort((a, b) => a - b).map((n) => html`<option value=${n}>${n}</option>`)}
          </select>
        </label>
        <label>${t("news.feed.refresh")}
          <select value=${minutes} onChange=${(e) => onChange({ refresh_seconds: Number(e.currentTarget.value) * 60 })}>
            ${[...new Set([...REFRESH_MINUTES.filter((m) => m <= longest), minutes])].sort((a, b) => a - b).map((m) => html`<option value=${m}>${t("news.minutes", { n: m })}</option>`)}
          </select>
        </label>
        <label>${t("news.feed.order")}
          <select value=${feed.priority} onChange=${(e) => onChange({ priority: Number(e.currentTarget.value) })}>
            ${[[10, "first"], [0, "normal"], [-10, "last"]].map(([v, k]) => html`<option value=${v}>${t(`news.priority.${k}`)}</option>`)}
          </select>
        </label>
        <label>${t("news.feed.tap")}
          <select value=${feed.tap_behavior} onChange=${(e) => onChange({ tap_behavior: e.currentTarget.value })}>
            <option value="open_url">${t("news.tap.open")}</option>
            <option value="none">${t("news.tap.none")}</option>
          </select>
        </label>
      </div>
      <div class="settings alert">
        <${Switch} label=${t("news.feed.alert")} checked=${!!feed.alert}
          onChange=${(alert) => onChange(alert && feed.refresh_seconds > ALERT_MAX_REFRESH_MINUTES * 60
            ? { alert, refresh_seconds: ALERT_MAX_REFRESH_MINUTES * 60 }
            : { alert })} />
        ${feed.alert && html`
          <label class="grow">${t("news.feed.keywords")}
            <input type="text" value=${keywords} placeholder=${t("news.feed.keywords.placeholder")}
              onInput=${(e) => {
                setKeywords(e.currentTarget.value);
                onChange({ alert_keywords: e.currentTarget.value.split(",").map((k) => k.trim()).filter(Boolean) });
              }} />
          </label>`}
        ${perDay != null && html`<span class="hint">${t("news.rate", { n: perDay })}</span>`}
        ${feed.alert && html`<small class="hint">${(feed.alert_keywords ?? []).length ? t("news.feed.keywords.hint") : t("news.feed.alert.hint")}</small>`}
        ${busy && html`<small class="warn-text">${t("news.feed.alert.busy", { n: perDay })}</small>`}
      </div>
      ${check && html`<div class="settings" style=${{ display: "block" }}>
        ${check.loading && html`<span class="muted">${t("common.loading")}</span>`}
        ${check.error && html`<span class="bad-text">${check.error}</span>`}
        ${check.feed && !(check.feed.headlines ?? []).length && html`<span class="hint">${t("news.add.quiet")}</span>`}
        ${check.feed && html`<ol class="headlines">${(check.feed.headlines ?? []).map((h) => html`<li>${h}</li>`)}</ol>`}
      </div>`}
    </div>`;
}

function ReadState() {
  const seen = sensors.value?.rss_seen_count ?? 0;
  const run = async (path, done) => {
    try {
      await api.post(path, {});
      await loadSensors();
      toast(done, "ok");
    } catch (error) {
      toast(error.message, "error");
    }
  };
  return html`
    <section class="panel">
      <header><h3>${t("news.read")}</h3></header>
      <p class="hint" style=${{ marginBottom: "10px" }}>${t("news.read.count", { n: seen })}</p>
      <div class="row">
        <button class="btn small" onClick=${() => run("/api/sensors/rss/mark-read", t("news.read.marked"))}>${t("news.read.mark")}</button>
        <button class="btn small ghost" onClick=${() => run("/api/sensors/rss/reset-read", t("news.read.reset.done"))}>${t("news.read.reset")}</button>
      </div>
    </section>`;
}
