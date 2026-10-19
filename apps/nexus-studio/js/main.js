// Linexus Studio: shell, navigation and background polling.
import { html, render, signal, useEffect } from "./lib/html.js";
import { t, lang } from "./lib/i18n.js";
import { Icon } from "./lib/icons.js";
import {
  config, loadConfig, loadError, refreshDaemon, refreshHealth, refreshMetrics, loadCatalogs, flush,
} from "./lib/store.js";
import "./lib/theme.js";
import { TopBar } from "./components/TopBar.js";
import { Toasts } from "./components/Dialog.js";
import { CommandPalette } from "./components/CommandPalette.js";
import { PagesView } from "./views/PagesView.js";
import { NewsView } from "./views/NewsView.js";
import { ConnectionsView } from "./views/ConnectionsView.js";
import { DeviceView } from "./views/DeviceView.js";
import { SettingsView } from "./views/SettingsView.js";

const VIEWS = {
  pages: { icon: "layout-panel-top", view: PagesView },
  news: { icon: "newspaper", view: NewsView },
  connections: { icon: "plug", view: ConnectionsView },
  device: { icon: "monitor", view: DeviceView },
  settings: { icon: "settings", view: SettingsView },
};

const route = signal(currentRoute());
function currentRoute() {
  const hash = location.hash.replace(/^#/, "");
  return VIEWS[hash] ? hash : "pages";
}
window.addEventListener("hashchange", () => {
  flush();
  route.value = currentRoute();
});

function Rail() {
  return html`
    <nav class="rail" aria-label=${t("nav.label")}>
      ${Object.entries(VIEWS).map(([id, { icon }]) => html`
        <a href=${`#${id}`} aria-current=${route.value === id ? "page" : undefined}>
          <${Icon} name=${icon} />${t(`nav.${id}`)}
        </a>`)}
    </nav>`;
}

function App() {
  useEffect(() => {
    document.documentElement.lang = lang.value;
    loadConfig();
    loadCatalogs();
    refreshDaemon();
    refreshHealth();
    refreshMetrics();
    const daemonTimer = setInterval(refreshDaemon, 2000);
    const healthTimer = setInterval(refreshHealth, 20000);
    const metricsTimer = setInterval(refreshMetrics, 10000);
    return () => {
      clearInterval(daemonTimer);
      clearInterval(healthTimer);
      clearInterval(metricsTimer);
    };
  }, []);

  if (loadError.value) {
    return html`<div class="empty" style=${{ height: "100vh" }}>
      <${Icon} name="alert-triangle" />
      <h2>${t("load.error.title")}</h2>
      <p>${loadError.value}</p>
      <button class="btn" onClick=${loadConfig}>${t("common.retry")}</button>
    </div>`;
  }
  if (!config.value) {
    return html`<div class="boot"><div class="boot-strip"></div><p>${t("common.loading")}</p></div>`;
  }
  const View = VIEWS[route.value].view;
  return html`
    <div class="shell">
      <${TopBar} />
      <${Rail} />
      <main class="main" id="main"><${View} /></main>
    </div>
    <${CommandPalette} />
    <${Toasts} />`;
}

render(html`<${App} />`, document.getElementById("app"));
