// Lucide icons as inline SVG. The interface's own icons are bundled; the full
// catalogue (2,000+ icons with search tags) loads on demand for the picker.
import { html, signal } from "./html.js";
import { api } from "./api.js";
import { UI_ICONS } from "./ui-icons.js";

/** name -> { tags, body } once loaded */
export const catalog = signal(null);
let loading = null;

export function loadCatalog() {
  if (!loading) {
    loading = api.get("/api/icons").then((data) => {
      const map = new Map();
      for (const [name, tags, body] of data.icons) map.set(name, { tags, body });
      catalog.value = map;
      return map;
    });
  }
  return loading;
}

export function iconBody(name) {
  const bare = (name || "").replace(/^lucide:/, "");
  return UI_ICONS[bare] ?? catalog.value?.get(bare)?.body ?? null;
}

const ESCAPES = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" };
const escapeHtml = (text) => String(text).replace(/[&<>"']/g, (ch) => ESCAPES[ch]);

/** Inline Lucide icon. The SVG body is trusted catalogue markup; `title` is escaped text. */
export function Icon({ name, size = 18, class: cls = "", title }) {
  const body = iconBody(name);
  if (!body && !catalog.value) loadCatalog();
  return html`<svg
    class=${`icon ${cls}`}
    width=${size}
    height=${size}
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden=${title ? undefined : "true"}
    role=${title ? "img" : undefined}
    dangerouslySetInnerHTML=${{ __html: (title ? `<title>${escapeHtml(title)}</title>` : "") + (body ?? "") }}
  ></svg>`;
}

/** Icon reference from config (`lucide:x`, `app:id`, `data:` URL) as an element. */
export function IconRef({ value, size = 18 }) {
  if (!value) return null;
  if (value.startsWith("data:")) {
    return html`<img src=${value} width=${size} height=${size} alt="" />`;
  }
  if (value.startsWith("app:")) {
    const id = encodeURIComponent(value.slice(4));
    return html`<img src=${`/api/app-icon?id=${id}&size=${size > 32 ? 64 : 32}`} width=${size} height=${size} alt="" />`;
  }
  return html`<${Icon} name=${value} size=${size} />`;
}
