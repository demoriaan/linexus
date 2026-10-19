// Interface language. English and German ship with Studio; the choice is
// kept in the browser and defaults to the browser language.
import { signal, computed } from "./html.js";
import { STRINGS } from "./strings.js";

const stored = (() => {
  try {
    return localStorage.getItem("nexus.lang");
  } catch {
    return null;
  }
})();

export const lang = signal(stored || (navigator.language?.toLowerCase().startsWith("de") ? "de" : "en"));

export function setLang(value) {
  lang.value = value;
  try {
    localStorage.setItem("nexus.lang", value);
  } catch {
    /* private mode: language just is not remembered */
  }
  document.documentElement.lang = value;
}

const table = computed(() => STRINGS[lang.value] ?? STRINGS.en);

/** Translates `key`, filling `{name}` placeholders from `vars`. */
export function t(key, vars) {
  const text = table.value[key] ?? STRINGS.en[key] ?? key;
  if (!vars) return text;
  return text.replace(/\{(\w+)\}/g, (_, name) => (vars[name] ?? `{${name}}`));
}

/** Whether the current language has its own text for `key`. */
export function hasText(key) {
  return key in table.value;
}
