// News: the feeds behind news widgets and how headlines run on the strip.
// Edits save on their own through /api/sensors/news, which replaces only
// feeds and headline pacing.
import { html, useEffect, useRef, useState } from "../lib/html.js";
import { api } from "../lib/api.js";
import { t } from "../lib/i18n.js";
import { Icon } from "../lib/icons.js";
import { sensors, sensorsError, loadSensors, saveNews } from "../lib/sensors.js";
import { toast, uniqueId } from "../lib/store.js";
import { ConfirmButton, Switch } from "../components/Dialog.js";

// Feed addresses checked against the live sites.
const SUGGESTIONS = [
  ["tagesschau", "https://www.tagesschau.de/index~rss2.xml"],
  ["SPIEGEL", "https://www.spiegel.de/schlagzeilen/index.rss"],
  ["ZEIT", "https://newsfeed.zeit.de/index"],
  ["FAZ", "https://www.faz.net/rss/aktuell/"],
  ["Süddeutsche", "https://rss.sueddeutsche.de/rss/Topthemen"],
  ["n-tv", "https://www.n-tv.de/rss"],
  ["Deutschlandfunk", "https://www.deutschlandfunk.de/nachrichten-100.rss"],
  ["heise", "https://www.heise.de/rss/heise-atom.xml"],
  ["golem", "https://rss.golem.de/rss.php?feed=RSS2.0"],
  ["t3n", "https://t3n.de/rss.xml"],
  ["ComputerBase", "https://www.computerbase.de/rss/news.xml"],
  ["netzpolitik.org", "https://netzpolitik.org/feed/"],
  ["BBC News", "https://feeds.bbci.co.uk/news/rss.xml"],
  ["The Verge", "https://www.theverge.com/rss/index.xml"],
  ["Ars Technica", "https://feeds.arstechnica.com/arstechnica/index"],
  ["Hacker News", "https://hnrss.org/frontpage"],
  ["Phoronix", "https://www.phoronix.com/rss.php"],
];

const DISPLAY_DEFAULTS = { scroll_speed_px_per_sec: 50, start_pause_ms: 2000, end_pause_ms: 1500, min_pair_ms: 5000, hold_to_dismiss_ms: 600 };
const seconds = (ms) => `${(ms / 1000).toFixed(ms % 1000 ? 2 : 1)} s`;
const DISPLAY_FIELDS = [
  ["scroll_speed_px_per_sec", 20, 150, 5, (v) => t("news.speed.value", { px: v, chars: (v / 8.5).toFixed(1) })],
  ["start_pause_ms", 0, 6000, 250, seconds],
  ["end_pause_ms", 0, 6000, 250, seconds],
  ["min_pair_ms", 2000, 30000, 500, seconds],
  ["hold_to_dismiss_ms", 300, 2000, 50, (v) => `${(v / 1000).toFixed(2)} s`],
];
const REFRESH_MINUTES = [5, 10, 15, 30, 60, 120, 240];

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
              <${FeedRow} key=${feed.id} feed=${feed} title=${titles[feed.id] || host(feed.url)} first=${i === 0} last=${i === rss.length - 1}
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
  const check = async (raw) => {
    const url = normalize(raw);
    if (!url) return;
    const mine = ++seq.current;
    setResult({ loading: true, url });
    try {
      const data = await api.post("/api/sensors/rss/inspect", { url });
      if (mine === seq.current) setResult({ feed: data.feed });
    } catch (error) {
      if (mine === seq.current) setResult({ error: error.body?.error || error.message });
    }
  };
  const add = (feed) => {
    const id = uniqueId(feed.suggested_id || "feed", new Set(rss.map((f) => f.id)));
    onAdd({ id, url: feed.feed_url, limit: 3, refresh_seconds: 900, show_mode: "unread_first", priority: 0, tap_behavior: "open_url" });
    setResult({ added: feed.title || host(feed.feed_url) });
    setAddress("");
  };
  const exists = (url) => rss.some((f) => f.url === url);
  return html`
    <section class="panel">
      <header><h3>${t("news.add")}</h3></header>
      <form class="row" onSubmit=${(e) => { e.preventDefault(); check(address); }}>
        <input type="text" class="grow" placeholder=${t("news.add.placeholder")} value=${address} onInput=${(e) => setAddress(e.currentTarget.value)} />
        <button class="btn primary" type="submit">${t("news.add.find")}</button>
      </form>
      <p class="hint" style=${{ margin: "10px 0 8px" }}>${t("news.add.suggestions")}</p>
      <div class="suggestions">
        ${SUGGESTIONS.map(([name, url]) => html`
          <button class="btn small" disabled=${exists(url)} onClick=${() => check(url)}>${name}</button>`)}
      </div>
      ${result && html`<div class="gesture" style=${{ marginTop: "14px" }}>
        ${result.loading && html`<span class="row tight muted"><${Icon} name="loader" size=${15} />${t("news.add.checking", { url: result.url })}</span>`}
        ${result.error && html`<span class="bad-text">${result.error}</span>`}
        ${result.added && html`<span class="ok-text">${t("news.add.added", { name: result.added })}</span>`}
        ${result.feed && html`
          <div class="stack" style=${{ gap: "8px" }}>
            <div><b>${result.feed.title || host(result.feed.feed_url)}</b><br /><span class="hint">${result.feed.feed_url}</span></div>
            <ol class="headlines">${(result.feed.headlines ?? []).map((h) => html`<li>${h}</li>`)}</ol>
            ${(result.feed.discovered ?? []).filter((u) => u !== result.feed.feed_url).length > 0 && html`
              <div class="row tight hint">${t("news.add.more")}
                ${result.feed.discovered.filter((u) => u !== result.feed.feed_url).map((u) => html`<button class="btn small ghost" onClick=${() => check(u)}>${host(u)}${new URL(u).pathname}</button>`)}
              </div>`}
            <div><button class="btn primary" disabled=${exists(result.feed.feed_url)} onClick=${() => add(result.feed)}>
              ${exists(result.feed.feed_url) ? t("news.add.exists") : t("news.add.confirm")}
            </button></div>
          </div>`}
      </div>`}
    </section>`;
}

function FeedRow({ feed, title, first, last, onChange, onMove, onRemove }) {
  const [check, setCheck] = useState(null);
  const enabled = feed.show_mode !== "hidden";
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
          <select value=${Math.round(feed.refresh_seconds / 60)} onChange=${(e) => onChange({ refresh_seconds: Number(e.currentTarget.value) * 60 })}>
            ${REFRESH_MINUTES.map((m) => html`<option value=${m}>${t("news.minutes", { n: m })}</option>`)}
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
      ${check && html`<div class="settings" style=${{ display: "block" }}>
        ${check.loading && html`<span class="muted">${t("common.loading")}</span>`}
        ${check.error && html`<span class="bad-text">${check.error}</span>`}
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
