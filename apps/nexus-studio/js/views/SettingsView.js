// Settings: Studio's language and look, backups, and credits.
import { html, useEffect, useState } from "../lib/html.js";
import { api } from "../lib/api.js";
import { t, lang, setLang } from "../lib/i18n.js";
import { Icon } from "../lib/icons.js";
import { config, loadConfig, toast, configPath, flush, saveState } from "../lib/store.js";
import { themeChoice, setTheme } from "../lib/theme.js";
import { Segmented, Dialog } from "../components/Dialog.js";

const CREDITS = [
  ["Barlow Semi Condensed", "Jeremy Tribby", "SIL Open Font License 1.1", "https://github.com/jpt/barlow"],
  ["Terminus", "Dimitar Zhekov", "SIL Open Font License 1.1", "https://terminus-font.sourceforge.net/"],
  ["Inter", "Rasmus Andersson", "SIL Open Font License 1.1", "https://rsms.me/inter/"],
  ["Lucide icons", "Lucide contributors", "ISC License", "https://lucide.dev/"],
  ["Preact, Signals", "Preact team", "MIT License", "https://preactjs.com/"],
  ["htm", "Jason Miller", "Apache License 2.0", "https://github.com/developit/htm"],
  ["Weather data", "Open-Meteo", "CC BY 4.0", "https://open-meteo.com/"],
];

/** Whether some action's URL, body, typed text or arguments may hold keys or tokens. */
function mayCarrySecrets(cfg) {
  const types = ["http_request", "open_url", "text_input"];
  const risky = (a) => !!a && (types.includes(a.type) || (a.type === "shell_command" && a.args?.length > 0) || (a.steps ?? []).some(risky));
  return (cfg?.profiles ?? []).some((p) => p.pages.some((pg) => pg.widgets.some((w) =>
    risky(w.tap) || risky(w.double_tap) || risky(w.hold) || (w.type === "command" && w.args?.length > 0))));
}

/**
 * What an uploaded pack would install, before anything is replaced: its
 * size and every button that acts outside the strip, grouped by kind.
 */
function ImportReview({ review, onConfirm, onClose }) {
  const groups = new Map();
  for (const item of review.reach) {
    if (!groups.has(item.kind)) groups.set(item.kind, []);
    groups.get(item.kind).push(item);
  }
  const where = (item) => [item.profile, item.page, item.widget].join(" › ")
    + (item.gesture ? ` (${t(`gesture.${item.gesture}`)})` : "")
    + (item.in_sequence ? ` · ${t("import.reach.sequence")}` : "");
  return html`
    <${Dialog} title=${t("import.title")} onClose=${onClose} wide
      footer=${html`<button class="btn" onClick=${onClose}>${t("common.cancel")}</button>
        <button class=${`btn ${review.reach.length ? "danger" : "primary"}`} onClick=${onConfirm}>${t("import.confirm")}</button>`}>
      <p><b>${t("import.counts", { profiles: review.profiles.length, pages: review.page_count, images: review.embedded_images })}</b></p>
      <p style=${{ marginTop: "6px" }}>${t("import.replaces")}</p>
      ${review.migrated_from && html`<p class="hint" style=${{ marginTop: "8px" }}>${t("import.migrated")}</p>`}
      ${review.notes.length > 0 && html`<ul class="hint">${review.notes.map((note) => html`<li>${note}</li>`)}</ul>`}
      ${review.reach.length
        ? html`
          <div class="banner" style=${{ margin: "14px 0 8px" }}><${Icon} name="alert-triangle" /><div class="grow">${t("import.reach.lead")}</div></div>
          ${[...groups].map(([kind, items]) => html`
            <h4 style=${{ margin: "12px 0 4px" }}>${t(`import.reach.${kind}`)}</h4>
            <div class="list">
              ${items.map((item) => html`
                <div class="list-row"><div class="grow"><div class="meta">${where(item)}</div>
                  <code style=${{ display: "block", wordBreak: "break-all", fontSize: "12.5px" }}>${item.detail}</code></div></div>`)}
            </div>`)}`
        : html`<p class="hint" style=${{ marginTop: "10px" }}>${t("import.reach.none")}</p>`}
    <//>`;
}

export function SettingsView() {
  const [version, setVersion] = useState(null);
  const [review, setReview] = useState(null);
  const [confirmReset, setConfirmReset] = useState(false);
  useEffect(() => { api.get("/api/version").then(setVersion).catch(() => {}); }, []);
  // Step one: the server checks the pack and lists what it would install.
  const inspectPack = async (file) => {
    try {
      const bytes = await file.arrayBuffer();
      setReview({ bytes, data: await api.post("/api/pack/inspect", bytes) });
    } catch (error) {
      toast(error.message, "error");
    }
  };
  // Edits that could not be saved would be lost by a replace, and the backup
  // would not hold them either.
  const savedFirst = async () => {
    await flush();
    if (saveState.value !== "saved") {
      toast(t("settings.unsaved.first"), "error");
      return false;
    }
    return true;
  };
  // Step two, after the user confirmed: pending edits are saved first so the
  // backup holds them, then the reviewed upload replaces the config.
  const importPack = async () => {
    const { bytes, data } = review;
    setReview(null);
    try {
      if (!(await savedFirst())) return;
      const result = await api.post(`/api/pack/import?reviewed=${encodeURIComponent(data.digest)}`, bytes);
      await loadConfig();
      toast(result.backup ? t("settings.imported.backup", { path: result.backup }) : t("settings.imported"), "ok");
    } catch (error) {
      toast(error.message, "error");
    }
  };
  const reset = async () => {
    setConfirmReset(false);
    try {
      if (!(await savedFirst())) return;
      const result = await api.post("/api/config/reset");
      await loadConfig();
      toast(result.backup ? t("settings.reset.done.backup", { path: result.backup }) : t("settings.reset.done"), "ok");
    } catch (error) {
      toast(error.message, "error");
    }
  };
  return html`
    <div class="view">
      <div class="view-head"><div><h1>${t("nav.settings")}</h1><p>${t("settings.lead")}</p></div></div>
      <div class="cols">
        <div class="stack" style=${{ gap: "18px" }}>
          <section class="panel">
            <header><${Icon} name="palette" /><h3>${t("settings.studio")}</h3></header>
            <div class="stack">
              <div class="field"><span>${t("settings.language")}</span>
                <${Segmented} value=${lang.value} onChange=${setLang} options=${[{ value: "en", label: "English" }, { value: "de", label: "Deutsch" }]} />
              </div>
              <div class="field"><span>${t("settings.appearance")}</span>
                <${Segmented} value=${themeChoice.value} onChange=${setTheme}
                  options=${[["system", "monitor-smartphone"], ["dark", "moon"], ["light", "sun"]].map(([value, icon]) => ({ value, icon, label: t(`settings.appearance.${value}`) }))} />
              </div>
            </div>
          </section>
          <section class="panel">
            <header><${Icon} name="download" /><h3>${t("settings.backup")}</h3></header>
            <p class="hint" style=${{ marginBottom: "12px" }}>${t("settings.backup.lead")}</p>
            <div class="row">
              <button class="btn" onClick=${() => api.download("/api/pack/export")}><${Icon} name="download" size=${16} />${t("settings.export")}</button>
              <label class="btn"><${Icon} name="upload" size=${16} />${t("settings.import")}
                <input type="file" accept=".nexuspack,application/zip" class="sr-only"
                  onChange=${(e) => { const file = e.currentTarget.files[0]; e.currentTarget.value = ""; if (file) inspectPack(file); }} />
              </label>
            </div>
            ${mayCarrySecrets(config.value) && html`
              <p class="hint row tight" style=${{ marginTop: "10px", flexWrap: "nowrap", alignItems: "start" }}>
                <${Icon} name="alert-triangle" size=${15} /><span>${t("settings.export.secrets")}</span>
              </p>`}
            <p class="hint" style=${{ marginTop: "12px" }}>${t("settings.config.path", { path: configPath.value })}</p>
            <hr class="divider" style=${{ margin: "14px 0" }} />
            <div class="row"><span class="grow hint">${t("settings.reset.hint")}</span>
              <button class="btn small danger" onClick=${() => setConfirmReset(true)}>${t("settings.reset")}</button></div>
          </section>
          ${review && html`<${ImportReview} review=${review.data} onConfirm=${importPack} onClose=${() => setReview(null)} />`}
          ${confirmReset && html`
            <${Dialog} title=${t("settings.reset.title")} onClose=${() => setConfirmReset(false)}
              footer=${html`<button class="btn" onClick=${() => setConfirmReset(false)}>${t("common.cancel")}</button>
                <button class="btn danger" onClick=${reset}>${t("settings.reset")}</button>`}>
              <p>${t("settings.reset.confirm")}</p>
            <//>`}
        </div>
        <section class="panel">
          <header><${Icon} name="info" /><h3>${t("settings.about")}</h3></header>
          <p style=${{ marginBottom: "10px" }}>${t("settings.about.lead")}</p>
          ${version && html`<p class="hint" style=${{ marginBottom: "12px" }}>${t("settings.version", { v: version.version })}</p>`}
          <h4 style=${{ margin: "14px 0 6px" }}>${t("settings.credits")}</h4>
          <div class="list">
            ${CREDITS.map(([name, author, license, url]) => html`
              <div class="list-row"><div class="grow"><a href=${url} target="_blank" rel="noreferrer" class="title">${name}</a><div class="meta">${author}</div></div><span class="hint">${license}</span></div>`)}
          </div>
          <p class="hint" style=${{ marginTop: "14px" }}>${t("settings.trademark")}</p>
        </section>
      </div>
    </div>`;
}
