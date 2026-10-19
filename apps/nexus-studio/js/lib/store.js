// Application state: the config being edited, selection, undo history,
// autosave (guarded by the saved file's revision, with a local draft of
// unsaved edits), daemon status and toasts. Components read signals directly.
import { signal, computed, batch, effect } from "./html.js";
import { api } from "./api.js";
import { t } from "./i18n.js";

export const STRIP_W = 640;
export const MIN_W = 24;
export const SNAP = 8;

export const config = signal(null);
export const configPath = signal("");
export const loadError = signal(null);
export const saveState = signal("saved"); // saved | pending | saving | error | conflict
export const saveError = signal("");
export const validationErrors = signal([]);
/** Set when a save was refused because the file changed elsewhere: `{ revision }` of the file now. */
export const conflict = signal(null);
/** Unsaved edits from an earlier session, offered for restoring: `{ config, savedAt, changed }`. */
export const draftOffer = signal(null);

// Revision of the saved file this editor started from; every save names
// it, so the server can refuse to overwrite changes made elsewhere.
let revision = null;

export const selectedProfileId = signal(null);
export const selectedPageId = signal(null);
export const selectedWidgetId = signal(null);

export const daemon = signal({ reachable: false, status: null });
export const health = signal(null);
export const toasts = signal([]);
export const metricsCatalog = signal([]);
export const templates = signal([]);
export const themes = signal([]);

const past = [];
const future = [];
export const canUndo = signal(false);
export const canRedo = signal(false);
let lastCoalesce = null;

export const profile = computed(() => {
  const c = config.value;
  if (!c) return null;
  return c.profiles.find((p) => p.id === selectedProfileId.value) ?? c.profiles.find((p) => p.id === c.active_profile) ?? c.profiles[0];
});
export const page = computed(() => {
  const p = profile.value;
  if (!p) return null;
  return p.pages.find((pg) => pg.id === selectedPageId.value) ?? p.pages[0];
});
export const widget = computed(() => page.value?.widgets.find((w) => w.id === selectedWidgetId.value) ?? null);

// The strip follows the page being edited (when its profile is the one on
// the strip), so every change can be checked on the hardware right away.
let followTimer = null;
effect(() => {
  const pageId = page.value?.id;
  const onStrip = profile.value && config.value && profile.value.id === config.value.active_profile;
  if (!pageId || !onStrip) return;
  clearTimeout(followTimer);
  followTimer = setTimeout(() => {
    if (daemon.value.status?.page_id !== pageId) {
      api.post("/api/daemon/page", { page_id: pageId }).catch(() => {});
    }
  }, 250);
});

export function toast(text, kind = "info") {
  const id = Math.random().toString(36).slice(2);
  toasts.value = [...toasts.value, { id, text, kind }];
  setTimeout(() => (toasts.value = toasts.value.filter((x) => x.id !== id)), kind === "error" ? 6000 : 3200);
}

let loadedOnce = false;
// Counts loads; a save that was still running when a new config was loaded
// must not touch the state of the new one.
let generation = 0;

/**
 * Loads the saved config and starts over: pending autosaves and the undo
 * history belong to the config that is being replaced. On the first load a
 * draft left by an earlier session is offered; later loads (after an
 * import, a reset or "Reload") discard it.
 */
export async function loadConfig() {
  try {
    const data = await api.get("/api/config");
    clearTimeout(saveTimer);
    saveAgain = false;
    generation += 1;
    past.length = 0;
    future.length = 0;
    lastCoalesce = null;
    revision = data.revision ?? null;
    batch(() => {
      config.value = data.config;
      configPath.value = data.path;
      loadError.value = null;
      selectedProfileId.value = data.config.active_profile;
      selectedPageId.value = null;
      selectedWidgetId.value = null;
      canUndo.value = false;
      canRedo.value = false;
      saveState.value = "saved";
      saveError.value = "";
      validationErrors.value = [];
      conflict.value = null;
    });
    if (loadedOnce) clearDraft();
    else offerDraft(data.path);
    loadedOnce = true;
    if (data.migrated) toast(t("config.migrated"), "ok");
  } catch (error) {
    loadError.value = error.message;
  }
}

// ---- local draft -------------------------------------------------------------
// While edits are not saved (autosave failing, a conflict, the tab closing
// mid-save) a copy lives in localStorage, so closing the tab loses nothing.

// Each tab keeps its own draft (sessionStorage gives a tab an id that
// survives reloads) and a heartbeat, so a draft is offered only to the tab
// that wrote it or once that tab is gone, never to a tab that is still open.
const DRAFT_PREFIX = "nexus.draft.";
const ALIVE_PREFIX = "nexus.alive.";
const ALIVE_MS = 5000;
// Browsers run timers of long-hidden tabs only about once a minute, so a
// tab counts as gone only after this long without a heartbeat.
const GONE_MS = 90_000;
const lastBeat = (tab) => {
  try {
    return Number(localStorage.getItem(ALIVE_PREFIX + tab)) || 0;
  } catch {
    return 0;
  }
};
const TAB_ID = (() => {
  const fresh = () => Math.random().toString(36).slice(2, 10);
  try {
    let id = sessionStorage.getItem("nexus.tab");
    // A duplicated tab inherits sessionStorage and with it the id of a tab
    // that is still open (a reload removed its heartbeat on the way out).
    if (!id || Date.now() - lastBeat(id) < GONE_MS) id = fresh();
    sessionStorage.setItem("nexus.tab", id);
    return id;
  } catch {
    return fresh();
  }
})();
const DRAFT_KEY = DRAFT_PREFIX + TAB_ID;

function beat() {
  try {
    localStorage.setItem(ALIVE_PREFIX + TAB_ID, String(Date.now()));
  } catch {
    /* storage blocked: drafts are not kept either */
  }
}
beat();
setInterval(beat, ALIVE_MS);
window.addEventListener("pagehide", () => {
  try {
    localStorage.removeItem(ALIVE_PREFIX + TAB_ID);
  } catch {
    /* nothing stored */
  }
});

function keepDraft() {
  try {
    localStorage.setItem(DRAFT_KEY, JSON.stringify({ path: configPath.value, revision, savedAt: Date.now(), config: config.value }));
  } catch {
    // Too large for storage (configs with several images) or blocked: an
    // older draft must not stay behind and be offered as these changes.
    // The leave-page prompt still warns.
    clearDraft();
  }
}

function clearDraft(key = DRAFT_KEY) {
  try {
    localStorage.removeItem(key);
  } catch {
    /* nothing stored */
  }
}

/**
 * Drafts this tab may offer: its own, those of tabs that stopped beating,
 * and the single shared draft older builds kept. Heartbeats of tabs that
 * crashed long ago are cleaned up on the way.
 */
function orphanedDrafts() {
  const out = [];
  try {
    const keys = Array.from({ length: localStorage.length }, (_, i) => localStorage.key(i));
    for (const key of keys) {
      if (key === "nexus.draft") {
        out.push(key);
      } else if (key?.startsWith(DRAFT_PREFIX)) {
        const tab = key.slice(DRAFT_PREFIX.length);
        if (tab === TAB_ID || Date.now() - lastBeat(tab) > GONE_MS) out.push(key);
      } else if (key?.startsWith(ALIVE_PREFIX) && Date.now() - (Number(localStorage.getItem(key)) || 0) > 86_400_000) {
        localStorage.removeItem(key);
      }
    }
  } catch {
    /* storage blocked */
  }
  return out;
}

/** JSON with sorted keys, so equal configs compare equal whatever their key order. */
const canonical = (value) => JSON.stringify(value, (_, v) => (v && typeof v === "object" && !Array.isArray(v)
  ? Object.fromEntries(Object.entries(v).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)))
  : v));

function offerDraft(path) {
  let best = null;
  for (const key of orphanedDrafts()) {
    let draft = null;
    try {
      draft = JSON.parse(localStorage.getItem(key));
    } catch {
      /* unreadable draft: dropped below */
    }
    if (!draft?.config || draft.path !== path || canonical(draft.config) === canonical(config.value)) {
      clearDraft(key);
    } else if (!best || draft.savedAt > best.draft.savedAt) {
      best = { key, draft };
    }
  }
  if (!best) return;
  const { key, draft } = best;
  draftOffer.value = { key, config: draft.config, savedAt: draft.savedAt, changed: draft.revision !== revision };
}

/** Puts the offered draft back into the editor (one undo step) and saves it. */
export function restoreDraft() {
  const draft = draftOffer.value;
  draftOffer.value = null;
  if (!draft) return;
  past.push(config.value);
  config.value = draft.config;
  canUndo.value = true;
  ensureSelection();
  // The edits become this tab's draft before the other copy goes, so a
  // crash before the save lands loses nothing.
  keepDraft();
  if (draft.key !== DRAFT_KEY) clearDraft(draft.key);
  scheduleSave();
}

export function discardDraft() {
  const key = draftOffer.value?.key;
  draftOffer.value = null;
  if (key) clearDraft(key);
}

/**
 * Deep copy for undo history that shares strings instead of copying them,
 * so embedded images (base64, up to megabytes each) exist once no matter
 * how many undo steps refer to them.
 */
function copyConfig(value) {
  if (Array.isArray(value)) return value.map(copyConfig);
  if (value && typeof value === "object") {
    const out = {};
    for (const key of Object.keys(value)) out[key] = copyConfig(value[key]);
    return out;
  }
  return value;
}

/**
 * Applies `mutate(draft)` to a copy of the config, records undo history and
 * schedules an autosave. Rapid edits with the same `coalesce` key (typing,
 * dragging) collapse into one undo step.
 */
export function edit(mutate, { coalesce = null } = {}) {
  const current = config.value;
  if (!current) return;
  const draft = copyConfig(current);
  mutate(draft);
  const now = Date.now();
  const merge = coalesce && lastCoalesce && lastCoalesce.key === coalesce && now - lastCoalesce.at < 900;
  if (!merge) past.push(current);
  if (past.length > 200) past.shift();
  lastCoalesce = coalesce ? { key: coalesce, at: now } : null;
  future.length = 0;
  config.value = draft;
  canUndo.value = past.length > 0;
  canRedo.value = false;
  scheduleSave();
}

export function undo() {
  if (!past.length) return;
  future.push(config.value);
  config.value = past.pop();
  lastCoalesce = null;
  canUndo.value = past.length > 0;
  canRedo.value = true;
  ensureSelection();
  scheduleSave();
}

export function redo() {
  if (!future.length) return;
  past.push(config.value);
  config.value = future.pop();
  canUndo.value = true;
  canRedo.value = future.length > 0;
  ensureSelection();
  scheduleSave();
}

function ensureSelection() {
  if (selectedWidgetId.value && !widget.value) selectedWidgetId.value = null;
}

let saveTimer = null;
let saving = null;
let saveAgain = false;

function scheduleSave() {
  saveState.value = "pending";
  clearTimeout(saveTimer);
  saveTimer = setTimeout(save, 450);
}

/** Saves now; waits for an in-flight save and saves again if needed. */
export async function save() {
  clearTimeout(saveTimer);
  if (saving) {
    saveAgain = true;
    return saving;
  }
  saveState.value = "saving";
  const snapshot = config.value;
  const started = generation;
  saving = api
    .post("/api/config", snapshot, { headers: { "If-Match": `"${revision ?? ""}"` } })
    .then((data) => {
      if (started !== generation) return;
      revision = data.revision ?? revision;
      validationErrors.value = [];
      saveError.value = "";
      saveState.value = config.value === snapshot ? "saved" : "pending";
      if (saveState.value === "saved") clearDraft();
    })
    .catch((error) => {
      if (started !== generation) return;
      if (error.status === 409) {
        saveState.value = "conflict";
        saveError.value = t("conflict.title");
        conflict.value = { revision: error.body?.revision ?? null };
      } else {
        const message = error.status === 413 ? t("save.too.large") : error.message;
        saveState.value = "error";
        saveError.value = message;
        validationErrors.value = error.status === 413 ? [message] : error.body?.errors ?? [message];
      }
      keepDraft();
    })
    .finally(() => {
      saving = null;
      if (started !== generation) {
        // Edits made after the load waited for this save to finish.
        if (saveAgain) {
          saveAgain = false;
          scheduleSave();
        }
        return;
      }
      if (saveAgain || saveState.value === "pending") {
        saveAgain = false;
        if (config.value !== snapshot) scheduleSave();
      }
    });
  return saving;
}

/** After a conflict: saves this tab's config over the one saved elsewhere. */
export async function overwrite() {
  const target = conflict.value?.revision;
  conflict.value = null;
  // Wait for a save still running: it would otherwise swallow this one,
  // and if it ran into the conflict too it reports the newest revision.
  await saving;
  revision = conflict.value?.revision ?? target ?? revision;
  conflict.value = null;
  return save();
}

/** Flushes a pending autosave (before leaving the page or reloading). */
export function flush() {
  if (saveState.value === "pending") return save();
  return saving ?? Promise.resolve();
}

window.addEventListener("beforeunload", (event) => {
  const state = saveState.value;
  if (state === "saved") return;
  keepDraft();
  if (state === "pending" || state === "saving") save();
  event.preventDefault();
});

// ---- helpers that edit the selected profile/page ---------------------------

export function profileIndex(draft) {
  const id = profile.value?.id;
  return draft.profiles.findIndex((p) => p.id === id);
}

export function withPage(draft, fn) {
  const p = draft.profiles[profileIndex(draft)];
  const pg = p?.pages.find((x) => x.id === page.value?.id);
  if (pg) fn(pg, p);
}

export function uniqueId(base, taken) {
  const slug = (base || "item").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "") || "item";
  if (!taken.has(slug)) return slug;
  for (let i = 2; ; i += 1) {
    if (!taken.has(`${slug}-${i}`)) return `${slug}-${i}`;
  }
}

/** Free X ranges of a page, sorted left to right. */
export function freeGaps(pg, ignoreId = null) {
  const used = pg.widgets.filter((w) => w.id !== ignoreId).sort((a, b) => a.x - b.x);
  const gaps = [];
  let cursor = 0;
  for (const w of used) {
    if (w.x > cursor) gaps.push({ x: cursor, w: w.x - cursor });
    cursor = Math.max(cursor, w.x + w.w);
  }
  if (cursor < STRIP_W) gaps.push({ x: cursor, w: STRIP_W - cursor });
  return gaps.filter((g) => g.w >= MIN_W);
}

/** Where `w` can move to between its neighbours. */
export function limits(pg, id) {
  const others = pg.widgets.filter((w) => w.id !== id);
  const self = pg.widgets.find((w) => w.id === id);
  let left = 0;
  let right = STRIP_W;
  for (const o of others) {
    if (o.x + o.w <= self.x) left = Math.max(left, o.x + o.w);
    if (o.x >= self.x + self.w) right = Math.min(right, o.x);
  }
  return { left, right };
}

export const snap = (x) => Math.round(x / SNAP) * SNAP;

// ---- daemon status ---------------------------------------------------------

export async function refreshDaemon() {
  try {
    const data = await api.get("/api/daemon/status");
    daemon.value = { reachable: data.reachable, status: data.status, message: data.message };
  } catch {
    daemon.value = { reachable: false, status: null };
  }
}

export async function refreshHealth() {
  try {
    health.value = await api.get("/api/health");
  } catch {
    /* the server itself is gone; the next poll reports it */
  }
}

export async function refreshMetrics() {
  try {
    const data = await api.get("/api/metrics");
    metricsCatalog.value = data.metrics;
  } catch {
    /* keep the last catalogue */
  }
}

export async function loadCatalogs() {
  const [tpl, th] = await Promise.allSettled([api.get("/api/templates"), api.get("/api/themes")]);
  if (tpl.status === "fulfilled") templates.value = tpl.value.templates;
  if (th.status === "fulfilled") themes.value = th.value.presets;
}
