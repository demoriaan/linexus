// Sound: the privacy badge, and PipeWire's outputs and microphones with
// your own names, which of them the strip shows, and in what order.
import { html } from "../lib/html.js";
import { t } from "../lib/i18n.js";
import { Icon } from "../lib/icons.js";
import { config, edit } from "../lib/store.js";
import { Switch, ConfirmButton } from "../components/Dialog.js";
import { useAudio, orderDevices, savedDevices, KIND_ICON } from "../components/SoundFields.js";

/**
 * Keeps the settings entry of a device lean: no empty name, no default
 * hiding. A virtual device shows while it is in use unless hidden by
 * name, so hiding one is always kept, in use or not.
 */
function entryOf(row) {
  const entry = { node: row.node };
  if (row.name?.trim()) entry.name = row.name;
  if (row.hidden !== undefined && (row.hidden === true || row.hidden !== !!row.virtual)) entry.hidden = row.hidden;
  return entry;
}

const plain = (row) => Object.keys(entryOf(row)).length === 1;

/**
 * The rows that need an entry: the strip shows listed devices first and
 * the rest as PipeWire lists them, so a tail of plain rows already in that
 * order needs none.
 */
function listed(rows, devices) {
  for (let k = 0; k < rows.length; k += 1) {
    const head = new Set(rows.slice(0, k).map((r) => r.node));
    const rest = devices.filter((d) => !head.has(d.node));
    if (rows.slice(k).every((r, i) => plain(r) && r.node === rest[i]?.node)) return rows.slice(0, k);
  }
  return rows;
}

export function SoundPanel() {
  const [data, error, reload] = useAudio();
  const privacy = config.value.settings.overlays.privacy ?? true;
  const saved = savedDevices();
  const rowsOf = (devices, inUse) => orderDevices(devices, saved).map((d) => {
    const entry = saved.find((e) => e.node === d.node);
    return { node: d.node, name: entry?.name, hidden: entry?.hidden, virtual: d.virtual, inUse: d.node === inUse, device: d };
  });
  const outputs = rowsOf(data?.outputs ?? [], data?.default_output);
  const inputs = rowsOf(data?.inputs ?? [], data?.default_input);
  const known = new Set([...outputs, ...inputs].map((r) => r.node));
  const absent = data ? saved.filter((e) => !known.has(e.node)) : [];

  /** Writes the devices back in the order shown, see `listed`. */
  const save = (nextOutputs, nextInputs, coalesce) => {
    const devices = [...listed(nextOutputs, data.outputs), ...listed(nextInputs, data.inputs)].map(entryOf);
    const kept = absent.filter((e) => !devices.some((d) => d.node === e.node));
    edit((draft) => {
      const all = [...devices, ...kept];
      if (all.length) draft.settings.audio = { ...draft.settings.audio, devices: all };
      else if (draft.settings.audio) delete draft.settings.audio.devices;
    }, { coalesce });
  };
  const change = (rows, i, patch, coalesce) => {
    const next = rows.map((r, j) => (j === i ? { ...r, ...patch } : r));
    if (rows === outputs) save(next, inputs, coalesce);
    else save(outputs, next, coalesce);
  };
  const move = (rows, i) => {
    const next = [...rows];
    [next[i - 1], next[i]] = [next[i], next[i - 1]];
    if (rows === outputs) save(next, inputs);
    else save(outputs, next);
  };
  const forget = (node) => edit((draft) => {
    draft.settings.audio.devices = draft.settings.audio.devices.filter((e) => e.node !== node);
  });

  const section = (title, rows, inUse) => html`
    <h4 class="hint" style=${{ margin: "6px 0 0", textTransform: "uppercase", letterSpacing: "0.06em" }}>${title}</h4>
    <div class="list">
      ${!rows.length && html`<span class="hint">${t("sound.none")}</span>`}
      ${rows.map((r, i) => {
        const shown = !(r.hidden ?? (r.virtual && !r.inUse));
        return html`
          <div class="list-row" key=${r.node}>
            <${Icon} name=${KIND_ICON[r.device.kind] ?? "speaker"} />
            <div class="grow stack" style=${{ gap: "4px" }}>
              <input type="text" maxlength="64" value=${r.name ?? ""} placeholder=${r.device.name} aria-label=${r.device.name}
                onInput=${(e) => change(rows, i, { name: e.currentTarget.value }, `sound-name:${r.node}`)} />
              <div class="meta" title=${r.node}>${[r.device.detail, r.node === inUse && t("sound.in_use")].filter(Boolean).join(" · ") || r.node}</div>
            </div>
            <button class="icon-btn" title=${t("sound.up")} aria-label=${t("sound.up")} disabled=${i === 0} onClick=${() => move(rows, i)}><${Icon} name="chevron-up" size=${15} /></button>
            <${Switch} ariaLabel=${t("sound.shown")} checked=${shown} onChange=${(on) => change(rows, i, { hidden: !on })} />
          </div>`;
      })}
    </div>`;

  return html`
    <section class="panel">
      <header>
        <${Icon} name="speaker" /><h3>${t("sound.title")}</h3>
        <span class="grow"></span>
        <button class="icon-btn" title=${t("sound.refresh")} aria-label=${t("sound.refresh")} onClick=${reload}><${Icon} name="refresh-cw" size=${15} /></button>
      </header>
      <div class="stack">
        <div class="field inline"><span>${t("sound.privacy")}</span>
          <${Switch} ariaLabel=${t("sound.privacy")} checked=${privacy} onChange=${(on) => edit((d) => { d.settings.overlays.privacy = on; })} /></div>
        <small class="hint" style=${{ marginTop: "-8px" }}>${t("sound.privacy.hint")}</small>
        <hr class="divider" />
        <p class="hint">${t("sound.lead")}</p>
        ${error && html`<span class="bad-text">${t("sound.unavailable")}</span>`}
        ${!data && !error && html`<span class="hint">${t("common.loading")}</span>`}
        ${data && section(t("sound.outputs"), outputs, data.default_output)}
        ${data && section(t("sound.inputs"), inputs, data.default_input)}
        ${absent.length > 0 && html`
          <h4 class="hint" style=${{ margin: "6px 0 0", textTransform: "uppercase", letterSpacing: "0.06em" }}>${t("sound.absent")}</h4>
          <div class="list">
            ${absent.map((e) => html`
              <div class="list-row" key=${e.node}>
                <${Icon} name="unplug" />
                <div class="grow"><div class="title">${e.name || e.node}</div><div class="meta">${e.node}</div></div>
                <${ConfirmButton} class="btn small ghost" title=${t("sound.forget")} onConfirm=${() => forget(e.node)}>${t("sound.forget")}<//>
              </div>`)}
          </div>`}
        <small class="hint">${t("sound.meter.hint")}</small>
      </div>
    </section>`;
}
