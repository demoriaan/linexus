// The editor: the strip on stage, pages below it, widget library, and the
// inspector on the right.
import { html, useEffect, useState, signal } from "../lib/html.js";
import { t } from "../lib/i18n.js";
import { Icon } from "../lib/icons.js";
import {
  page, widget, selectedWidgetId, edit, withPage, limits, undo, redo, validationErrors, health,
} from "../lib/store.js";
import { Stage } from "../components/Stage.js";
import { Filmstrip } from "../components/Filmstrip.js";
import { Library } from "../components/Library.js";
import { Inspector } from "../components/Inspector.js";

const libraryDrag = signal(null);
export const stageMode = signal("edit");

function isTyping(event) {
  const el = event.target;
  return el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement || el instanceof HTMLSelectElement || el.isContentEditable;
}

export function PagesView() {
  useEffect(() => {
    const onKey = (e) => {
      if (isTyping(e) || document.querySelector(".scrim")) return;
      const mod = e.ctrlKey || e.metaKey;
      if (mod && e.key.toLowerCase() === "z") {
        e.preventDefault();
        return e.shiftKey ? redo() : undo();
      }
      if (mod && e.key.toLowerCase() === "y") {
        e.preventDefault();
        return redo();
      }
      const w = widget.value;
      if (!w) return;
      if (e.key === "Escape") return (selectedWidgetId.value = null);
      if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        edit((draft) => withPage(draft, (pg) => { pg.widgets = pg.widgets.filter((it) => it.id !== w.id); }));
        selectedWidgetId.value = null;
        return;
      }
      if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        e.preventDefault();
        const step = (e.shiftKey ? 8 : 1) * (e.key === "ArrowLeft" ? -1 : 1);
        const { left, right } = limits(page.value, w.id);
        edit((draft) => withPage(draft, (pg) => {
          const it = pg.widgets.find((x) => x.id === w.id);
          if (e.altKey) it.w = Math.max(24, Math.min(right - it.x, it.w + step));
          else it.x = Math.max(left, Math.min(right - it.w, it.x + step));
        }), { coalesce: `nudge:${w.id}` });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const problems = validationErrors.value;
  const deviceProblem = health.value?.checks?.find((c) => c.state === "error");
  return html`
    <div class="view">
      ${deviceProblem && html`
        <div class="banner">
          <${Icon} name="alert-triangle" />
          <div class="grow"><b>${deviceProblem.title}</b><br /><span class="hint">${deviceProblem.detail}</span></div>
          <a class="btn small" href="#device">${t("banner.fix")}</a>
        </div>`}
      ${problems.length > 0 && html`
        <div class="banner">
          <${Icon} name="alert-triangle" />
          <div class="grow"><b>${t("save.invalid")}</b><br /><span class="hint">${problems.join(" · ")}</span></div>
        </div>`}
      <div class="pages-layout">
        <div class="stack" style=${{ gap: "18px", minWidth: 0 }}>
          <${Stage} mode=${stageMode.value} setMode=${(m) => (stageMode.value = m)} libraryDrag=${libraryDrag} />
          <${Filmstrip} />
          ${stageMode.value === "edit" && html`<${Library} libraryDrag=${libraryDrag} />`}
        </div>
        <${Inspector} />
      </div>
    </div>`;
}

export function useNow(interval = 1000) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), interval);
    return () => clearInterval(timer);
  }, [interval]);
  return now;
}
