// Pickers for the sound widgets and the output button: PipeWire's outputs,
// microphones and the apps playing now, read when first needed.
import { html, useEffect, useState } from "../lib/html.js";
import { api } from "../lib/api.js";
import { t } from "../lib/i18n.js";
import { Icon } from "../lib/icons.js";
import { config } from "../lib/store.js";

let audio = null;
let loading = null;

/** Outputs, microphones and playing apps; `fresh` reads them again. */
export function loadAudio(fresh = false) {
  if (fresh) audio = null;
  if (audio) return Promise.resolve(audio);
  loading ??= api.get("/api/audio")
    .then((data) => { audio = data; return data; })
    .finally(() => { loading = null; });
  return loading;
}

/** [data, error, reload] for components that list devices. */
export function useAudio() {
  const [data, setData] = useState(audio);
  const [error, setError] = useState(null);
  const load = (fresh) => loadAudio(fresh)
    .then((d) => { setData(d); setError(null); })
    .catch((e) => setError(e.message));
  useEffect(() => { if (!audio) load(false); }, []);
  return [data, error, () => load(true)];
}

export const KIND_ICON = { headset: "headphones", monitor: "monitor", speaker: "speaker", microphone: "mic" };

/** Pinned apps and the outputs of a button: as many as the config takes (MAX_AUDIO_LIST). */
export const MAX_LIST = 16;

export function savedDevices() {
  return config.value?.settings?.audio?.devices ?? [];
}

/**
 * Devices in the order the strip shows them: the ones in the settings
 * first, in their order, then the rest as PipeWire lists them.
 */
export function orderDevices(devices, saved = savedDevices()) {
  const listed = saved.map((entry) => devices.find((d) => d.node === entry.node)).filter(Boolean);
  return [...listed, ...devices.filter((d) => !listed.includes(d))];
}

/** The name a device shows on the strip: yours, else PipeWire's. */
export function deviceName(device, saved = savedDevices()) {
  return saved.find((entry) => entry.node === device.node)?.name || device.name;
}

/** Apps the mixer puts first while they play, in their order. */
export function PinnedAppsField({ value, onChange }) {
  const pinned = value ?? [];
  const [data] = useAudio();
  const [name, setName] = useState("");
  const has = (app) => pinned.some((p) => p.toLowerCase() === app.toLowerCase());
  const full = pinned.length >= MAX_LIST;
  const add = (app) => {
    const clean = app.trim();
    if (clean && !has(clean) && !full) onChange([...pinned, clean]);
    setName("");
  };
  const move = (i) => {
    const next = [...pinned];
    [next[i - 1], next[i]] = [next[i], next[i - 1]];
    onChange(next);
  };
  const playing = (data?.apps ?? []).filter((app) => !has(app));
  return html`
    <div class="stack" style=${{ gap: "6px" }}>
      ${pinned.map((app, i) => html`
        <div class="row tight" key=${app}>
          <span class="grow">${app}</span>
          <button class="icon-btn" title=${t("sound.up")} aria-label=${t("sound.up")} disabled=${i === 0} onClick=${() => move(i)}><${Icon} name="chevron-up" size=${15} /></button>
          <button class="icon-btn" title=${t("common.remove")} aria-label=${t("common.remove")} onClick=${() => onChange(pinned.filter((_, j) => j !== i))}><${Icon} name="x" size=${15} /></button>
        </div>`)}
      ${playing.length > 0 && !full && html`
        <div class="row tight">
          ${playing.map((app) => html`<button class="btn small" key=${app} onClick=${() => add(app)}><${Icon} name="plus" size=${13} />${app}</button>`)}
        </div>`}
      <div class="row tight">
        <input type="text" class="grow" maxlength="64" value=${name} placeholder=${t("prop.mixer.pinned.placeholder")} disabled=${full}
          onInput=${(e) => setName(e.currentTarget.value)} onKeyDown=${(e) => { if (e.key === "Enter") add(name); }} />
        <button class="btn small" disabled=${full || !name.trim()} onClick=${() => add(name)}>${t("prop.mixer.pinned.add")}</button>
      </div>
      ${full && html`<small class="hint">${t("sound.list_full", { n: MAX_LIST })}</small>`}
    </div>`;
}

/** Outputs a button switches between, in the order the strip shows them. */
export function OutputsField({ value, onChange }) {
  const chosen = value ?? [];
  const [data, error] = useAudio();
  const present = orderDevices(data?.outputs ?? []).filter((d) => !d.virtual || chosen.includes(d.node));
  const rows = [
    ...present.map((d) => ({ node: d.node, name: deviceName(d), icon: KIND_ICON[d.kind] ?? "speaker", detail: d.detail })),
    ...chosen.filter((node) => !present.some((d) => d.node === node)).map((node) => ({
      node,
      name: savedDevices().find((entry) => entry.node === node)?.name || node,
      icon: "speaker",
      detail: t("sound.absent"),
    })),
  ];
  const full = chosen.length >= MAX_LIST;
  const toggle = (node, on) => {
    if (on && full) return;
    const next = rows.map((r) => r.node).filter((n) => (n === node ? on : chosen.includes(n)));
    onChange(next);
  };
  return html`
    <div class="stack" style=${{ gap: "4px" }}>
      ${!data && !error && html`<span class="hint">${t("common.loading")}</span>`}
      ${error && html`<small class="hint">${t("sound.unavailable")}</small>`}
      ${rows.map((r) => html`
        <label class="row tight" key=${r.node}>
          <input type="checkbox" checked=${chosen.includes(r.node)} disabled=${full && !chosen.includes(r.node)}
            onChange=${(e) => toggle(r.node, e.currentTarget.checked)} />
          <${Icon} name=${r.icon} size=${15} />
          <span class="grow">${r.name}${r.detail && html` <span class="muted">· ${r.detail}</span>`}</span>
        </label>`)}
      ${full && html`<small class="hint">${t("sound.list_full", { n: MAX_LIST })}</small>`}
      <small class="hint">${t("action.audio_output.hint")}</small>
    </div>`;
}
