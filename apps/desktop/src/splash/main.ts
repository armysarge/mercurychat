// The start-up splash: a second, tiny page of the same app. It uses the same
// translations; the language last chosen in the main window is remembered
// (the core's setting is not readable yet while the splash is up).
import { createApp } from "vue";
import { applyLanguage, i18n, storedLanguage } from "../i18n";
import "../tokens.css";
import "../ui.css";
import Splash from "./Splash.vue";

applyLanguage(storedLanguage());
createApp(Splash).use(i18n).mount("#app");
