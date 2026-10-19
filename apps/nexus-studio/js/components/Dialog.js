// Modal dialog, toasts and small shared controls.
import { html, useEffect, useRef, useState } from "../lib/html.js";
import { t } from "../lib/i18n.js";
import { Icon } from "../lib/icons.js";
import { toasts } from "../lib/store.js";

/**
 * Modal dialog. Without `onClose` it cannot be dismissed (no close button,
 * Escape or outside click), so the user has to pick one of the footer
 * buttons. `focusFirst={false}` focuses the dialog itself instead of its
 * first control: for dialogs that open on their own while the user may be
 * typing, so the next key press does not trigger a button.
 */
export function Dialog({ title, onClose, children, footer, wide = false, focusFirst = true }) {
  const ref = useRef(null);
  useEffect(() => {
    const previous = document.activeElement;
    const onKey = (e) => e.key === "Escape" && onClose?.();
    window.addEventListener("keydown", onKey);
    if (focusFirst) ref.current?.querySelector("input, select, textarea, button:not(.icon-btn)")?.focus();
    else ref.current?.focus();
    return () => {
      window.removeEventListener("keydown", onKey);
      previous?.focus?.();
    };
  }, []);
  return html`
    <div class="scrim" onMouseDown=${(e) => e.target === e.currentTarget && onClose?.()}>
      <div class="dialog" role="dialog" aria-modal="true" aria-label=${title} tabindex="-1" ref=${ref} style=${wide ? { width: "min(980px, calc(100vw - 32px))" } : null}>
        <header>
          <h2>${title}</h2>
          ${onClose && html`<button class="icon-btn" onClick=${onClose} aria-label=${t("common.close")}><${Icon} name="x" /></button>`}
        </header>
        <div class="body">${children}</div>
        ${footer && html`<footer>${footer}</footer>`}
      </div>
    </div>`;
}

export function Toasts() {
  return html`
    <div class="toasts" role="status" aria-live="polite">
      ${toasts.value.map((toast) => html`
        <div class=${`toast ${toast.kind}`} key=${toast.id}>
          <${Icon} name=${toast.kind === "error" ? "alert-triangle" : toast.kind === "ok" ? "circle-check" : "info"} />
          <span>${toast.text}</span>
        </div>`)}
    </div>`;
}

export function Switch({ checked, onChange, label, disabled, ariaLabel }) {
  return html`
    <label class="switch">
      <input type="checkbox" role="switch" aria-label=${ariaLabel ?? label} checked=${!!checked} disabled=${disabled} onChange=${(e) => onChange(e.currentTarget.checked)} />
      <i></i>
      ${label && html`<span>${label}</span>`}
    </label>`;
}

export function Segmented({ value, options, onChange, label }) {
  return html`
    <div class="segmented" role="group" aria-label=${label}>
      ${options.map((o) => html`
        <button aria-pressed=${o.value === value} onClick=${() => onChange(o.value)}>
          ${o.icon && html`<${Icon} name=${o.icon} size=${15} />`}${o.label}
        </button>`)}
    </div>`;
}

/** Button that asks for a second click before doing something destructive. */
export function ConfirmButton({ onConfirm, children, class: cls = "btn small danger", label, title }) {
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    if (!armed) return undefined;
    const timer = setTimeout(() => setArmed(false), 3000);
    return () => clearTimeout(timer);
  }, [armed]);
  return html`
    <button class=${`${cls} ${armed ? "confirm" : ""}`} title=${title} aria-label=${armed ? t("common.confirm") : title}
      onClick=${() => (armed ? (setArmed(false), onConfirm()) : setArmed(true))}>
      ${armed ? t("common.confirm") : children ?? label}
    </button>`;
}

/** Popover anchored below its trigger; closes on outside click or Escape. */
export function Popover({ trigger, children, align = "left", width }) {
  const [open, setOpen] = useState(false);
  const root = useRef(null);
  useEffect(() => {
    if (!open) return undefined;
    const onDown = (e) => !root.current?.contains(e.target) && setOpen(false);
    const onKey = (e) => e.key === "Escape" && setOpen(false);
    document.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);
  return html`
    <div ref=${root} style=${{ position: "relative", display: "inline-flex" }}>
      ${trigger({ open, toggle: () => setOpen(!open) })}
      ${open && html`
        <div class="popover" style=${{ top: "calc(100% + 6px)", [align]: 0, width }}>
          ${children({ close: () => setOpen(false) })}
        </div>`}
    </div>`;
}
