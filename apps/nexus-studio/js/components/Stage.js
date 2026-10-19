// The strip, large: either the page being edited (rendered by the same
// engine as the device) with draggable widget zones, or the live device
// mirror that forwards clicks and drags to the panel as touches.
import { html, useEffect, useRef, useState, useCallback } from "../lib/html.js";
import { api } from "../lib/api.js";
import { t } from "../lib/i18n.js";
import { Icon } from "../lib/icons.js";
import {
  config, profile, page, selectedWidgetId, edit, withPage, limits, snap, freeGaps,
  uniqueId, daemon, STRIP_W, MIN_W, toast,
} from "../lib/store.js";
import { WIDGETS, newWidget, widgetTitle } from "../widgets.js";

const TICKS = [0, 80, 160, 240, 320, 400, 480, 560, 640];

/** Server-rendered preview of `pg`, refreshed when it changes. */
export function usePreview(pg, theme, settings, extra) {
  const [url, setUrl] = useState(null);
  const key = JSON.stringify([pg, theme, settings?.locale, settings?.hour12, settings?.temperature_unit, extra]);
  useEffect(() => {
    if (!pg) return undefined;
    let cancelled = false;
    const timer = setTimeout(async () => {
      try {
        const blob = await api.preview(pg, theme, settings, extra);
        if (cancelled) return;
        const next = URL.createObjectURL(blob);
        setUrl((prev) => {
          if (prev) setTimeout(() => URL.revokeObjectURL(prev), 1000);
          return next;
        });
      } catch {
        /* keep the last good preview */
      }
    }, 40);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [key]);
  return url;
}

/** Polls the daemon's current frame while `active`. */
function useLiveFrame(active) {
  const [url, setUrl] = useState(null);
  useEffect(() => {
    if (!active) return undefined;
    let stop = false;
    let last = null;
    const loop = async () => {
      while (!stop) {
        const started = performance.now();
        if (!document.hidden) {
          try {
            const blob = await api.frame();
            const next = URL.createObjectURL(blob);
            setUrl(next);
            if (last) setTimeout(((u) => () => URL.revokeObjectURL(u))(last), 500);
            last = next;
          } catch {
            /* daemon restarting; try again shortly */
          }
        }
        const spent = performance.now() - started;
        await new Promise((r) => setTimeout(r, Math.max(30, 110 - spent)));
      }
    };
    loop();
    return () => {
      stop = true;
    };
  }, [active]);
  return url;
}

export function Stage({ mode, setMode, libraryDrag }) {
  const pg = page.value;
  const prof = profile.value;
  const settings = config.value?.settings;
  const previewUrl = usePreview(pg, prof?.theme, settings);
  const live = mode === "live";
  const liveUrl = useLiveFrame(live && daemon.value.reachable);
  const screen = useRef(null);
  const [drag, setDrag] = useState(null);
  const [dropX, setDropX] = useState(null);
  const [booted, setBooted] = useState(false);

  useEffect(() => {
    if (previewUrl && !booted) setBooted(true);
  }, [previewUrl]);

  const toStrip = useCallback((clientX) => {
    const rect = screen.current.getBoundingClientRect();
    return Math.max(0, Math.min(STRIP_W - 1, Math.round(((clientX - rect.left) / rect.width) * STRIP_W)));
  }, []);

  // ---- live mirror: forward pointer input as touches ----------------------
  // Touches go out one request at a time, so they reach the daemon in order
  // (separate requests could overtake each other and leave it pressing). A
  // move waiting in the queue is replaced by a newer one; began and ended
  // are never dropped, and an ended that fails is tried again.
  const touchState = useRef({ down: false, lastX: 0, lastSent: 0, queue: [], sending: false });
  const drainTouches = async () => {
    const st = touchState.current;
    st.sending = true;
    while (st.queue.length) {
      const { phase, x } = st.queue.shift();
      for (let attempt = 0; attempt < (phase === "ended" ? 3 : 1); attempt += 1) {
        const sent = await api.post("/api/daemon/touch", { phase, x }).then(() => true, () => false);
        if (sent) break;
      }
    }
    st.sending = false;
  };
  const sendTouch = (phase, x) => {
    const st = touchState.current;
    const last = st.queue[st.queue.length - 1];
    if (phase === "moved" && last?.phase === "moved") last.x = x;
    else st.queue.push({ phase, x });
    if (!st.sending) drainTouches();
  };
  // Leaving live mode (or the page) mid-press still lifts the finger.
  useEffect(() => () => {
    if (touchState.current.down) {
      touchState.current.down = false;
      sendTouch("ended", touchState.current.lastX);
    }
  }, [live]);
  const onLiveDown = (event) => {
    if (!live || !daemon.value.reachable) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    const st = touchState.current;
    st.down = true;
    st.lastX = toStrip(event.clientX);
    sendTouch("began", st.lastX);
  };
  const onLiveMove = (event) => {
    const st = touchState.current;
    if (!live || !st.down) return;
    st.lastX = toStrip(event.clientX);
    const now = performance.now();
    if (now - st.lastSent < 33) return;
    st.lastSent = now;
    sendTouch("moved", st.lastX);
  };
  const onLiveUp = (event) => {
    const st = touchState.current;
    if (!live || !st.down) return;
    st.down = false;
    st.lastX = toStrip(event.clientX);
    sendTouch("ended", st.lastX);
  };

  // ---- edit mode: move and resize widgets ---------------------------------
  const startDrag = (event, w, kind) => {
    if (live) return;
    event.stopPropagation();
    event.preventDefault();
    selectedWidgetId.value = w.id;
    const { left, right } = limits(pg, w.id);
    event.currentTarget.setPointerCapture?.(event.pointerId);
    setDrag({ id: w.id, kind, startX: toStrip(event.clientX), x: w.x, w: w.w, left, right, moved: false });
  };
  const onPointerMove = (event) => {
    if (live) return onLiveMove(event);
    if (!drag) return;
    const dx = toStrip(event.clientX) - drag.startX;
    if (!drag.moved && Math.abs(dx) < 3) return;
    let { x, w } = drag;
    if (drag.kind === "move") {
      x = Math.max(drag.left, Math.min(drag.right - w, snap(drag.x + dx)));
    } else if (drag.kind === "l") {
      const nx = Math.max(drag.left, Math.min(drag.x + drag.w - MIN_W, snap(drag.x + dx)));
      w = drag.x + drag.w - nx;
      x = nx;
    } else {
      const end = Math.max(drag.x + MIN_W, Math.min(drag.right, snap(drag.x + drag.w + dx)));
      w = end - drag.x;
    }
    setDrag({ ...drag, moved: true });
    edit((draft) => withPage(draft, (p) => {
      const target = p.widgets.find((it) => it.id === drag.id);
      if (target) Object.assign(target, { x, w });
    }), { coalesce: `drag:${drag.id}` });
  };
  const onPointerUp = (event) => {
    if (live) return onLiveUp(event);
    setDrag(null);
  };

  // ---- dropping new widgets from the library ------------------------------
  const onDragOver = (event) => {
    if (live || !libraryDrag.value) return;
    event.preventDefault();
    setDropX(toStrip(event.clientX));
  };
  const onDrop = (event) => {
    if (live) return;
    const type = libraryDrag.value;
    libraryDrag.value = null;
    setDropX(null);
    if (!type) return;
    event.preventDefault();
    addWidgetAt(type, toStrip(event.clientX));
  };

  const onBackgroundClick = (event) => {
    if (live) return;
    if (event.target === screen.current || event.target.classList.contains("zones")) {
      selectedWidgetId.value = null;
    }
  };

  const gaps = pg && !live ? freeGaps(pg) : [];
  const pct = (v) => `${(v / STRIP_W) * 100}%`;

  return html`
    <section class="stage" aria-label=${t("stage.label")}>
      <div class="stage-bar">
        <h2>${live ? daemon.value.status?.page_name ?? "" : pg?.name ?? ""}</h2>
        ${pg?.hidden && html`<span class="pill" title=${t("page.hidden.hint")}><${Icon} name="folder-open" size=${14} />${t("page.hidden")}</span>`}
        <div class="spacer"></div>
        <div class="segmented" role="group" aria-label=${t("stage.mode")}>
          <button aria-pressed=${!live} onClick=${() => setMode("edit")}><${Icon} name="pencil" size=${15} />${t("stage.edit")}</button>
          <button aria-pressed=${live} onClick=${() => setMode("live")} disabled=${!daemon.value.reachable} title=${daemon.value.reachable ? "" : t("stage.live.offline")}>
            <${Icon} name="monitor" size=${15} />${t("stage.live")}
          </button>
        </div>
      </div>
      <div class="ruler" aria-hidden="true">
        ${TICKS.map((v) => html`<span style=${{ left: pct(v) }}>${v}</span><i style=${{ left: pct(v) }}></i>`)}
      </div>
      <div class="bezel">
        <div
          class=${`screen ${live ? "live" : ""}`}
          ref=${screen}
          onPointerDown=${live ? onLiveDown : undefined}
          onPointerMove=${onPointerMove}
          onPointerUp=${onPointerUp}
          onPointerCancel=${onPointerUp}
          onDragOver=${onDragOver}
          onDragLeave=${() => setDropX(null)}
          onDrop=${onDrop}
          onClick=${onBackgroundClick}
        >
          ${(live ? liveUrl : previewUrl) && html`<img class=${booted ? "boot-in" : ""} src=${live ? liveUrl : previewUrl} alt=${live ? t("stage.live.alt") : t("stage.preview.alt")} draggable="false" />`}
          ${!live && pg && html`
            <div class="zones">
              ${pg.widgets.map((w) => html`
                <div
                  class=${`zone ${selectedWidgetId.value === w.id ? "selected" : ""} ${drag?.id === w.id ? "dragging" : ""}`}
                  style=${{ left: pct(w.x), width: pct(w.w) }}
                  title=${widgetTitle(w, t)}
                  role="button"
                  tabindex="0"
                  aria-label=${t("stage.widget", { name: widgetTitle(w, t) })}
                  onPointerDown=${(e) => startDrag(e, w, "move")}
                  onKeyDown=${(e) => e.key === "Enter" && (selectedWidgetId.value = w.id)}
                >
                  <span class="handle l" onPointerDown=${(e) => startDrag(e, w, "l")}></span>
                  <span class="handle r" onPointerDown=${(e) => startDrag(e, w, "r")}></span>
                </div>`)}
              ${gaps.filter((g) => g.w >= 32).map((g) => html`
                <button class="gap-add" style=${{ left: pct(g.x + g.w / 2) }} title=${t("stage.add.here")} aria-label=${t("stage.add.here")}
                  onClick=${(e) => { e.stopPropagation(); addWidgetAt("button", g.x + g.w / 2); }}>
                  <${Icon} name="plus" size=${14} />
                </button>`)}
              ${dropX !== null && html`<div class="zone drop" style=${{ left: pct(Math.max(0, dropX - 4)), width: pct(8) }}></div>`}
            </div>`}
        </div>
      </div>
      ${!live && pg && html`
        <div class="columns" aria-hidden="true">
          ${pg.widgets.map((w) => html`
            <span class=${selectedWidgetId.value === w.id ? "selected" : ""} style=${{ left: pct(w.x), width: pct(w.w) }}></span>
            ${w.w >= 64 && html`<em style=${{ left: pct(w.x + w.w / 2) }}>${widgetTitle(w, t)}</em>`}`)}
        </div>`}
      ${live && html`<p class="hint" style=${{ marginTop: "12px" }}>${t("stage.live.hint")}</p>`}
    </section>`;
}

/** Inserts a `type` widget into the free gap at or nearest to `x`. */
export function addWidgetAt(type, x) {
  const pg = page.value;
  if (!pg) return;
  const gaps = freeGaps(pg);
  if (!gaps.length) {
    toast(t("stage.full"), "error");
    return;
  }
  const gap = gaps.find((g) => x >= g.x && x < g.x + g.w)
    ?? gaps.reduce((best, g) => (Math.abs(g.x + g.w / 2 - x) < Math.abs(best.x + best.w / 2 - x) ? g : best));
  const spec = WIDGETS[type];
  const w = Math.min(gap.w, spec.w);
  const left = Math.max(gap.x, Math.min(gap.x + gap.w - w, snap(x - w / 2)));
  const taken = new Set(pg.widgets.map((it) => it.id));
  const id = uniqueId(type, taken);
  edit((draft) => withPage(draft, (p) => {
    p.widgets.push(newWidget(type, id, left, w));
    p.widgets.sort((a, b) => a.x - b.x);
  }));
  selectedWidgetId.value = id;
}
