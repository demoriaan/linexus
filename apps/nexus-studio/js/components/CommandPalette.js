// Ctrl/Cmd+K: jump anywhere and run common commands by typing.
import { html, signal, useEffect, useMemo, useRef, useState } from "../lib/html.js";
import { t, setLang, lang } from "../lib/i18n.js";
import { Icon } from "../lib/icons.js";
import { api } from "../lib/api.js";
import {
  config, profile, selectedPageId, selectedWidgetId, selectedProfileId, undo, redo, toast, templates,
} from "../lib/store.js";
import { WIDGET_ORDER, WIDGETS } from "../widgets.js";
import { addWidgetAt } from "./Stage.js";
import { activateProfile } from "./TopBar.js";
import { setTheme } from "../lib/theme.js";

const open = signal(false);
export const openPalette = () => (open.value = true);

window.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
    e.preventDefault();
    open.value = !open.value;
  }
});

function commands() {
  const list = [];
  const go = (hash) => () => (location.hash = hash);
  for (const [hash, icon] of [["pages", "layout-panel-top"], ["news", "newspaper"], ["connections", "plug"], ["device", "monitor"], ["settings", "settings"]]) {
    list.push({ id: `view:${hash}`, icon, label: t(`nav.${hash}`), hint: t("palette.view"), run: go(hash) });
  }
  const prof = profile.value;
  for (const pg of prof?.pages ?? []) {
    list.push({
      id: `page:${pg.id}`, icon: "layout-panel-top", label: pg.name, hint: t("palette.page"),
      run: () => { location.hash = "pages"; selectedPageId.value = pg.id; selectedWidgetId.value = null; },
    });
    list.push({
      id: `show:${pg.id}`, icon: "monitor", label: t("palette.show.page", { name: pg.name }), hint: t("palette.device"),
      run: () => api.post("/api/daemon/page", { page_id: pg.id }).catch((e) => toast(e.message, "error")),
    });
  }
  for (const type of WIDGET_ORDER) {
    list.push({
      id: `add:${type}`, icon: WIDGETS[type].icon, label: t("palette.add.widget", { name: t(`widget.${type}`) }), hint: t("palette.widget"),
      run: () => { location.hash = "pages"; addWidgetAt(type, 320); },
    });
  }
  for (const p of config.value?.profiles ?? []) {
    list.push({ id: `edit-profile:${p.id}`, icon: "layers", label: t("palette.edit.profile", { name: p.name }), hint: t("palette.profile"), run: () => { selectedProfileId.value = p.id; selectedPageId.value = null; } });
    list.push({ id: `activate:${p.id}`, icon: "monitor", label: t("profile.activate", { name: p.name }), hint: t("palette.device"), run: () => activateProfile(p.id) });
  }
  const device = (path, body, done) => () => api.post(path, body).then(() => done && toast(done, "ok")).catch((e) => toast(e.message, "error"));
  list.push({ id: "blank", icon: "eye-off", label: t("palette.blank"), hint: t("palette.device"), run: device("/api/daemon/blank", {}) });
  list.push({ id: "restore", icon: "eye", label: t("palette.restore"), hint: t("palette.device"), run: device("/api/daemon/restore", {}) });
  for (const percent of [25, 50, 75, 100]) {
    list.push({ id: `bright:${percent}`, icon: "sun", label: t("palette.brightness", { n: percent }), hint: t("palette.device"), run: device("/api/daemon/brightness", { percent }) });
  }
  list.push({ id: "restart", icon: "refresh-cw", label: t("palette.restart"), hint: t("palette.device"), run: device("/api/daemon/restart", {}, t("device.restarted")) });
  list.push({ id: "push", icon: "bell", label: t("palette.test.notify"), hint: t("palette.device"), run: device("/api/daemon/notify", { text: t("palette.test.notify.text"), icon: "sparkles" }) });
  list.push({ id: "undo", icon: "undo-2", label: t("common.undo"), hint: "Ctrl Z", run: undo });
  list.push({ id: "redo", icon: "redo-2", label: t("common.redo"), hint: "Ctrl Shift Z", run: redo });
  list.push({ id: "theme-dark", icon: "moon", label: t("palette.theme.dark"), hint: t("palette.studio"), run: () => setTheme("dark") });
  list.push({ id: "theme-light", icon: "sun", label: t("palette.theme.light"), hint: t("palette.studio"), run: () => setTheme("light") });
  list.push({ id: "lang", icon: "languages", label: lang.value === "de" ? "Switch to English" : "Auf Deutsch umschalten", hint: t("palette.studio"), run: () => setLang(lang.value === "de" ? "en" : "de") });
  list.push({ id: "export", icon: "download", label: t("settings.export"), hint: t("palette.backup"), run: () => api.download("/api/pack/export") });
  if (templates.value.length) {
    list.push({ id: "add-page", icon: "list-plus", label: t("pages.add.title"), hint: t("palette.page"), run: () => { location.hash = "pages"; document.querySelector(".thumb.add")?.click(); } });
  }
  return list;
}

function score(label, query) {
  const l = label.toLowerCase();
  if (l.startsWith(query)) return 3;
  if (l.includes(query)) return 2;
  let i = 0;
  for (const ch of l) if (ch === query[i]) i += 1;
  return i === query.length ? 1 : 0;
}

export function CommandPalette() {
  if (!open.value) return null;
  return html`<${PaletteDialog} />`;
}

function PaletteDialog() {
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const input = useRef(null);
  const all = useMemo(commands, []);
  const q = query.trim().toLowerCase();
  const results = q
    ? all.map((c) => [c, score(c.label, q)]).filter(([, s]) => s > 0).sort((a, b) => b[1] - a[1]).map(([c]) => c).slice(0, 60)
    : all.slice(0, 60);
  useEffect(() => input.current?.focus(), []);
  useEffect(() => setIndex(0), [query]);
  const run = (cmd) => {
    open.value = false;
    cmd?.run();
  };
  const onKey = (e) => {
    if (e.key === "Escape") open.value = false;
    else if (e.key === "ArrowDown") { e.preventDefault(); setIndex(Math.min(results.length - 1, index + 1)); }
    else if (e.key === "ArrowUp") { e.preventDefault(); setIndex(Math.max(0, index - 1)); }
    else if (e.key === "Enter") run(results[index]);
  };
  return html`
    <div class="scrim" onMouseDown=${(e) => e.target === e.currentTarget && (open.value = false)}>
      <div class="dialog palette" role="dialog" aria-modal="true" aria-label=${t("palette.open")}>
        <input ref=${input} type="search" role="combobox" aria-expanded="true" aria-controls="palette-list"
          placeholder=${t("palette.placeholder")} value=${query} onInput=${(e) => setQuery(e.currentTarget.value)} onKeyDown=${onKey} />
        <div class="menu" id="palette-list" role="listbox">
          ${results.map((c, i) => html`
            <button role="option" aria-selected=${i === index} onMouseEnter=${() => setIndex(i)} onClick=${() => run(c)}>
              <${Icon} name=${c.icon} size=${16} />${c.label}<small>${c.hint}</small>
            </button>`)}
          ${!results.length && html`<div class="empty">${t("palette.none")}</div>`}
        </div>
      </div>
    </div>`;
}
