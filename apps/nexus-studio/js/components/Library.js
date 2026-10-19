// Widget library: drag a widget onto the strip, or click to drop it into
// the widest free gap.
import { html } from "../lib/html.js";
import { t } from "../lib/i18n.js";
import { Icon } from "../lib/icons.js";
import { page, freeGaps } from "../lib/store.js";
import { WIDGETS, WIDGET_ORDER } from "../widgets.js";
import { addWidgetAt } from "./Stage.js";

export function Library({ libraryDrag }) {
  const addToWidestGap = (type) => {
    const gaps = page.value ? freeGaps(page.value) : [];
    const widest = gaps.reduce((best, g) => (!best || g.w > best.w ? g : best), null);
    addWidgetAt(type, widest ? widest.x + widest.w / 2 : 0);
  };
  return html`
    <section class="panel">
      <header>
        <h3>${t("library.title")}</h3>
        <p>${t("library.hint")}</p>
      </header>
      <div class="library">
        ${WIDGET_ORDER.map((type) => html`
          <button
            class="lib-card"
            draggable="true"
            onDragStart=${(e) => { libraryDrag.value = type; e.dataTransfer.setData("text/plain", type); e.dataTransfer.effectAllowed = "copy"; }}
            onDragEnd=${() => (libraryDrag.value = null)}
            onClick=${() => addToWidestGap(type)}
          >
            <span class="head"><${Icon} name=${WIDGETS[type].icon} />${t(`widget.${type}`)}</span>
            <p>${t(`widget.${type}.hint`)}</p>
          </button>`)}
      </div>
    </section>`;
}
