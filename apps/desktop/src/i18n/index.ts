// The UI's translations (vue-i18n, Composition API mode). English is the
// source of truth and the fallback. The messages are JSON compiled at build
// time by @intlify/unplugin-vue-i18n, so the runtime never compiles a message
// (no eval / new Function: Tauri's CSP forbids it).
import { createI18n } from "vue-i18n";
import en from "./locales/en.json";
import es from "./locales/es.json";
import fr from "./locales/fr.json";
import ptBR from "./locales/pt-BR.json";
import { FALLBACK_LOCALE, pluralRules, resolveLocale, type Locale } from "./locale";

export const i18n = createI18n({
  legacy: false,
  locale: FALLBACK_LOCALE,
  fallbackLocale: FALLBACK_LOCALE,
  pluralRules,
  messages: { en, es, "pt-BR": ptBR, fr },
});

/** `t` for code outside components (store.ts, api.ts). Reactive to the locale. */
export const t = i18n.global.t;
export const te = i18n.global.te;

/** The language in use. */
export function currentLocale(): Locale {
  return i18n.global.locale.value as Locale;
}

/** Apply the stored `ui.language` setting (`system` follows the OS). */
export function applyLanguage(setting: string): Locale {
  const preferred = typeof navigator === "undefined" ? [] : navigator.languages?.length ? navigator.languages : [navigator.language];
  const locale = resolveLocale(setting, preferred);
  i18n.global.locale.value = locale;
  if (typeof document !== "undefined") document.documentElement.lang = locale;
  return locale;
}
