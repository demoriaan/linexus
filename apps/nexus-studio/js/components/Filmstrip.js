// Pages of the selected profile as live thumbnails; drag to reorder, plus a
// gallery of templates to add new pages from.
import { html, useState } from "../lib/html.js";
import { t } from "../lib/i18n.js";
import { Icon } from "../lib/icons.js";
import {
  config, profile, page, selectedPageId, selectedWidgetId, edit, profileIndex, uniqueId, templates,
} from "../lib/store.js";
import { usePreview } from "./Stage.js";
import { Dialog } from "./Dialog.js";

function Thumb({ pg, index, current, onDragStart, onDropAt }) {
  const prof = profile.value;
  const url = usePreview(pg, prof.theme, config.value.settings);
  const [over, setOver] = useState(false);
  const visible = prof.pages.filter((p) => !p.hidden).findIndex((p) => p.id === pg.id);
  return html`
    <button
      class=${`thumb ${over ? "dragover" : ""}`}
      aria-current=${current ? "true" : "false"}
      draggable="true"
      onDragStart=${(e) => { e.dataTransfer.setData("text/x-nexus-page", String(index)); onDragStart(index); }}
      onDragOver=${(e) => { if (e.dataTransfer.types.includes("text/x-nexus-page")) { e.preventDefault(); setOver(true); } }}
      onDragLeave=${() => setOver(false)}
      onDrop=${(e) => { e.preventDefault(); setOver(false); onDropAt(index); }}
      onClick=${() => { selectedPageId.value = pg.id; selectedWidgetId.value = null; }}
    >
      ${url ? html`<img src=${url} alt="" />` : html`<img alt="" />`}
      <span class="label">
        ${pg.hidden && html`<${Icon} name="folder-open" size=${14} />`}
        ${pg.name || pg.id}
        <small>${pg.hidden ? t("page.hidden") : visible + 1}</small>
      </span>
    </button>`;
}

export function Filmstrip() {
  const prof = profile.value;
  const [dragFrom, setDragFrom] = useState(null);
  const [gallery, setGallery] = useState(false);
  if (!prof) return null;
  const move = (to) => {
    if (dragFrom === null || dragFrom === to) return;
    edit((draft) => {
      const pages = draft.profiles[profileIndex(draft)].pages;
      const [item] = pages.splice(dragFrom, 1);
      pages.splice(to, 0, item);
    });
    setDragFrom(null);
  };
  return html`
    <div class="filmstrip" role="list" aria-label=${t("pages.list")}>
      ${prof.pages.map((pg, i) => html`
        <${Thumb} key=${pg.id} pg=${pg} index=${i} current=${page.value?.id === pg.id} onDragStart=${setDragFrom} onDropAt=${move} />`)}
      <button class="thumb add" onClick=${() => setGallery(true)}>
        <${Icon} name="plus" size=${20} />
        ${t("pages.add")}
      </button>
    </div>
    ${gallery && html`<${TemplateGallery} onClose=${() => setGallery(false)} />`}`;
}

function TemplatePreview({ template }) {
  const url = usePreview(template.page, profile.value.theme, config.value.settings);
  return url ? html`<img class="preview-strip" src=${url} alt="" />` : html`<div class="preview-strip"></div>`;
}

export function TemplateGallery({ onClose }) {
  const add = (template) => {
    const prof = profile.value;
    const taken = new Set(prof.pages.map((p) => p.id));
    const id = uniqueId(template.id, taken);
    const name = t(`template.${template.id}`);
    edit((draft) => {
      const pages = draft.profiles[profileIndex(draft)].pages;
      const at = pages.findIndex((p) => p.id === page.value?.id);
      pages.splice(at + 1, 0, { ...structuredClone(template.page), id, name });
    });
    selectedPageId.value = id;
    selectedWidgetId.value = null;
    onClose();
  };
  return html`
    <${Dialog} title=${t("pages.add.title")} onClose=${onClose} wide>
      <p class="hint" style=${{ marginBottom: "12px" }}>${t("pages.add.hint")}</p>
      <div class="template-grid">
        ${templates.value.map((tpl) => html`
          <button onClick=${() => add(tpl)}>
            <${TemplatePreview} template=${tpl} />
            <b>${t(`template.${tpl.id}`)}</b>
            <span class="hint">${t(`template.${tpl.id}.hint`)}</span>
          </button>`)}
      </div>
    <//>`;
}
