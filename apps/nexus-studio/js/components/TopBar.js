// Top bar: profile being edited, save state, undo/redo, command palette and
// the strip's connection state.
import { html, useState } from "../lib/html.js";
import { t, lang } from "../lib/i18n.js";
import { Icon } from "../lib/icons.js";
import { api } from "../lib/api.js";
import {
  config, profile, selectedProfileId, selectedPageId, selectedWidgetId, saveState, saveError,
  canUndo, canRedo, undo, redo, daemon, edit, uniqueId, toast, conflict, overwrite, loadConfig,
  draftOffer, restoreDraft, discardDraft,
} from "../lib/store.js";
import { Popover, Dialog, ConfirmButton } from "./Dialog.js";
import { openPalette } from "./CommandPalette.js";

function SaveIndicator() {
  const state = saveState.value;
  const map = {
    saved: ["circle-check", t("save.saved"), "ok-text"],
    pending: ["pencil", t("save.pending"), "muted"],
    saving: ["loader", t("save.saving"), "muted"],
    error: ["alert-triangle", t("save.error"), "bad-text"],
    conflict: ["alert-triangle", t("save.conflict"), "bad-text"],
  };
  const [icon, label, cls] = map[state];
  return html`<span class=${`save-state row tight ${cls}`} title=${state === "error" || state === "conflict" ? saveError.value : label} aria-label=${label} role="status">
    <${Icon} name=${icon} size=${15} /><span class="save-label">${label}</span>
  </span>`;
}

/**
 * The file changed elsewhere since this tab loaded it: reload or overwrite.
 * It opens on its own, usually mid-typing, so no button gets the focus.
 */
function ConflictDialog() {
  if (!conflict.value) return null;
  return html`
    <${Dialog} title=${t("conflict.title")} onClose=${() => (conflict.value = null)} focusFirst=${false}
      footer=${html`<button class="btn" onClick=${loadConfig}>${t("conflict.reload")}</button>
        <button class="btn primary" onClick=${overwrite}>${t("conflict.overwrite")}</button>`}>
      <p>${t("conflict.lead")}</p>
    <//>`;
}

/**
 * Unsaved edits from an earlier session, found on start. Only the two
 * buttons close it, so the draft is never thrown away by accident.
 */
function DraftDialog() {
  const draft = draftOffer.value;
  if (!draft) return null;
  const time = new Date(draft.savedAt).toLocaleString(lang.value);
  return html`
    <${Dialog} title=${t("draft.title")} focusFirst=${false}
      footer=${html`<button class="btn" onClick=${discardDraft}>${t("draft.discard")}</button>
        <button class="btn primary" onClick=${restoreDraft}>${t("draft.restore")}</button>`}>
      <p>${t("draft.lead", { time })}</p>
      ${draft.changed && html`<p class="hint" style=${{ marginTop: "8px" }}>${t("draft.changed")}</p>`}
    <//>`;
}

function DevicePill() {
  const d = daemon.value;
  const status = d.status;
  let dot = "bad";
  let text = t("device.offline");
  if (d.reachable && status) {
    if (status.device.state === "connected") {
      dot = status.blanked ? "warn" : "ok";
      text = status.blanked ? t("device.blanked") : t("device.connected", { page: status.page_name });
    } else if (status.device.state === "emulator") {
      dot = "warn";
      text = t("device.emulator");
    } else {
      dot = "warn";
      text = t("device.waiting");
    }
  }
  return html`<a class="pill device-pill" href="#device" title=${d.message ?? text} aria-label=${text}><span class=${`dot ${dot}`}></span><span class="pill-label">${text}</span></a>`;
}

export async function activateProfile(id) {
  edit((draft) => { draft.active_profile = id; });
  try {
    await api.post("/api/daemon/profile", { profile_id: id });
  } catch {
    /* the saved config makes it active on the next daemon start */
  }
}

function ProfileMenu() {
  const c = config.value;
  const prof = profile.value;
  const [renaming, setRenaming] = useState(null);
  if (!c || !prof) return null;
  const create = () => {
    const taken = new Set(c.profiles.map((p) => p.id));
    const id = uniqueId(`${prof.id}-copy`, taken);
    edit((draft) => {
      const copy = structuredClone(draft.profiles.find((p) => p.id === prof.id));
      copy.id = id;
      copy.name = t("profile.copy.name", { name: prof.name });
      draft.profiles.push(copy);
    });
    selectedProfileId.value = id;
    selectedPageId.value = null;
    toast(t("profile.created"), "ok");
  };
  const remove = () => {
    edit((draft) => {
      draft.profiles = draft.profiles.filter((p) => p.id !== prof.id);
      if (draft.active_profile === prof.id) draft.active_profile = draft.profiles[0].id;
    });
    selectedProfileId.value = null;
    selectedPageId.value = null;
  };
  return html`
    <${Popover} width="300px" trigger=${({ toggle }) => html`
      <button class="btn ghost" onClick=${toggle} aria-haspopup="menu">
        <${Icon} name="layers" size=${16} />
        <span>${prof.name}</span>
        ${c.active_profile === prof.id && html`<span class="dot ok" title=${t("profile.on.device")}></span>`}
        <${Icon} name="chevron-down" size=${15} />
      </button>`}>
      ${({ close }) => html`
        <div class="menu" role="menu">
          ${c.profiles.map((p) => html`
            <button role="menuitem" aria-selected=${p.id === prof.id} onClick=${() => { selectedProfileId.value = p.id; selectedPageId.value = null; selectedWidgetId.value = null; close(); }}>
              <${Icon} name=${p.id === prof.id ? "check" : "layers"} size=${16} />
              ${p.name}
              ${c.active_profile === p.id ? html`<small>${t("profile.on.device")}</small>` : html`<small>${t("profile.pages", { n: p.pages.length })}</small>`}
            </button>`)}
          <div class="sep"></div>
          ${c.active_profile !== prof.id && html`
            <button onClick=${() => { activateProfile(prof.id); close(); }}><${Icon} name="monitor" size=${16} />${t("profile.activate", { name: prof.name })}</button>`}
          <button onClick=${() => { setRenaming(prof.name); close(); }}><${Icon} name="pencil" size=${16} />${t("profile.rename")}</button>
          <button onClick=${() => { create(); close(); }}><${Icon} name="copy" size=${16} />${t("profile.duplicate")}</button>
          ${c.profiles.length > 1 && html`<div style=${{ padding: "4px 6px" }}><${ConfirmButton} onConfirm=${() => { remove(); close(); }}>${t("profile.delete", { name: prof.name })}<//></div>`}
        </div>`}
    <//>
    ${renaming !== null && html`
      <${Dialog} title=${t("profile.rename")} onClose=${() => setRenaming(null)}
        footer=${html`<button class="btn" onClick=${() => setRenaming(null)}>${t("common.cancel")}</button>
          <button class="btn primary" onClick=${() => {
            edit((draft) => { draft.profiles.find((p) => p.id === prof.id).name = renaming.trim() || prof.name; });
            setRenaming(null);
          }}>${t("common.save")}</button>`}>
        <input type="text" value=${renaming} onInput=${(e) => setRenaming(e.currentTarget.value)} />
      <//>`}`;
}

export function TopBar() {
  return html`
    <header class="topbar">
      <div class="brand"><img src="favicon.svg" alt="" /><b>Linexus Studio</b></div>
      <${ProfileMenu} />
      <${SaveIndicator} />
      <${ConflictDialog} />
      <${DraftDialog} />
      <div class="row tight">
        <button class="icon-btn" onClick=${undo} disabled=${!canUndo.value} title=${t("common.undo")} aria-label=${t("common.undo")}><${Icon} name="undo-2" /></button>
        <button class="icon-btn" onClick=${redo} disabled=${!canRedo.value} title=${t("common.redo")} aria-label=${t("common.redo")}><${Icon} name="redo-2" /></button>
      </div>
      <div class="spacer"></div>
      <button class="btn ghost palette-btn" onClick=${openPalette} aria-label=${t("palette.open")} title=${t("palette.open")}>
        <${Icon} name="search" size=${16} /><span class="muted palette-label">${t("palette.open")}</span>
        <kbd class="muted">Ctrl K</kbd>
      </button>
      <${DevicePill} />
    </header>`;
}
