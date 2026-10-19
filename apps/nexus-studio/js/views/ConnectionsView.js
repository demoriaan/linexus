// Connections: services the strip reads from or talks to — Home Assistant,
// weather, desktop notifications, and incoming/outgoing webhooks.
import { html, useEffect, useState } from "../lib/html.js";
import { api } from "../lib/api.js";
import { t } from "../lib/i18n.js";
import { Icon } from "../lib/icons.js";
import { config, edit, toast, uniqueId } from "../lib/store.js";
import { sensors, loadSensors, saveSection } from "../lib/sensors.js";
import { Switch, ConfirmButton } from "../components/Dialog.js";
import { Field, TextInput, LocationField } from "../components/Fields.js";

export function ConnectionsView() {
  useEffect(() => { loadSensors(); }, []);
  return html`
    <div class="view">
      <div class="view-head"><div><h1>${t("nav.connections")}</h1><p>${t("conn.lead")}</p></div></div>
      <div class="cols">
        <div class="stack" style=${{ gap: "18px" }}>
          <${HomeAssistant} />
          <${Weather} />
        </div>
        <div class="stack" style=${{ gap: "18px" }}>
          <${Notifications} />
          <${Webhooks} />
        </div>
      </div>
      <div class="cols" style=${{ marginTop: "18px" }}>
        <${OutgoingList} kind="mqtt" />
        <${OutgoingList} kind="ifttt" />
      </div>
    </div>`;
}

function HomeAssistant() {
  const saved = sensors.value?.config?.home_assistant_server ?? null;
  const [url, setUrl] = useState(saved?.url ?? "");
  const [token, setToken] = useState(saved?.token ?? "");
  const [status, setStatus] = useState(null);
  useEffect(() => {
    setUrl(saved?.url ?? "");
    setToken(saved?.token ?? "");
  }, [saved?.url, saved?.token]);
  const test = async () => {
    setStatus({ busy: true });
    try {
      const data = await api.post("/api/home-assistant/test", { url, token });
      setStatus({ ok: t("ha.test.ok", { n: data.entities }) });
    } catch (error) {
      setStatus({ error: error.message });
    }
  };
  const save = async () => {
    try {
      await api.post("/api/home-assistant/server", url ? { url, token } : null);
      await loadSensors();
      toast(t("ha.saved"), "ok");
    } catch (error) {
      toast(error.message, "error");
    }
  };
  return html`
    <section class="panel">
      <header><${Icon} name="house" /><h3>Home Assistant</h3>${saved && html`<span class="dot ok" title=${t("ha.connected")}></span>`}</header>
      <p class="hint" style=${{ marginBottom: "12px" }}>${t("ha.lead")}</p>
      <div class="stack">
        <${Field} label=${t("ha.url")}><${TextInput} type="url" value=${url} placeholder="http://homeassistant.local:8123" onInput=${setUrl} /><//>
        <${Field} label=${t("ha.token")} hint=${t("ha.token.hint")}><${TextInput} type="password" value=${token} onInput=${setToken} /><//>
        <div class="row">
          <button class="btn" onClick=${test} disabled=${!url || !token}>${t("ha.test")}</button>
          <button class="btn primary" onClick=${save}>${t("common.save")}</button>
          ${status?.ok && html`<span class="ok-text">${status.ok}</span>`}
          ${status?.error && html`<span class="bad-text">${status.error}</span>`}
        </div>
      </div>
    </section>`;
}

function Weather() {
  const settings = config.value.settings;
  return html`
    <section class="panel">
      <header><${Icon} name="cloud-sun" /><h3>${t("weather.title")}</h3></header>
      <p class="hint" style=${{ marginBottom: "12px" }}>${t("weather.lead")}</p>
      <${LocationField} value=${settings.weather} onChange=${(weather) => edit((d) => { if (weather) d.settings.weather = weather; else delete d.settings.weather; })} />
      <p class="hint" style=${{ marginTop: "12px" }}>${t("weather.credit")} <a href="https://open-meteo.com/" target="_blank" rel="noreferrer">Open-Meteo</a> (CC BY 4.0)</p>
    </section>`;
}

function Notifications() {
  const n = config.value.settings.notifications;
  const o = config.value.settings.overlays;
  const set = (patch, coalesce) => edit((d) => Object.assign(d.settings.notifications, patch), { coalesce });
  const setOverlay = (patch) => edit((d) => Object.assign(d.settings.overlays, patch));
  return html`
    <section class="panel">
      <header><${Icon} name="bell" /><h3>${t("notify.title")}</h3></header>
      <div class="stack">
        <div class="field inline"><span>${t("notify.enabled")}</span><${Switch} ariaLabel=${t("notify.enabled")} checked=${n.enabled} onChange=${(enabled) => set({ enabled })} /></div>
        <small class="hint" style=${{ marginTop: "-8px" }}>${t("notify.enabled.hint")}</small>
        <div class="field inline"><span>${t("notify.private")}</span><${Switch} ariaLabel=${t("notify.private")} checked=${n.private} onChange=${(v) => set({ private: v })} /></div>
        <${Field} label=${t("notify.duration", { s: (n.duration_ms / 1000).toFixed(1) })}>
          <input type="range" min="2000" max="15000" step="500" value=${n.duration_ms} onInput=${(e) => set({ duration_ms: Number(e.currentTarget.value) }, "dur")} />
        <//>
        <${Field} label=${t("notify.muted")} hint=${t("notify.muted.hint")}>
          <${TextInput} value=${n.muted_apps.join(", ")} onInput=${(v) => set({ muted_apps: v.split(",").map((s) => s.trim()).filter(Boolean) }, "muted")} />
        <//>
        <hr class="divider" />
        <div class="field inline"><span>${t("overlay.volume")}</span><${Switch} ariaLabel=${t("overlay.volume")} checked=${o.volume} onChange=${(volume) => setOverlay({ volume })} /></div>
        <div class="field inline"><span>${t("overlay.track")}</span><${Switch} ariaLabel=${t("overlay.track")} checked=${o.now_playing} onChange=${(now_playing) => setOverlay({ now_playing })} /></div>
        <div><button class="btn small" onClick=${() => api.post("/api/daemon/notify", { text: t("notify.test.text"), icon: "bell" }).catch((e) => toast(e.message, "error"))}>${t("notify.test")}</button></div>
      </div>
    </section>`;
}

/**
 * Incoming webhooks. Studio only stores deliveries for names listed in
 * sensors.json, sent by programs on this computer (web pages are refused),
 * so a name has to be added here before anything can post to it.
 */
function Webhooks() {
  const records = sensors.value?.webhook_records ?? [];
  const hooks = sensors.value?.config?.webhooks ?? [];
  const [name, setName] = useState("");
  const base = `${location.origin}/api/sensors/webhook/`;
  // Both work on the freshly stored list, so names added in another tab stay.
  const register = async (id) => {
    try {
      await saveSection("webhooks", (stored) => (stored.some((h) => h.id === id) ? stored : [...stored, { id }]));
    } catch (error) {
      toast(error.message, "error");
    }
  };
  const add = async () => {
    if (!name.trim()) return;
    await register(uniqueId(name, new Set()));
    setName("");
  };
  const forget = async (id) => {
    try {
      await saveSection("webhooks", (stored) => stored.filter((h) => h.id !== id));
    } catch (error) {
      toast(error.message, "error");
    }
  };
  const removeRecord = async (id) => {
    try {
      await api.del(`/api/sensors/webhook/${encodeURIComponent(id)}`);
      await loadSensors();
    } catch (error) {
      toast(error.message, "error");
    }
  };
  // Goes through the API client, whose token marks it as Studio's own.
  const sendTest = async (id) => {
    try {
      await api.post(`/api/sensors/webhook/${encodeURIComponent(id)}`, { title: t("webhook.test.title"), url: "https://example.com" });
      await loadSensors();
      toast(t("webhook.test.sent"), "ok");
    } catch (error) {
      toast(error.message, "error");
    }
  };
  return html`
    <section class="panel">
      <header><${Icon} name="link" /><h3>${t("webhook.title")}</h3></header>
      <p class="hint">${t("webhook.lead")}</p>
      <div class="row tight" style=${{ margin: "10px 0" }}>
        <input type="text" class="grow" placeholder=${t("webhook.name")} aria-label=${t("webhook.name")} value=${name}
          onInput=${(e) => setName(e.currentTarget.value)} onKeyDown=${(e) => e.key === "Enter" && add()} />
        <button class="btn small" onClick=${add} disabled=${!name.trim()}><${Icon} name="plus" size=${14} />${t("common.add")}</button>
      </div>
      <div class="list">
        ${hooks.map((h) => html`
          <div class="list-row">
            <div class="grow"><div class="title">${h.id}</div><div class="meta">${base}${h.id}</div></div>
            <button class="btn small" onClick=${() => sendTest(h.id)}>${t("webhook.test")}</button>
            <${ConfirmButton} class="btn small ghost danger" title=${t("common.remove")} onConfirm=${() => forget(h.id)}><${Icon} name="trash-2" size=${15} /><//>
          </div>`)}
        ${!hooks.length && html`<p class="hint">${t("webhook.none")}</p>`}
      </div>
      <pre class="code"><code>curl -X POST ${base}${hooks[0]?.id ?? "build"} -H 'Content-Type: application/json' -d '{"title":"Build passed","url":"https://ci.example"}'</code></pre>
      <h4 style=${{ margin: "14px 0 6px" }}>${t("webhook.received")}</h4>
      <div class="list">
        ${records.map((r) => html`
          <div class="list-row">
            <div class="grow"><div class="title">${r.label}</div><div class="meta">${JSON.stringify(r.value).slice(0, 140)}</div></div>
            ${!hooks.some((h) => h.id === r.label) && html`
              <button class="btn small" title=${t("webhook.register.hint")} onClick=${() => register(r.label)}>${t("webhook.register")}</button>`}
            <${ConfirmButton} class="btn small ghost danger" title=${t("common.delete")} onConfirm=${() => removeRecord(r.label)}><${Icon} name="trash-2" size=${15} /><//>
          </div>`)}
        ${!records.length && html`<p class="hint">${t("webhook.empty")}</p>`}
      </div>
    </section>`;
}

const OUTGOING = {
  mqtt: {
    icon: "wifi",
    fields: [
      ["id", "text"], ["host", "text"], ["port", "number"], ["topic", "text"], ["username", "text"],
      ["password", "password"], ["payload_template", "text"], ["qos", "number"], ["retain", "toggle"],
      ["home_assistant_discovery", "toggle"],
    ],
    blank: { id: "broker", host: "localhost", port: 1883, topic: "nexus/card", client_id: "nexus-studio", username: "", password: "", payload_template: "{title}", retain: false, qos: 0, home_assistant_discovery: false },
  },
  ifttt: {
    icon: "zap",
    fields: [["id", "text"], ["event", "text"], ["key", "password"], ["value1_template", "text"], ["value2_template", "text"], ["value3_template", "text"]],
    blank: { id: "phone", event: "nexus_alert", key: "", value1_template: "{title}", value2_template: "{source}", value3_template: "{url}" },
  },
};

function OutgoingList({ kind }) {
  const spec = OUTGOING[kind];
  const stored = sensors.value?.config?.[kind] ?? [];
  const [items, setItems] = useState(stored);
  const [dirty, setDirty] = useState(false);
  useEffect(() => { if (!dirty) setItems(stored); }, [JSON.stringify(stored)]);
  const set = (i, key, value) => {
    setItems(items.map((it, j) => (j === i ? { ...it, [key]: value } : it)));
    setDirty(true);
  };
  const save = async () => {
    try {
      await saveSection(kind, items);
      setDirty(false);
      toast(t("common.saved"), "ok");
    } catch (error) {
      toast(error.message, "error");
    }
  };
  const test = async (id) => {
    try {
      if (dirty) await save();
      const data = await api.post(`/api/sensors/integrations/${kind}/${encodeURIComponent(id)}/test`, {});
      toast(data.message ?? t("common.done"), "ok");
    } catch (error) {
      toast(error.message, "error");
    }
  };
  return html`
    <section class="panel">
      <header><${Icon} name=${spec.icon} /><h3>${t(`${kind}.title`)}</h3>
        <button class="btn small" onClick=${() => { setItems([...items, structuredClone(spec.blank)]); setDirty(true); }}><${Icon} name="plus" size=${14} />${t("common.add")}</button>
        ${dirty && html`<button class="btn small primary" onClick=${save}>${t("common.save")}</button>`}
      </header>
      <p class="hint" style=${{ marginBottom: "10px" }}>${t(`${kind}.lead`)}</p>
      ${items.map((item, i) => html`
        <div class="gesture" style=${{ marginBottom: "10px" }}>
          <div class="two">
            ${spec.fields.map(([key, type]) => type === "toggle"
              ? html`<div class="field inline"><span>${t(`${kind}.${key}`)}</span><${Switch} checked=${item[key]} onChange=${(v) => set(i, key, v)} /></div>`
              : html`<${Field} label=${t(`${kind}.${key}`)}>
                  <input type=${type} value=${item[key] ?? ""} onInput=${(e) => set(i, key, type === "number" ? Number(e.currentTarget.value) : e.currentTarget.value)} />
                <//>`)}
          </div>
          <div class="row">
            <button class="btn small" onClick=${() => test(item.id)}>${t("common.test")}</button>
            <span class="grow"></span>
            <${ConfirmButton} onConfirm=${() => { setItems(items.filter((_, j) => j !== i)); setDirty(true); }}>${t("common.remove")}<//>
          </div>
        </div>`)}
      ${!items.length && html`<p class="hint">${t(`${kind}.empty`)}</p>`}
    </section>`;
}
