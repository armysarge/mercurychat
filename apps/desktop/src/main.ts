import { createApp } from "vue";
import App from "./App.vue";
import { applyLanguage, i18n, storedLanguage } from "./i18n";
import { installShellGuards } from "./shellGuards";
import "./tokens.css";
import "./ui.css";
import { initAppearance } from "./themes/state.ts";
import "./extras"; // M3c: QSY, information and suggestion events from the start

// Until the core's snapshot says which language was chosen, use the one last
// remembered (or the system's).
applyLanguage(storedLanguage());
installShellGuards();
// The saved theme is applied before the first paint (the shell holds it in appearance.json).
void initAppearance().finally(() => createApp(App).use(i18n).mount("#app"));
