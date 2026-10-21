// Editor for one gesture's action (tap, double tap or hold).
import { html, useEffect, useState } from "../lib/html.js";
import { t } from "../lib/i18n.js";
import { Icon } from "../lib/icons.js";
import { profile, page, config } from "../lib/store.js";
import { Field, TextInput, NumberInput, Select, AppField, EntityField, ArgsField, ObsField, defaultObsTarget, loadObsSources } from "./Fields.js";

export const ACTION_GROUPS = [
  ["open", ["launch_app", "open_url", "open_path"]],
  ["media", ["media", "volume", "microphone_mute"]],
  ["input", ["keyboard_shortcut", "text_input"]],
  ["strip", ["next_page", "previous_page", "go_to_page", "go_back", "switch_profile", "next_profile", "previous_profile", "brightness", "display_off"]],
  ["system", ["lock_screen", "home_assistant", "obs", "http_request", "shell_command", "sequence"]],
];

const OBS_COMMANDS = [
  "toggle_record", "start_record", "stop_record", "pause_record",
  "toggle_stream", "start_stream", "stop_stream",
  "set_scene", "toggle_mute",
  "toggle_replay_buffer", "save_replay", "toggle_virtual_cam",
];
/** Commands that name a scene or an audio source, and which list they pick from. */
const OBS_TARGETS = { set_scene: "scenes", toggle_mute: "audio_inputs" };

const DEFAULTS = {
  open_url: { url: "https://" },
  open_path: { path: "~/" },
  launch_app: { target: "default:browser" },
  media: { command: "play_pause" },
  volume: { command: "up", step: 5 },
  keyboard_shortcut: { keys: [] },
  text_input: { text: "" },
  // The server refuses an empty program, so a new action starts runnable.
  shell_command: { program: "notify-send", args: ["Nexus", "Button pressed"] },
  http_request: { method: "POST", url: "https://" },
  home_assistant: { service: "light.toggle", entity_id: "" },
  obs: { command: "toggle_record" },
  brightness: { command: "cycle" },
  sequence: { steps: [], delay_ms: 0 },
};

/**
 * Defaults that depend on the config: page and profile switches start out
 * pointing at the first other page or profile, because the server refuses
 * empty targets and one invalid action would block every later save.
 */
function defaultsFor(type) {
  if (type === "go_to_page") {
    const pages = profile.value?.pages ?? [];
    return { page_id: (pages.find((p) => p.id !== page.value?.id) ?? pages[0])?.id ?? "" };
  }
  if (type === "switch_profile") {
    const profiles = config.value?.profiles ?? [];
    return { profile_id: (profiles.find((p) => p.id !== profile.value?.id) ?? profiles[0])?.id ?? "" };
  }
  return structuredClone(DEFAULTS[type] ?? {});
}

export function ActionEditor({ value, onChange, depth = 0 }) {
  const type = value?.type ?? "";
  const set = (patch) => onChange({ ...value, ...patch });
  const choose = (next) => {
    if (!next) return onChange(null);
    onChange({ type: next, ...defaultsFor(next) });
  };
  return html`
    <div class="stack" style=${{ gap: "10px" }}>
      <select value=${type} onChange=${(e) => choose(e.currentTarget.value)} aria-label=${t("action.type")}>
        <option value="">${t("action.none")}</option>
        ${ACTION_GROUPS.map(([group, types]) => html`
          <optgroup label=${t(`action.group.${group}`)}>
            ${types.filter((x) => depth === 0 || x !== "sequence").map((x) => html`<option value=${x}>${t(`action.${x}`)}</option>`)}
          </optgroup>`)}
      </select>
      ${type && html`<${ActionFields} value=${value} set=${set} depth=${depth} />`}
    </div>`;
}

function ActionFields({ value, set, depth }) {
  const pages = profile.value?.pages ?? [];
  const profiles = config.value?.profiles ?? [];
  switch (value.type) {
    case "open_url":
      return html`<${Field} label=${t("action.url")}><${TextInput} type="url" value=${value.url} onInput=${(url) => set({ url })} /><//>`;
    case "open_path":
      return html`<${Field} label=${t("action.path")} hint=${t("action.path.hint")}><${TextInput} value=${value.path} onInput=${(path) => set({ path })} /><//>`;
    case "launch_app":
      return html`<${Field} group label=${t("action.app")}><${AppField} value=${value.target} onChange=${(target) => set({ target })} /><//>`;
    case "media":
      return html`<${Field} label=${t("action.media.command")}>
        <${Select} value=${value.command} options=${["play_pause", "next", "previous", "stop"]} labelOf=${(o) => t(`media.${o}`)} onChange=${(command) => set({ command })} />
      <//>`;
    case "volume":
      return html`<div class="two">
        <${Field} label=${t("action.volume.command")}>
          <${Select} value=${value.command} options=${["up", "down", "set", "toggle_mute"]} labelOf=${(o) => t(`volume.${o}`)} onChange=${(command) => set({ command })} />
        <//>
        ${value.command === "set"
          ? html`<${Field} label=${t("action.percent")}><${NumberInput} value=${value.percent ?? 50} min=${0} max=${150} onInput=${(percent) => set({ percent })} /><//>`
          : value.command !== "toggle_mute" && html`<${Field} label=${t("action.step")}><${NumberInput} value=${value.step ?? 5} min=${1} max=${50} onInput=${(step) => set({ step })} /><//>`}
      </div>`;
    case "keyboard_shortcut":
      return html`<${Field} group label=${t("action.shortcut")} hint=${t("action.shortcut.hint")}><${ShortcutRecorder} keys=${value.keys} onChange=${(keys) => set({ keys })} /><//>`;
    case "text_input":
      return html`<${Field} label=${t("action.text")} hint=${t("action.text.hint")}><textarea rows="2" value=${value.text} onInput=${(e) => set({ text: e.currentTarget.value })}></textarea><//>`;
    case "shell_command":
      return html`
        <p class="hint">${t("action.shell.hint")}</p>
        <${Field} label=${t("action.program")}><${TextInput} value=${value.program} onInput=${(program) => set({ program })} placeholder="notify-send" /><//>
        <${Field} label=${t("action.args")}><${ArgsField} value=${value.args} onChange=${(args) => set({ args })} /><//>`;
    case "http_request":
      return html`
        <div class="two">
          <${Field} label=${t("action.method")}><${Select} value=${value.method} options=${["GET", "POST", "PUT"]} onChange=${(method) => set({ method })} /><//>
          <${Field} label=${t("action.content.type")}><${TextInput} value=${value.content_type} optional onInput=${(c) => set({ content_type: c || undefined })} /><//>
        </div>
        <${Field} label=${t("action.url")}><${TextInput} type="url" value=${value.url} onInput=${(url) => set({ url })} /><//>
        ${value.method !== "GET" && html`<${Field} label=${t("action.body")}><textarea rows="3" value=${value.body ?? ""} onInput=${(e) => set({ body: e.currentTarget.value || undefined })}></textarea><//>`}`;
    case "home_assistant":
      return html`
        <${Field} label=${t("action.ha.service")} hint=${t("action.ha.service.hint")}><${TextInput} value=${value.service} onInput=${(service) => set({ service })} /><//>
        <${Field} label=${t("action.ha.entity")}><${EntityField} value=${value.entity_id} onChange=${(entity_id) => set({ entity_id: entity_id || undefined })} /><//>`;
    case "obs":
      return html`<${ObsFields} value=${value} set=${set} />`;
    case "go_to_page":
      return html`<${Field} label=${t("action.page")}>
        <${Select} value=${value.page_id} options=${["", ...pages.map((p) => p.id)]} labelOf=${(id) => (id ? pages.find((p) => p.id === id)?.name ?? id : t("action.page.pick"))} onChange=${(page_id) => set({ page_id })} />
      <//>`;
    case "switch_profile":
      return html`<${Field} label=${t("action.profile")}>
        <${Select} value=${value.profile_id} options=${["", ...profiles.map((p) => p.id)]} labelOf=${(id) => (id ? profiles.find((p) => p.id === id)?.name ?? id : t("action.profile.pick"))} onChange=${(profile_id) => set({ profile_id })} />
      <//>`;
    case "brightness":
      return html`<div class="two">
        <${Field} label=${t("action.brightness.command")}>
          <${Select} value=${value.command} options=${["cycle", "up", "down", "set"]} labelOf=${(o) => t(`brightness.${o}`)} onChange=${(command) => set({ command })} />
        <//>
        ${value.command === "set" && html`<${Field} label=${t("action.percent")}><${NumberInput} value=${value.percent ?? 50} min=${0} max=${100} onInput=${(percent) => set({ percent })} /><//>`}
      </div>`;
    case "sequence":
      return html`<${SequenceEditor} value=${value} set=${set} depth=${depth} />`;
    default:
      return html`<p class="hint">${t(`action.${value.type}.hint`)}</p>`;
  }
}

function ObsFields({ value, set }) {
  // OBS's names load while the command is being chosen, so a scene or mute
  // button starts with a name OBS knows.
  useEffect(() => { loadObsSources().catch(() => null); }, []);
  const kind = OBS_TARGETS[value.command];
  const choose = (command) => set({ command, target: OBS_TARGETS[command] ? defaultObsTarget(OBS_TARGETS[command]) : undefined });
  return html`
    <${Field} label=${t("action.obs.command")}>
      <${Select} value=${value.command} options=${OBS_COMMANDS} labelOf=${(o) => t(`obs.command.${o}`)} onChange=${choose} />
    <//>
    ${kind && html`<${Field} label=${t(kind === "scenes" ? "action.obs.scene" : "action.obs.source")}>
      <${ObsField} kind=${kind} value=${value.target} onChange=${(target) => set({ target: target || undefined })} />
    <//>`}`;
}

/** Steps per sequence the config allows. */
const MAX_STEPS = 16;

function SequenceEditor({ value, set, depth }) {
  const steps = value.steps ?? [];
  const update = (i, step) => set({ steps: steps.map((s, j) => (j === i ? step : s)).filter(Boolean) });
  return html`
    <div class="stack" style=${{ gap: "8px" }}>
      ${steps.map((step, i) => html`
        <div class="gesture">
          <div class="row"><b>${t("action.step.n", { n: i + 1 })}</b></div>
          <${ActionEditor} value=${step} onChange=${(s) => update(i, s)} depth=${depth + 1} />
        </div>`)}
      <div class="row">
        <button class="btn small" disabled=${steps.length >= MAX_STEPS} title=${steps.length >= MAX_STEPS ? t("action.step.max", { n: MAX_STEPS }) : ""}
          onClick=${() => set({ steps: [...steps, { type: "next_page" }] })}><${Icon} name="plus" size=${14} />${t("action.step.add")}</button>
        <span class="grow"></span>
        <label class="row tight hint">${t("action.delay")}
          <input type="number" min="0" max="10000" step="50" style=${{ width: "90px" }} value=${value.delay_ms ?? 0} onInput=${(e) => set({ delay_ms: Number(e.currentTarget.value) || 0 })} /> ms
        </label>
      </div>
    </div>`;
}

const CODE_NAMES = {
  Enter: "enter", Escape: "esc", Tab: "tab", Space: "space", Backspace: "backspace", Delete: "delete",
  Home: "home", End: "end", PageUp: "pageup", PageDown: "pagedown",
  ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down",
};

/** Records a key combination (modifiers plus one key) as input-helper names. */
export function ShortcutRecorder({ keys, onChange }) {
  const [recording, setRecording] = useState(false);
  const onKeyDown = (e) => {
    if (!recording) return;
    e.preventDefault();
    if (["Control", "Shift", "Alt", "Meta", "AltGraph"].includes(e.key)) return;
    let key = CODE_NAMES[e.code];
    if (!key && /^Key[A-Z]$/.test(e.code)) key = e.code.slice(3).toLowerCase();
    if (!key && /^Digit\d$/.test(e.code)) key = e.code.slice(5);
    if (!key && /^F\d{1,2}$/.test(e.code)) key = e.code.toLowerCase();
    if (!key && e.key.length === 1) key = e.key.toLowerCase();
    if (!key) return;
    const combo = [e.ctrlKey && "ctrl", e.altKey && "alt", e.shiftKey && "shift", e.metaKey && "super", key].filter(Boolean);
    onChange(combo);
    setRecording(false);
  };
  return html`
    <button
      class=${`btn ${recording ? "primary" : ""}`}
      style=${{ justifyContent: "center" }}
      onClick=${() => setRecording(true)}
      onKeyDown=${onKeyDown}
      onBlur=${() => setRecording(false)}
    >
      ${recording ? t("action.shortcut.press") : keys?.length ? keys.map((k) => k[0].toUpperCase() + k.slice(1)).join(" + ") : t("action.shortcut.record")}
    </button>`;
}
