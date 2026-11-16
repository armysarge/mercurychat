import { createApp } from "vue";
import App from "./App.vue";
import { applyLanguage, i18n } from "./i18n";
import "./tokens.css";
import "./ui.css";

// Until the core's snapshot says which language was chosen, follow the system.
applyLanguage("system");
createApp(App).use(i18n).mount("#app");
