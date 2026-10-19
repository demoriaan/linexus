import { metricsCatalog } from "./lib/store.js";

// What Studio knows about each widget type: how it looks in the library,
// its default size and props, and which fields the inspector shows.
// Field kinds: text, number, select, toggle, icon, metric, location, entity,
// feeds, image, args.

export const WIDGETS = {
  clock: {
    icon: "clock",
    w: 120,
    props: { show_date: true, show_seconds: false },
    fields: [
      { key: "show_date", kind: "toggle" },
      { key: "show_seconds", kind: "toggle" },
      { key: "hour12", kind: "select", options: ["", "false", "true"], cast: "bool?" },
      { key: "label", kind: "text", optional: true },
    ],
  },
  metric: {
    icon: "activity",
    w: 134,
    props: { source: "cpu.usage", style: "sparkline", secondary: "cpu.temp" },
    fields: [
      { key: "source", kind: "metric" },
      { key: "style", kind: "select", options: ["sparkline", "bar", "ring", "number"] },
      { key: "secondary", kind: "metric", optional: true },
      { key: "label", kind: "text", optional: true },
      { key: "warn", kind: "number", optional: true },
      { key: "critical", kind: "number", optional: true },
    ],
  },
  network: { icon: "arrow-left-right", w: 134, props: {}, fields: [{ key: "interface", kind: "text", optional: true }] },
  media: {
    icon: "music",
    w: 480,
    props: { show_art: true, show_progress: true, show_controls: true, tint: true },
    fields: [
      { key: "show_art", kind: "toggle" },
      { key: "show_progress", kind: "toggle" },
      { key: "show_controls", kind: "toggle" },
      { key: "tint", kind: "toggle" },
      { key: "player", kind: "text", optional: true },
    ],
  },
  volume: {
    icon: "volume-2",
    w: 160,
    props: { show_value: true, step: 5 },
    fields: [{ key: "show_value", kind: "toggle" }],
  },
  button: {
    icon: "pointer",
    w: 107,
    props: { label: "Button", icon: "lucide:sparkles", style: "tile", show_label: true, running_indicator: true },
    fields: [
      { key: "label", kind: "text" },
      { key: "icon", kind: "icon" },
      { key: "style", kind: "select", options: ["tile", "inline", "icon"] },
      { key: "show_label", kind: "toggle" },
      { key: "running_indicator", kind: "toggle" },
    ],
  },
  news: {
    icon: "newspaper",
    w: 640,
    props: { font: "pixel" },
    fields: [
      { key: "font", kind: "select", options: ["pixel", "smooth"] },
      { key: "feeds", kind: "feeds" },
    ],
  },
  weather: {
    icon: "cloud-sun",
    w: 220,
    props: { show_range: true },
    fields: [
      { key: "location", kind: "location", optional: true },
      { key: "show_range", kind: "toggle" },
    ],
  },
  entity: {
    icon: "house",
    w: 150,
    props: { entity_id: "" },
    fields: [
      { key: "entity_id", kind: "entity" },
      { key: "label", kind: "text", optional: true },
      { key: "icon", kind: "icon", optional: true },
    ],
  },
  timer: {
    icon: "timer",
    w: 200,
    props: { mode: "pomodoro", minutes: 25, break_minutes: 5, long_break_minutes: 15, rounds: 4 },
    fields: [
      { key: "mode", kind: "select", options: ["pomodoro", "countdown", "stopwatch"] },
      { key: "minutes", kind: "number", min: 1, max: 600 },
      { key: "break_minutes", kind: "number", min: 1, max: 120, when: { mode: "pomodoro" } },
      { key: "long_break_minutes", kind: "number", min: 1, max: 120, when: { mode: "pomodoro" } },
      { key: "rounds", kind: "number", min: 1, max: 12, when: { mode: "pomodoro" } },
      { key: "label", kind: "text", optional: true },
    ],
  },
  text: {
    icon: "type",
    w: 200,
    props: { text: "CPU {cpu.temp}", size: "medium", align: "center" },
    fields: [
      { key: "text", kind: "text", tokens: true },
      { key: "caption", kind: "text", optional: true },
      { key: "size", kind: "select", options: ["small", "medium", "large"] },
      { key: "align", kind: "select", options: ["left", "center", "right"] },
    ],
  },
  image: {
    icon: "image",
    w: 160,
    props: { src: "", fit: "fill" },
    fields: [
      { key: "src", kind: "image" },
      { key: "fit", kind: "select", options: ["fill", "fit", "stretch"] },
    ],
  },
  command: {
    icon: "terminal",
    w: 200,
    // A runnable example: the server refuses an empty program.
    props: { program: "uptime", args: ["-p"], interval_seconds: 30 },
    fields: [
      { key: "program", kind: "text" },
      { key: "args", kind: "args" },
      { key: "interval_seconds", kind: "number", min: 2, max: 86400 },
      { key: "label", kind: "text", optional: true },
    ],
  },
  spacer: { icon: "move-horizontal", w: 40, props: {}, fields: [] },
};

export const WIDGET_ORDER = [
  "button", "clock", "metric", "network", "media", "volume", "news",
  "weather", "entity", "timer", "text", "image", "command", "spacer",
];

/** Built-in behaviour per type when no tap/double-tap/hold action is set. */
export const BUILTIN = {
  media: { tap: "builtin.media.tap" },
  volume: { tap: "builtin.volume.tap" },
  news: { tap: "builtin.news.tap", double_tap: "builtin.news.double", hold: "builtin.news.hold" },
  timer: { tap: "builtin.timer.tap", double_tap: "builtin.timer.double", hold: "builtin.timer.hold" },
  entity: { tap: "builtin.entity.tap" },
};

export function newWidget(type, id, x, w) {
  const spec = WIDGETS[type];
  return { id, x, w, type, ...structuredClone(spec.props) };
}

/** Readable short description of a widget instance for lists and labels. */
export function widgetTitle(widget, t) {
  switch (widget.type) {
    case "button":
      return widget.label || t("widget.button");
    case "metric":
      return widget.label || metricsCatalog.value.find((m) => m.id === widget.source)?.label || widget.source;
    case "entity":
      return widget.label || widget.entity_id || t("widget.entity");
    case "text":
      return widget.text || t("widget.text");
    default:
      return t(`widget.${widget.type}`);
  }
}
