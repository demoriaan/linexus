// Right-hand panel: properties of the selected widget, or of the page and
// profile when nothing is selected.
import { html, useEffect, useState } from "../lib/html.js";
import { t } from "../lib/i18n.js";
import { Icon, IconRef } from "../lib/icons.js";
import {
  profile, page, widget, selectedWidgetId, selectedPageId, edit, withPage, profileIndex,
  limits, freeGaps, uniqueId, themes, STRIP_W, MIN_W,
} from "../lib/store.js";
import { WIDGETS, BUILTIN, widgetTitle } from "../widgets.js";
import { ConfirmButton, Switch, Dialog } from "./Dialog.js";
import {
  Field, TextInput, NumberInput, Select, IconField, MetricField, LocationField, EntityField,
  ImageField, ArgsField, AppList, useApps,
} from "./Fields.js";
import { ActionEditor } from "./ActionEditor.js";
import { api } from "../lib/api.js";

function editWidget(patch, coalesce) {
  const id = selectedWidgetId.value;
  edit((draft) => withPage(draft, (pg) => {
    const w = pg.widgets.find((it) => it.id === id);
    if (!w) return;
    for (const [k, v] of Object.entries(patch)) {
      if (v === null || v === undefined || v === "") delete w[k];
      else w[k] = v;
    }
  }), { coalesce });
}

export function Inspector() {
  return html`<aside class="inspector">${widget.value ? html`<${WidgetInspector} />` : html`<${PageInspector} />`}</aside>`;
}

function WidgetInspector() {
  const w = widget.value;
  const spec = WIDGETS[w.type];
  const pg = page.value;
  const { left, right } = limits(pg, w.id);
  const duplicate = () => {
    const gaps = freeGaps(pg);
    const gap = gaps.find((g) => g.w >= Math.min(w.w, g.w) && g.w >= MIN_W);
    if (!gap) return;
    const id = uniqueId(w.id, new Set(pg.widgets.map((it) => it.id)));
    edit((draft) => withPage(draft, (p) => {
      p.widgets.push({ ...structuredClone(w), id, x: gap.x, w: Math.min(w.w, gap.w) });
      p.widgets.sort((a, b) => a.x - b.x);
    }));
    selectedWidgetId.value = id;
  };
  const remove = () => {
    edit((draft) => withPage(draft, (p) => { p.widgets = p.widgets.filter((it) => it.id !== w.id); }));
    selectedWidgetId.value = null;
  };
  return html`
    <section class="panel">
      <div class="kind">
        <${Icon} name=${spec.icon} />
        <div class="grow">
          <h3>${t(`widget.${w.type}`)}</h3>
          <span class="hint">${widgetTitle(w, t)}</span>
        </div>
        <button class="icon-btn" title=${t("widget.duplicate")} aria-label=${t("widget.duplicate")} onClick=${duplicate} disabled=${!freeGaps(pg).length}><${Icon} name="copy" /></button>
        <${ConfirmButton} class="btn small ghost danger" title=${t("common.delete")} onConfirm=${remove}><${Icon} name="trash-2" size=${15} /><//>
      </div>
      ${spec.fields.map((f) => html`<${PropField} key=${f.key} field=${f} w=${w} />`)}
      <div class="two">
        <${Field} label=${t("widget.x")}><${NumberInput} value=${w.x} min=${left} max=${right - w.w} onInput=${(x) => editWidget({ x: Math.max(left, Math.min(right - w.w, Math.round(x))) }, "x")} /><//>
        <${Field} label=${t("widget.width")}><${NumberInput} value=${w.w} min=${MIN_W} max=${right - w.x} onInput=${(v) => editWidget({ w: Math.max(MIN_W, Math.min(right - w.x, Math.round(v))) }, "w")} /><//>
      </div>
      <${AccentField} value=${w.accent} onChange=${(accent) => editWidget({ accent })} />
    </section>
    <section class="panel">
      <header><h3>${t("gesture.title")}</h3></header>
      <p class="hint" style=${{ marginTop: "-6px" }}>${t("gesture.hint")}</p>
      ${["tap", "double_tap", "hold"].map((g) => html`
        <div class="gesture">
          <div class="row"><b>${t(`gesture.${g}`)}</b></div>
          ${!w[g] && BUILTIN[w.type]?.[g] && html`<p class="builtin">${t(BUILTIN[w.type][g])}</p>`}
          <${ActionEditor} value=${w[g] ?? null} onChange=${(a) => editWidget({ [g]: a }, `${g}:${w.id}`)} />
          ${g === "hold" && (w.hold || w.tap) && html`
            <div class="field inline"><span>${t(w.hold ? "gesture.repeat" : "gesture.repeat.tap")}</span>
              <${Switch} ariaLabel=${t("gesture.repeat")} checked=${!!w.repeat} onChange=${(on) => editWidget({ repeat: on || undefined })} /></div>`}
        </div>`)}
    </section>`;
}

function PropField({ field, w }) {
  if (field.when && Object.entries(field.when).some(([k, v]) => w[k] !== v)) return null;
  const label = t(`prop.${w.type}.${field.key}`);
  const value = w[field.key];
  const set = (v) => editWidget({ [field.key]: v }, `${field.key}:${w.id}`);
  switch (field.kind) {
    case "toggle":
      return html`<div class="field inline"><span>${label}</span><${Switch} ariaLabel=${label} checked=${value} onChange=${set} /></div>`;
    case "text":
      return html`<${Field} label=${label} hint=${field.tokens ? t("prop.tokens.hint") : null}><${TextInput} value=${value} optional=${field.optional} onInput=${set} /><//>`;
    case "number":
      return html`<${Field} label=${label}><${NumberInput} value=${value} min=${field.min} max=${field.max} optional=${field.optional} onInput=${set} /><//>`;
    case "select": {
      const current = field.cast === "bool?" ? (value === undefined || value === null ? "" : String(value)) : value;
      const onChange = (v) => set(field.cast === "bool?" ? (v === "" ? null : v === "true") : v);
      return html`<${Field} label=${label}><${Select} value=${current} options=${field.options} labelOf=${(o) => t(`opt.${field.key}.${o || "auto"}`)} onChange=${onChange} /><//>`;
    }
    case "icon":
      return html`<${Field} group label=${label}><${IconField} value=${value} optional=${field.optional} onChange=${set} /><//>`;
    case "metric":
      return html`<${Field} label=${label}><${MetricField} value=${value} optional=${field.optional} onChange=${set} /><//>`;
    case "location":
      return html`<${Field} group label=${label}><${LocationField} value=${value} optional=${field.optional} onChange=${set} /><//>`;
    case "entity":
      return html`<${Field} label=${label}><${EntityField} value=${value} onChange=${set} /><//>`;
    case "image":
      return html`<${Field} group label=${label}><${ImageField} value=${value} max=${400} onChange=${set} /><//>`;
    case "args":
      return html`<${Field} label=${label} hint=${t("prop.args.hint")}><${ArgsField} value=${value} onChange=${set} /><//>`;
    case "feeds":
      return html`<${FeedsField} value=${value ?? []} onChange=${set} label=${label} />`;
    default:
      return null;
  }
}

function FeedsField({ value, onChange, label }) {
  const [feeds, setFeeds] = useState(null);
  useEffect(() => {
    api.get("/api/sensors/config").then((d) => setFeeds(d.config.rss ?? [])).catch(() => setFeeds([]));
  }, []);
  if (!feeds) return null;
  const toggle = (id, on) => onChange(on ? [...value, id] : value.filter((x) => x !== id));
  return html`
    <div class="field">
      <span>${label}</span>
      <small>${value.length ? t("prop.news.feeds.some") : t("prop.news.feeds.all")}</small>
      ${feeds.map((f) => html`
        <label class="row tight"><input type="checkbox" checked=${value.includes(f.id)} onChange=${(e) => toggle(f.id, e.currentTarget.checked)} />${f.id}</label>`)}
      ${!feeds.length && html`<small><a href="#news">${t("prop.news.feeds.add")}</a></small>`}
    </div>`;
}

function AccentField({ value, onChange, label = t("widget.accent") }) {
  const preset = themes.value.find((th) => th.id === (profile.value?.theme?.preset ?? "midnight"));
  const swatches = preset?.palette?.series ?? [];
  return html`
    <div class="field">
      <span>${label}</span>
      <div class="swatches">
        <button title=${t("common.auto")} aria-pressed=${!value} onClick=${() => onChange(null)}
          style=${{ background: "conic-gradient(#3ee6ca, #ffb84d, #b98cff, #5aa9ff, #ff6b8b, #3ee6ca)" }}></button>
        ${swatches.map((c) => html`<button title=${c} aria-pressed=${value === c} style=${{ background: c }} onClick=${() => onChange(c)}></button>`)}
        <input type="color" value=${value ?? swatches[0] ?? "#5aa9ff"} onInput=${(e) => onChange(e.currentTarget.value)} title=${t("common.custom")}
          style=${{ width: "30px", height: "26px", padding: 0, border: 0, background: "none" }} />
      </div>
    </div>`;
}

function editPage(patch, coalesce) {
  edit((draft) => withPage(draft, (pg) => {
    for (const [k, v] of Object.entries(patch)) {
      if (v === null || v === undefined || v === false) delete pg[k];
      else pg[k] = v;
    }
  }), { coalesce });
}

function PageInspector() {
  const pg = page.value;
  const prof = profile.value;
  if (!pg) return null;
  const bg = pg.background ?? {};
  const setBg = (patch) => {
    const next = { ...bg, ...patch };
    for (const k of Object.keys(next)) if (next[k] === null || next[k] === "") delete next[k];
    editPage({ background: next.image || next.color ? { fit: "fill", dim_percent: 0, ...next } : null }, "bg");
  };
  const duplicate = () => {
    const id = uniqueId(pg.id, new Set(prof.pages.map((p) => p.id)));
    edit((draft) => {
      const pages = draft.profiles[profileIndex(draft)].pages;
      const at = pages.findIndex((p) => p.id === pg.id);
      pages.splice(at + 1, 0, { ...structuredClone(pg), id, name: `${pg.name} 2` });
    });
    selectedPageId.value = id;
  };
  const remove = () => {
    edit((draft) => {
      const pages = draft.profiles[profileIndex(draft)].pages;
      draft.profiles[profileIndex(draft)].pages = pages.filter((p) => p.id !== pg.id);
    });
    selectedPageId.value = null;
  };
  const visibleCount = prof.pages.filter((p) => !p.hidden).length;
  return html`
    <section class="panel">
      <div class="kind">
        <${Icon} name="layout-panel-top" />
        <div class="grow"><h3>${t("page.title")}</h3><span class="hint">${t("page.select.hint")}</span></div>
        <button class="icon-btn" title=${t("page.duplicate")} aria-label=${t("page.duplicate")} onClick=${duplicate}><${Icon} name="copy" /></button>
        ${prof.pages.length > 1 && html`<${ConfirmButton} class="btn small ghost danger" title=${t("common.delete")} onConfirm=${remove}><${Icon} name="trash-2" size=${15} /><//>`}
      </div>
      <${Field} label=${t("page.name")}><${TextInput} value=${pg.name} onInput=${(name) => editPage({ name }, "name")} /><//>
      <div class="field inline">
        <span>${t("page.hidden.label")}</span>
        <${Switch} checked=${!!pg.hidden} disabled=${!pg.hidden && visibleCount <= 1} onChange=${(hidden) => editPage({ hidden })} />
      </div>
      <small class="hint" style=${{ marginTop: "-8px" }}>${t("page.hidden.hint")}</small>
      ${pg.hidden && html`<${Field} label=${t("page.auto.return")} hint=${t("page.auto.return.hint")}>
        <${NumberInput} value=${pg.auto_return_seconds} min=${3} max=${3600} optional onInput=${(v) => editPage({ auto_return_seconds: v })} /><//>`}
    </section>
    <section class="panel">
      <header><h3>${t("page.background")}</h3></header>
      <${Field} group label=${t("page.background.image")}><${ImageField} value=${bg.image} max=${1280} onChange=${(image) => setBg({ image })} /><//>
      <div class="two">
        <${Field} label=${t("page.background.fit")}><${Select} value=${bg.fit ?? "fill"} options=${["fill", "fit", "stretch"]} labelOf=${(o) => t(`opt.fit.${o}`)} onChange=${(fit) => setBg({ fit })} /><//>
        <${Field} label=${t("page.background.color")}>
          <input type="color" value=${bg.color ?? "#000000"} onInput=${(e) => setBg({ color: e.currentTarget.value })} style=${{ height: "34px", width: "100%", padding: 0, border: 0, background: "none" }} />
        <//>
      </div>
      <${Field} label=${t("page.background.dim", { n: bg.dim_percent ?? 0 })}>
        <input type="range" min="0" max="90" step="5" value=${bg.dim_percent ?? 0} onInput=${(e) => setBg({ dim_percent: Number(e.currentTarget.value) })} />
      <//>
      ${(bg.image || bg.color) && html`<button class="btn small ghost" onClick=${() => editPage({ background: null })}>${t("page.background.reset")}</button>`}
    </section>
    <${ProfileStyle} />`;
}

function ProfileStyle() {
  const prof = profile.value;
  const theme = prof.theme ?? { preset: "midnight" };
  const setTheme = (patch) => edit((draft) => {
    const p = draft.profiles[profileIndex(draft)];
    p.theme = { ...theme, ...patch };
    if (!p.theme.accent) delete p.theme.accent;
  }, { coalesce: "theme" });
  return html`
    <section class="panel">
      <header><h3>${t("profile.style")}</h3></header>
      <p class="hint" style=${{ marginTop: "-6px" }}>${t("profile.style.hint", { name: prof.name })}</p>
      <div class="field">
        <span>${t("profile.theme")}</span>
        <div class="swatches" style=${{ gap: "8px" }}>
          ${themes.value.map((th) => html`
            <button title=${t(`theme.${th.id}`)} aria-label=${t(`theme.${th.id}`)} aria-pressed=${theme.preset === th.id}
              style=${{ width: "44px", height: "30px", borderRadius: "8px", background: `linear-gradient(135deg, ${th.palette.bg_top} 55%, ${th.palette.accent} 55%)` }}
              onClick=${() => setTheme({ preset: th.id })}></button>`)}
        </div>
      </div>
      <${AccentField} label=${t("profile.accent")} value=${theme.accent} onChange=${(accent) => setTheme({ accent })} />
      <${AutoApps} />
      <${Field} label=${t("profile.brightness", { n: prof.brightness })}>
        <input type="range" min="5" max="100" step="5" value=${prof.brightness}
          onInput=${(e) => {
            const brightness = Number(e.currentTarget.value);
            edit((draft) => { draft.profiles[profileIndex(draft)].brightness = brightness; }, { coalesce: "brightness" });
            api.post("/api/daemon/brightness", { percent: brightness }).catch(() => {});
          }} />
      <//>
    </section>`;
}

/** Apps that switch the strip to this profile while they have focus. */
function AutoApps() {
  const prof = profile.value;
  const apps = prof.apps ?? [];
  const [picking, setPicking] = useState(false);
  const data = useApps();
  const setApps = (next) => edit((draft) => {
    const p = draft.profiles[profileIndex(draft)];
    if (next.length) p.apps = next;
    else delete p.apps;
  });
  const name = (id) => data?.apps.find((a) => a.id.replace(/\.desktop$/, "") === id)?.name ?? id;
  return html`
    <div class="field">
      <span>${t("profile.apps")}</span>
      <small>${t("profile.apps.hint")}</small>
      <div class="row tight">
        ${apps.map((id) => html`
          <span class="pill" style=${{ cursor: "default" }}>
            <${IconRef} value=${`app:${id}.desktop`} size=${16} />${name(id)}
            <button class="icon-btn" style=${{ width: "20px", height: "20px" }} aria-label=${t("common.remove")} onClick=${() => setApps(apps.filter((a) => a !== id))}><${Icon} name="x" size=${13} /></button>
          </span>`)}
        <button class="btn small" onClick=${() => setPicking(true)}><${Icon} name="plus" size=${14} />${t("profile.apps.add")}</button>
      </div>
    </div>
    ${picking && html`
      <${Dialog} title=${t("app.title")} onClose=${() => setPicking(false)}>
        <${AppList} onPick=${(app) => { const id = app.id.replace(/\.desktop$/, ""); if (!apps.includes(id)) setApps([...apps, id]); setPicking(false); }} />
      <//>`}`;
}

export { STRIP_W };
