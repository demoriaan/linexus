// Studio's own appearance: light, dark or following the system, and the
// accent colour borrowed from the strip theme being edited.
import { signal, effect } from "./html.js";
import { profile, themes } from "./store.js";

const read = () => {
  try {
    return localStorage.getItem("nexus.theme") || "system";
  } catch {
    return "system";
  }
};

export const themeChoice = signal(read());
const media = window.matchMedia("(prefers-color-scheme: light)");

function apply() {
  const choice = themeChoice.value;
  const light = choice === "light" || (choice === "system" && media.matches);
  document.documentElement.dataset.theme = light ? "light" : "dark";
}

export function setTheme(choice) {
  themeChoice.value = choice;
  try {
    localStorage.setItem("nexus.theme", choice);
  } catch {
    /* not remembered in private mode */
  }
}

media.addEventListener("change", apply);
effect(apply);

/** Accent = the profile's strip accent, so Studio and the device match. */
effect(() => {
  const theme = profile.value?.theme;
  const preset = themes.value.find((th) => th.id === (theme?.preset ?? "midnight"));
  const accent = theme?.accent ?? preset?.palette?.accent;
  if (!accent) return;
  const root = document.documentElement.style;
  root.setProperty("--accent", accent);
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(accent.slice(i, i + 2), 16));
  const luminance = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
  root.setProperty("--accent-ink", luminance > 0.55 ? "#08111c" : "#ffffff");
});
