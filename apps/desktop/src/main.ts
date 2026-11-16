import { createApp } from "vue";
import App from "./App.vue";
import { applyLanguage, i18n, storedLanguage } from "./i18n";
import { installShellGuards } from "./shellGuards";
import "./tokens.css";
import "./ui.css";

// Until the core's snapshot says which language was chosen, use the one last
// remembered (or the system's).
applyLanguage(storedLanguage());
installShellGuards();
createApp(App).use(i18n).mount("#app");
