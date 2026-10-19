// sensors.json: news feeds, headline pacing and connected services. The
// daemon re-reads it about once a second, so changes reach the strip
// without a restart.
import { signal } from "./html.js";
import { api } from "./api.js";

export const sensors = signal(null);
export const sensorsError = signal(null);

export async function loadSensors() {
  try {
    const data = await api.get("/api/sensors/config");
    sensors.value = data;
    sensorsError.value = data.config_error ?? null;
  } catch (error) {
    sensorsError.value = error.message;
  }
  return sensors.value;
}

/** Saves feeds and headline pacing only; other sections stay as on disk. */
export async function saveNews(rss, display) {
  const data = await api.post("/api/sensors/news", { rss, display });
  if (sensors.value) {
    sensors.value = { ...sensors.value, config: { ...sensors.value.config, rss, display } };
  }
  return data;
}

/**
 * Replaces one section (`webhooks`, `mqtt`, `ifttt`, `home_assistant`) on a
 * freshly loaded copy, so edits made elsewhere in the meantime survive.
 * `value` may be a function that derives the section from its fresh copy.
 */
export async function saveSection(section, value) {
  const fresh = await api.get("/api/sensors/config");
  const current = fresh.config[section] ?? [];
  const next = { ...fresh.config, [section]: typeof value === "function" ? value(current) : value };
  await api.post("/api/sensors/config", next);
  sensors.value = { ...fresh, config: next };
}
