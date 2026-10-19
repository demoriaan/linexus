// JSX-like templates without a build step: html`<div class=${x}>…</div>`.
import { h, render, Fragment } from "../../vendor/preact.mjs";
import htm from "../../vendor/htm.mjs";

export const html = htm.bind(h);
export { h, render, Fragment };
export {
  useState, useEffect, useMemo, useRef, useCallback, useLayoutEffect,
} from "../../vendor/hooks.mjs";
export { signal, computed, effect, batch, useSignal, useComputed } from "../../vendor/signals.mjs";
