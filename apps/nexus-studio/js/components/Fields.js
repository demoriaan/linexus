// Inputs used by the inspector: plain fields plus pickers for icons,
// applications, metrics, places, Home Assistant entities and images.
import { html, useEffect, useMemo, useState } from "../lib/html.js";
import { api } from "../lib/api.js";
import { t, lang } from "../lib/i18n.js";
import { Icon, IconRef, catalog, loadCatalog } from "../lib/icons.js";
import { metricsCatalog, refreshMetrics, toast } from "../lib/store.js";
import { Dialog, Switch } from "./Dialog.js";

/**
 * Labelled form row. `group` renders a <div role="group"> instead of a
 * <label>, for pickers made of buttons (a label would forward every click
 * on its text to the first button inside).
 */
export function Field({ label, hint, children, invalid, group = false }) {
  const Tag = group ? "div" : "label";
  return html`
    <${Tag} class=${`field ${invalid ? "invalid" : ""}`} role=${group ? "group" : undefined} aria-label=${group ? label : undefined}>
      <span>${label}</span>
      ${children}
      ${hint && html`<small>${hint}</small>`}
    <//>`;
}

export function TextInput({ value, onInput, placeholder, type = "text", optional }) {
  return html`<input
    type=${type}
    value=${value ?? ""}
    placeholder=${placeholder ?? (optional ? t("common.optional") : "")}
    onInput=${(e) => onInput(e.currentTarget.value)}
  />`;
}

export function NumberInput({ value, onInput, min, max, step = 1, optional }) {
  return html`<input
    type="number"
    value=${value ?? ""}
    min=${min}
    max=${max}
    step=${step}
    placeholder=${optional ? t("common.optional") : ""}
    onInput=${(e) => {
      const raw = e.currentTarget.value;
      if (raw === "") return onInput(optional ? null : min ?? 0);
      const n = Number(raw);
      if (Number.isFinite(n)) onInput(n);
    }}
  />`;
}

export function Select({ value, options, onChange, labelOf = (o) => o }) {
  return html`
    <select value=${value ?? ""} onChange=${(e) => onChange(e.currentTarget.value)}>
      ${options.map((o) => html`<option value=${o}>${labelOf(o)}</option>`)}
    </select>`;
}

// ---- icons -----------------------------------------------------------------

export function IconField({ value, onChange, optional }) {
  const [open, setOpen] = useState(false);
  return html`
    <div class="row tight">
      <span class="btn small" style=${{ pointerEvents: "none", minWidth: "40px", justifyContent: "center" }}>
        ${value ? html`<${IconRef} value=${value} size=${18} />` : html`<span class="muted">—</span>`}
      </span>
      <button class="btn small" onClick=${() => setOpen(true)}>${t("icon.choose")}</button>
      ${optional && value && html`<button class="btn small ghost" onClick=${() => onChange(null)}>${t("common.clear")}</button>`}
    </div>
    ${open && html`<${IconPicker} value=${value} onPick=${(v) => { onChange(v); setOpen(false); }} onClose=${() => setOpen(false)} />`}`;
}

export function IconPicker({ value, onPick, onClose }) {
  const [tab, setTab] = useState("icons");
  const [query, setQuery] = useState("");
  useEffect(() => { loadCatalog(); }, []);
  const results = useMemo(() => {
    if (!catalog.value) return [];
    const q = query.trim().toLowerCase();
    const out = [];
    for (const [name, { tags }] of catalog.value) {
      if (!q || name.includes(q) || tags.toLowerCase().includes(q)) out.push(name);
      if (out.length >= 400) break;
    }
    return out;
  }, [catalog.value, query]);
  const upload = async (file) => {
    const url = await imageToDataUrl(file, 96);
    if (url) onPick(url);
  };
  return html`
    <${Dialog} title=${t("icon.title")} onClose=${onClose} wide>
      <div class="row" style=${{ marginBottom: "12px" }}>
        <div class="segmented">
          <button aria-pressed=${tab === "icons"} onClick=${() => setTab("icons")}>${t("icon.tab.icons")}</button>
          <button aria-pressed=${tab === "apps"} onClick=${() => setTab("apps")}>${t("icon.tab.apps")}</button>
          <button aria-pressed=${tab === "upload"} onClick=${() => setTab("upload")}>${t("icon.tab.upload")}</button>
        </div>
        ${tab === "icons" && html`<input type="search" class="grow" placeholder=${t("icon.search")} value=${query} onInput=${(e) => setQuery(e.currentTarget.value)} autofocus />`}
      </div>
      ${tab === "icons" && (catalog.value
        ? html`<div class="icon-grid">
            ${results.map((name) => html`
              <button title=${name} aria-label=${name} aria-pressed=${value === `lucide:${name}`} onClick=${() => onPick(`lucide:${name}`)}>
                <${Icon} name=${name} size=${20} />
              </button>`)}
          </div>
          <p class="hint" style=${{ marginTop: "10px" }}>${t("icon.lucide")}</p>`
        : html`<div class="empty"><${Icon} name="loader" />${t("common.loading")}</div>`)}
      ${tab === "apps" && html`<${AppList} onPick=${(app) => onPick(`app:${app.id}`)} />`}
      ${tab === "upload" && html`
        <div class="stack">
          <p class="hint">${t("icon.upload.hint")}</p>
          <label class="btn" style=${{ justifySelf: "start" }}>
            <${Icon} name="upload" size=${16} />${t("image.choose")}
            <input type="file" class="sr-only" accept="image/png,image/jpeg,image/gif,image/svg+xml" onChange=${(e) => e.currentTarget.files[0] && upload(e.currentTarget.files[0])} />
          </label>
        </div>`}
    <//>`;
}

/** Largest embedded image the server accepts, as a data URL (base64 included). */
const MAX_IMAGE_DATA_URL = 3 * 1024 * 1024;

/**
 * Reads an image file and scales it so its longer side is at most `max` px.
 * The result must fit the server's per-image limit; otherwise the user gets
 * a readable message here instead of a config that can no longer be saved.
 */
export async function imageToDataUrl(file, max) {
  if (file.size > 8 * 1024 * 1024) {
    toast(t("image.too.big"), "error");
    return null;
  }
  let url;
  if (file.type === "image/svg+xml") {
    url = await new Promise((resolve) => {
      const reader = new FileReader();
      reader.onload = () => resolve(reader.result);
      reader.onerror = () => resolve(null);
      reader.readAsDataURL(file);
    });
  } else {
    const bitmap = await createImageBitmap(file).catch(() => null);
    if (!bitmap) {
      toast(t("image.unreadable"), "error");
      return null;
    }
    const scale = Math.min(1, max / Math.max(bitmap.width, bitmap.height));
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(bitmap.width * scale));
    canvas.height = Math.max(1, Math.round(bitmap.height * scale));
    canvas.getContext("2d").drawImage(bitmap, 0, 0, canvas.width, canvas.height);
    url = canvas.toDataURL("image/png");
    // Photos compress far better as JPEG than as PNG.
    if (url.length > MAX_IMAGE_DATA_URL) url = canvas.toDataURL("image/jpeg", 0.88);
  }
  if (!url) {
    toast(t("image.unreadable"), "error");
    return null;
  }
  if (url.length > MAX_IMAGE_DATA_URL) {
    toast(t("image.too.big.embedded"), "error");
    return null;
  }
  return url;
}

// ---- applications ------------------------------------------------------------

let appsCache = null;
export function useApps() {
  const [apps, setApps] = useState(appsCache);
  useEffect(() => {
    if (appsCache) return;
    api.get("/api/apps").then((data) => {
      appsCache = data;
      setApps(data);
    }).catch(() => setApps({ apps: [], defaults: [] }));
  }, []);
  return apps;
}

export function AppList({ onPick, includeDefaults = false }) {
  const data = useApps();
  const [query, setQuery] = useState("");
  if (!data) return html`<div class="empty"><${Icon} name="loader" />${t("common.loading")}</div>`;
  const q = query.trim().toLowerCase();
  const apps = data.apps.filter((a) => !q || a.name.toLowerCase().includes(q) || a.id.toLowerCase().includes(q));
  return html`
    <input type="search" placeholder=${t("app.search")} value=${query} onInput=${(e) => setQuery(e.currentTarget.value)} style=${{ marginBottom: "8px" }} />
    <div class="app-list list">
      ${includeDefaults && !q && data.defaults.map((d) => html`
        <button class="pick" onClick=${() => onPick(d)}>
          <${Icon} name=${{ "default:browser": "globe", "default:files": "folder-open", "default:terminal": "terminal", "default:mail": "mail" }[d.id] ?? "app-window"} size=${24} />
          <span class="grow"><span class="title">${t(`app.${d.id}`)}</span><br /><span class="meta">${t("app.default.hint")}</span></span>
        </button>`)}
      ${apps.map((a) => html`
        <button class="pick" onClick=${() => onPick(a)}>
          ${a.icon_url ? html`<img src=${a.icon_url} alt="" loading="lazy" />` : html`<${Icon} name="app-window" size=${24} />`}
          <span class="grow"><span class="title">${a.name}</span><br /><span class="meta">${a.id}</span></span>
        </button>`)}
    </div>`;
}

export function AppField({ value, onChange }) {
  const data = useApps();
  const [open, setOpen] = useState(false);
  const app = data?.apps.find((a) => a.id === value);
  const name = value?.startsWith("default:") ? t(`app.${value}`) : app?.name ?? value;
  return html`
    <div class="row tight">
      <span class="grow">${value ? name : html`<span class="muted">${t("app.none")}</span>`}</span>
      <button class="btn small" onClick=${() => setOpen(true)}>${t("app.choose")}</button>
    </div>
    ${open && html`
      <${Dialog} title=${t("app.title")} onClose=${() => setOpen(false)}>
        <${AppList} includeDefaults onPick=${(a) => { onChange(a.id); setOpen(false); }} />
      <//>`}`;
}

// ---- metrics -------------------------------------------------------------------

const UNIT_SUFFIX = { percent: "%", celsius: "°C", watts: "W", rpm: "RPM", megahertz: "MHz" };

export function formatReading(m) {
  if (!m) return "";
  if (m.unit === "bytes" || m.unit === "bytes_per_sec") {
    const units = ["B", "KB", "MB", "GB", "TB"];
    let v = m.value;
    let i = 0;
    while (v >= 1000 && i < units.length - 1) { v /= 1024; i += 1; }
    return `${v >= 100 || i === 0 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}${m.unit === "bytes_per_sec" ? "/s" : ""}`;
  }
  const digits = Math.abs(m.value) < 10 && m.unit === "plain" ? 1 : 0;
  return `${m.value.toFixed(digits)} ${UNIT_SUFFIX[m.unit] ?? ""}`.trim();
}

export function MetricField({ value, onChange, optional }) {
  useEffect(() => { if (!metricsCatalog.value.length) refreshMetrics(); }, []);
  const groups = useMemo(() => {
    const map = new Map();
    for (const m of metricsCatalog.value) {
      const family = m.id.includes(":") ? m.id.split(":")[0] : m.id.split(".")[0];
      if (!map.has(family)) map.set(family, []);
      map.get(family).push(m);
    }
    return [...map.entries()];
  }, [metricsCatalog.value]);
  const known = metricsCatalog.value.some((m) => m.id === value);
  return html`
    <select value=${value ?? ""} onChange=${(e) => onChange(e.currentTarget.value || null)}>
      ${optional && html`<option value="">${t("common.none")}</option>`}
      ${!known && value && html`<option value=${value}>${value}</option>`}
      ${groups.map(([family, items]) => html`
        <optgroup label=${t(`metric.family.${family}`)}>
          ${items.map((m) => html`<option value=${m.id}>${m.label} — ${formatReading(m)}</option>`)}
        </optgroup>`)}
    </select>`;
}

// ---- places ----------------------------------------------------------------------

export function LocationField({ value, onChange, optional }) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState(null);
  const [busy, setBusy] = useState(false);
  const search = async () => {
    if (!query.trim()) return;
    setBusy(true);
    try {
      const data = await api.get(`/api/geocode?q=${encodeURIComponent(query)}&lang=${lang.value}`);
      setResults(data.places);
    } catch (error) {
      toast(error.message, "error");
    } finally {
      setBusy(false);
    }
  };
  return html`
    <div class="stack" style=${{ gap: "8px" }}>
      ${value
        ? html`<div class="row tight"><${Icon} name="house" size=${16} /><span class="grow">${value.name}</span>
            <button class="btn small ghost" onClick=${() => onChange(null)}>${optional ? t("location.use.default") : t("common.clear")}</button></div>`
        : optional && html`<span class="hint">${t("location.default")}</span>`}
      <div class="row tight">
        <input type="search" class="grow" placeholder=${t("location.search")} value=${query}
          onInput=${(e) => setQuery(e.currentTarget.value)} onKeyDown=${(e) => e.key === "Enter" && search()} />
        <button class="btn small" onClick=${search} disabled=${busy}>${t("common.search")}</button>
      </div>
      ${results && (results.length
        ? html`<div class="list">${results.map((p) => html`
            <button class="pick"
              onClick=${() => { onChange({ name: p.name, latitude: p.latitude, longitude: p.longitude }); setResults(null); setQuery(""); }}>
              <span class="grow"><span class="title">${p.name}</span><br /><span class="meta">${[p.admin1, p.country].filter(Boolean).join(", ")}</span></span>
            </button>`)}</div>`
        : html`<span class="hint">${t("location.none")}</span>`)}
    </div>`;
}

// ---- Home Assistant entities --------------------------------------------------------

let entitiesCache = null;
export function EntityField({ value, onChange, domains }) {
  const [entities, setEntities] = useState(entitiesCache);
  const [error, setError] = useState(null);
  useEffect(() => {
    if (entitiesCache) return;
    api.get("/api/home-assistant/entities")
      .then((data) => { entitiesCache = data.entities; setEntities(data.entities); })
      .catch((e) => setError(e.message));
  }, []);
  if (error || !entities) {
    return html`
      <input type="text" value=${value ?? ""} placeholder="light.desk" onInput=${(e) => onChange(e.currentTarget.value)} />
      ${error && html`<small class="hint">${t("ha.not.connected")}</small>`}`;
  }
  const list = domains ? entities.filter((e) => domains.includes(e.domain)) : entities;
  return html`
    <select value=${value ?? ""} onChange=${(e) => onChange(e.currentTarget.value)}>
      <option value="">${t("ha.pick")}</option>
      ${list.map((e) => html`<option value=${e.entity_id}>${e.name || e.entity_id} (${e.state}${e.unit ? ` ${e.unit}` : ""})</option>`)}
    </select>`;
}

// ---- OBS scenes and audio sources ------------------------------------------------

let obsSources = null;
let obsLoading = null;
/** Counts connection changes, so a load for an older one is not kept. */
let obsGeneration = 0;
/** Forgets the lists, e.g. after the connection changed. */
export function forgetObsSources() {
  obsSources = null;
  obsLoading = null;
  obsGeneration += 1;
}

/** OBS's scenes and audio sources, fetched once per connection. */
export function loadObsSources() {
  if (obsSources) return Promise.resolve(obsSources);
  const generation = obsGeneration;
  obsLoading ??= api.get("/api/obs/sources")
    .then((data) => {
      if (generation === obsGeneration) obsSources = data;
      return data;
    })
    .finally(() => { if (generation === obsGeneration) obsLoading = null; });
  return obsLoading;
}

/**
 * A name to start a scene or mute button with, since the server refuses an
 * empty one: OBS's current scene or first audio source when known, else the
 * names a fresh English OBS uses.
 */
export function defaultObsTarget(kind) {
  if (kind === "scenes") return obsSources?.current_scene || obsSources?.scenes?.[0] || "Scene";
  return obsSources?.audio_inputs?.[0] || "Mic/Aux";
}

/** A scene (`kind` "scenes") or audio source ("audio_inputs") of OBS. */
export function ObsField({ value, onChange, kind }) {
  const [sources, setSources] = useState(obsSources);
  const [error, setError] = useState(null);
  useEffect(() => {
    if (obsSources) return;
    loadObsSources().then(setSources).catch((e) => setError(e.message));
  }, []);
  const names = sources?.[kind] ?? [];
  if (error || !sources) {
    return html`
      <input type="text" value=${value ?? ""} placeholder=${kind === "scenes" ? "Scene" : "Mic/Aux"} onInput=${(e) => onChange(e.currentTarget.value)} />
      ${error && html`<small class="hint">${t("obs.not.connected")}</small>`}`;
  }
  // A name OBS does not list (renamed, or OBS runs another collection) stays choosable.
  const options = value && !names.includes(value) ? [value, ...names] : names;
  return html`
    <select value=${value ?? ""} onChange=${(e) => onChange(e.currentTarget.value)}>
      ${!value && html`<option value="">${t(kind === "scenes" ? "obs.pick.scene" : "obs.pick.source")}</option>`}
      ${options.map((name) => html`<option value=${name}>${name}</option>`)}
    </select>`;
}

// ---- images --------------------------------------------------------------------------

export function ImageField({ value, onChange, max = 320 }) {
  return html`
    <div class="stack" style=${{ gap: "8px" }}>
      ${value && html`<img src=${value} alt="" style=${{ maxWidth: "100%", maxHeight: "60px", borderRadius: "6px", justifySelf: "start" }} />`}
      <div class="row tight">
        <label class="btn small">
          <${Icon} name="upload" size=${15} />${value ? t("image.replace") : t("image.choose")}
          <input type="file" class="sr-only" accept="image/png,image/jpeg,image/gif,image/svg+xml"
            onChange=${async (e) => { const f = e.currentTarget.files[0]; if (f) { const url = await imageToDataUrl(f, max); if (url) onChange(url); } }} />
        </label>
        ${value && html`<button class="btn small ghost" onClick=${() => onChange("")}>${t("common.clear")}</button>`}
      </div>
    </div>`;
}

export function ToggleField({ label, value, onChange }) {
  return html`<div class="field inline"><span>${label}</span><${Switch} ariaLabel=${label} checked=${value} onChange=${onChange} /></div>`;
}

/** One argument per line. */
export function ArgsField({ value, onChange }) {
  return html`<textarea rows="3" value=${(value ?? []).join("\n")} onInput=${(e) => onChange(e.currentTarget.value.split("\n").filter((l) => l.length))}></textarea>`;
}
