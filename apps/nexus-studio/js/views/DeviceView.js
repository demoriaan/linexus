// Device: connection and daemon state, how the strip behaves, setup checks
// and the event log.
import { html, useEffect, useState } from "../lib/html.js";
import { api } from "../lib/api.js";
import { t, hasText } from "../lib/i18n.js";
import { Icon } from "../lib/icons.js";
import { config, edit, daemon, health, refreshDaemon, refreshHealth, toast } from "../lib/store.js";
import { Switch, Segmented } from "../components/Dialog.js";
import { Field } from "../components/Fields.js";

export function DeviceView() {
  useEffect(() => { refreshHealth(); }, []);
  return html`
    <div class="view">
      <div class="view-head"><div><h1>${t("nav.device")}</h1><p>${t("device.lead")}</p></div></div>
      <div class="cols">
        <div class="stack" style=${{ gap: "18px" }}>
          <${Status} />
          <${Behaviour} />
        </div>
        <div class="stack" style=${{ gap: "18px" }}>
          <${Checks} />
          <${EventLog} />
        </div>
      </div>
    </div>`;
}

const call = (path, body, done) => api.post(path, body ?? {}).then(() => { refreshDaemon(); if (done) toast(done, "ok"); }).catch((e) => toast(e.message, "error"));

function Status() {
  const d = daemon.value;
  const s = d.status;
  const [brightness, setBrightness] = useState(null);
  const autostart = health.value?.daemon?.autostart;
  const rows = s
    ? [
        [t("device.state"), t(`device.state.${s.device.state}`)],
        [t("device.path"), s.device.path ?? s.device.error ?? "—"],
        [t("device.profile"), s.profile_name],
        [t("device.page"), `${s.page_name} (${s.page_index + 1}/${s.page_count})`],
      ]
    : [];
  return html`
    <section class="panel">
      <header><${Icon} name="monitor" /><h3>${t("device.status")}</h3>
        <span class=${`dot ${d.reachable ? (s?.device.state === "connected" ? "ok" : "warn") : "bad"}`}></span></header>
      ${!d.reachable && html`<p class="hint" style=${{ marginBottom: "12px" }}>${t("device.daemon.off")}</p>`}
      ${rows.length > 0 && html`<div class="list" style=${{ marginBottom: "12px" }}>${rows.map(([k, v]) => html`
        <div class="list-row"><span class="grow muted">${k}</span><span>${v}</span></div>`)}</div>`}
      ${s && html`
        <${Field} label=${t("device.brightness", { n: brightness ?? s.brightness_percent })}>
          <input type="range" min="0" max="100" step="5" value=${brightness ?? s.brightness_percent}
            onInput=${(e) => { const percent = Number(e.currentTarget.value); setBrightness(percent); api.post("/api/daemon/brightness", { percent }).catch(() => {}); }}
            onChange=${() => setTimeout(() => setBrightness(null), 2500)} />
        <//>
        <div class="row" style=${{ marginTop: "10px" }}>
          ${s.manual_blanked
            ? html`<button class="btn" onClick=${() => call("/api/daemon/restore")}><${Icon} name="eye" size=${16} />${t("device.restore")}</button>`
            : html`<button class="btn" onClick=${() => call("/api/daemon/blank")}><${Icon} name="eye-off" size=${16} />${t("device.blank")}</button>`}
        </div>`}
      <hr class="divider" style=${{ margin: "14px 0" }} />
      <div class="row">
        ${d.reachable
          ? html`<button class="btn" onClick=${() => call("/api/daemon/restart", {}, t("device.restarted"))}><${Icon} name="refresh-cw" size=${16} />${t("device.restart")}</button>
                 <button class="btn ghost" onClick=${() => call("/api/daemon/stop", {}, t("device.stopped"))}><${Icon} name="power" size=${16} />${t("device.stop")}</button>`
          : html`<button class="btn primary" onClick=${() => call("/api/daemon/start", {}, t("device.started"))}><${Icon} name="play" size=${16} />${t("device.start")}</button>`}
        <span class="grow"></span>
        ${autostart !== undefined && html`<${Switch} checked=${autostart} label=${t("device.autostart")}
          onChange=${(enabled) => api.post("/api/daemon/autostart", { enabled }).then(refreshHealth).catch((e) => toast(e.message, "error"))} />`}
      </div>
    </section>`;
}

function Behaviour() {
  const settings = config.value.settings;
  const idle = settings.idle;
  const setIdle = (patch, coalesce) => edit((d) => Object.assign(d.settings.idle, patch), { coalesce });
  const setSettings = (patch) => edit((d) => Object.assign(d.settings, patch));
  return html`
    <section class="panel">
      <header><${Icon} name="sparkles" /><h3>${t("behaviour.title")}</h3></header>
      <div class="stack">
        <div class="field"><span>${t("behaviour.transition")}</span>
          <${Segmented} value=${settings.transition} onChange=${(transition) => setSettings({ transition })}
            options=${["slide", "fade", "none"].map((v) => ({ value: v, label: t(`behaviour.transition.${v}`) }))} />
        </div>
        <div class="field"><span>${t("behaviour.locale")}</span>
          <${Segmented} value=${settings.locale} onChange=${(locale) => setSettings({ locale })}
            options=${[["auto", t("common.auto")], ["en", "English"], ["de", "Deutsch"], ["fr", "Français"]].map(([value, label]) => ({ value, label }))} />
        </div>
        <div class="two">
          <div class="field"><span>${t("behaviour.clock")}</span>
            <${Segmented} value=${settings.hour12 ? "12" : "24"} onChange=${(v) => setSettings({ hour12: v === "12" })}
              options=${[{ value: "24", label: "24 h" }, { value: "12", label: "12 h" }]} />
          </div>
          <div class="field"><span>${t("behaviour.temperature")}</span>
            <${Segmented} value=${settings.temperature_unit} onChange=${(temperature_unit) => setSettings({ temperature_unit })}
              options=${[{ value: "celsius", label: "°C" }, { value: "fahrenheit", label: "°F" }]} />
          </div>
        </div>
        <hr class="divider" />
        <${Field} label=${idle.dim_after_minutes ? t("behaviour.dim.after", { n: idle.dim_after_minutes }) : t("behaviour.dim.never")}>
          <input type="range" min="0" max="60" step="1" value=${idle.dim_after_minutes} onInput=${(e) => setIdle({ dim_after_minutes: Number(e.currentTarget.value) }, "dim")} />
        <//>
        ${idle.dim_after_minutes > 0 && html`
          <${Field} label=${t("behaviour.dim.level", { n: idle.dim_percent })}>
            <input type="range" min="0" max="80" step="5" value=${idle.dim_percent} onInput=${(e) => setIdle({ dim_percent: Number(e.currentTarget.value) }, "dimp")} />
          <//>
          <div class="field inline"><span>${t("behaviour.wake")}</span><${Switch} ariaLabel=${t("behaviour.wake")} checked=${idle.wake_touch_only} onChange=${(wake_touch_only) => setIdle({ wake_touch_only })} /></div>`}
        <${Field} label=${idle.off_after_minutes ? t("behaviour.off.after", { n: idle.off_after_minutes }) : t("behaviour.off.never")} hint=${t("behaviour.off.hint")}>
          <input type="range" min="0" max="120" step="5" value=${idle.off_after_minutes ?? 0} onInput=${(e) => setIdle({ off_after_minutes: Number(e.currentTarget.value) }, "off")} />
        <//>
        <div class="field inline"><span>${t("behaviour.lock")}</span><${Switch} ariaLabel=${t("behaviour.lock")} checked=${idle.blank_when_locked} onChange=${(blank_when_locked) => setIdle({ blank_when_locked })} /></div>
        <${TouchCorrection} />
      </div>
    </section>`;
}

/**
 * How far the panel reads touches from the finger: measured by the
 * calibration (marks on the strip to touch one after the other) or set by
 * hand. The daemon applies it to the strip's own touches only.
 */
function TouchCorrection() {
  const touch = { left: 0, right: 0, ...config.value.settings.touch };
  const set = (patch, coalesce) => edit((d) => { d.settings.touch = { ...touch, ...d.settings.touch, ...patch }; }, { coalesce });
  const [calibration, setCalibration] = useState(null);
  const call = async (action) => {
    const data = await api.post("/api/daemon/calibrate", { action });
    setCalibration(data.daemon?.calibration ?? null);
  };
  const running = calibration?.state === "running";
  // Follow the strip while marks are being touched.
  useEffect(() => {
    if (!running) return undefined;
    const timer = setInterval(() => call("status").catch(() => setCalibration(null)), 400);
    return () => clearInterval(timer);
  }, [running]);
  // Leaving the page ends a calibration nobody watches any more.
  useEffect(() => () => { api.post("/api/daemon/calibrate", { action: "cancel" }).catch(() => {}); }, []);
  const start = () => call("start").catch((e) => toast(e.message, "error"));
  const signed = (n) => (n > 0 ? `+${n}` : `${n}`);
  const slider = (key) => html`
    <${Field} label=${t(`touch.${key}`, { n: touch[key] > 0 ? `+${touch[key]}` : touch[key] })}>
      <input type="range" min="-100" max="100" step="1" value=${touch[key]} onInput=${(e) => set({ [key]: Number(e.currentTarget.value) }, `touch-${key}`)} />
    <//>`;
  const done = calibration?.state === "done";
  const exact = done && calibration.left === 0 && calibration.right === 0;
  return html`
    <hr class="divider" />
    <div class="field"><span>${t("touch.title")}</span><small>${t("touch.hint")}</small></div>
    <div class="row">
      ${running
        ? html`<span class="hint">${t("touch.calibrating", { n: Math.min(calibration.step + 1, calibration.steps), total: calibration.steps })}</span>
            <button class="btn small ghost" onClick=${() => call("cancel").catch(() => setCalibration(null))}>${t("common.cancel")}</button>`
        : html`<button class="btn small primary" onClick=${start}><${Icon} name="crosshair" size=${14} />${t("touch.calibrate")}</button>`}
    </div>
    ${done && html`
      <div class="row">
        <span class="ok-text">${exact
          ? t("touch.accurate")
          : t("touch.measured", { left: signed(calibration.left), right: signed(calibration.right) })}</span>
        ${(calibration.left !== touch.left || calibration.right !== touch.right) && html`
          <button class="btn small primary" onClick=${() => { set({ left: calibration.left, right: calibration.right }); setCalibration(null); toast(t("touch.applied"), "ok"); }}>${t("touch.apply")}</button>`}
        <button class="btn small ghost" onClick=${() => setCalibration(null)}>${t("touch.discard")}</button>
      </div>`}
    ${calibration?.state === "failed" && html`<span class="bad-text">${t("touch.failed")}</span>`}
    ${calibration?.state === "cancelled" && html`<span class="hint">${t("touch.cancelled")}</span>`}
    <div class="two">${slider("left")}${slider("right")}</div>
    ${(touch.left || touch.right) ? html`<div><button class="btn small ghost" onClick=${() => set({ left: 0, right: 0 })}>${t("touch.reset")}</button></div>` : null}`;
}

/** A check's text in the UI language, or the daemon's English one. */
function checkText(c, part) {
  const key = `health.${c.key}.${part}`;
  if (!c.key || !hasText(key)) return c[part];
  const program = c.args?.program;
  return t(key, { ...c.args, purpose: program && hasText(`health.purpose.${program}`) ? t(`health.purpose.${program}`) : "" });
}

function Checks() {
  const checks = health.value?.checks ?? [];
  const copy = (text) => navigator.clipboard?.writeText(text).then(() => toast(t("common.copied"), "ok"));
  return html`
    <section class="panel">
      <header><${Icon} name="circle-check" /><h3>${t("checks.title")}</h3>
        <button class="icon-btn" onClick=${refreshHealth} aria-label=${t("common.refresh")}><${Icon} name="refresh-cw" /></button></header>
      ${!checks.length && html`<p class="hint">${t("common.loading")}</p>`}
      ${checks.map((c) => html`
        <div class="check">
          <span class=${`dot ${c.state === "ok" ? "ok" : c.state === "warn" ? "warn" : "bad"}`}></span>
          <b>${checkText(c, "title")}</b>
          ${c.detail && html`<p>${checkText(c, "detail")}</p>`}
          ${c.fix && html`<pre onClick=${() => copy(c.fix)} title=${t("common.copy")}>${c.fix}</pre>`}
        </div>`)}
    </section>`;
}

function EventLog() {
  const [events, setEvents] = useState(null);
  const load = () => api.get("/api/events?limit=120").then((d) => setEvents(d.events ?? d.records ?? [])).catch(() => setEvents([]));
  useEffect(() => { load(); }, []);
  const time = (ts) => {
    const d = new Date(typeof ts === "number" ? ts * (ts < 1e12 ? 1000 : 1) : ts);
    return Number.isNaN(d.getTime()) ? "" : d.toLocaleTimeString();
  };
  return html`
    <section class="panel">
      <header><${Icon} name="activity" /><h3>${t("events.title")}</h3>
        <button class="icon-btn" onClick=${load} aria-label=${t("common.refresh")}><${Icon} name="refresh-cw" /></button></header>
      <div class="log">
        ${(events ?? []).slice().reverse().map((e) => html`
          <div>
            <time>${time(e.ts ?? e.timestamp ?? e.time)}</time>
            <span class=${`lvl-${e.level}`}>${e.level}</span>
            <span>${e.message}${e.fields && Object.keys(e.fields).length ? html` <span class="muted">${Object.entries(e.fields).map(([k, v]) => `${k}=${v}`).join(" ")}</span>` : ""}</span>
          </div>`)}
        ${events && !events.length && html`<p class="hint">${t("events.empty")}</p>`}
      </div>
    </section>`;
}
