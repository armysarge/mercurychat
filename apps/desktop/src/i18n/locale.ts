// Which languages the app ships and how the stored setting maps to one.
// Plain TypeScript with no imports: scripts/check-i18n.mjs loads this file
// directly (node strips the types), so keep it erasable.

/** The values of the core's `ui.language` setting (AppCommand::SetUiLanguage). */
export const LANGUAGE_SETTINGS = ["system", "en", "es", "pt-BR", "fr"] as const;
export type LanguageSetting = (typeof LANGUAGE_SETTINGS)[number];

/** The languages with a message file. English is the source of truth. */
export const LOCALES = ["en", "es", "pt-BR", "fr"] as const;
export type Locale = (typeof LOCALES)[number];

export const FALLBACK_LOCALE: Locale = "en";

/** Each language in its own name, whatever language the app is showing. */
export const NATIVE_NAMES: Record<Locale, string> = {
  en: "English",
  es: "Español",
  "pt-BR": "Português (Brasil)",
  fr: "Français",
};

/** Languages whose files are machine drafts: the picker says so (translated). */
export const DRAFT_LOCALES: readonly Locale[] = ["es", "pt-BR", "fr"];

function closest(tag: string): Locale | null {
  const primary = tag.toLowerCase().split(/[-_]/)[0];
  switch (primary) {
    case "en":
      return "en";
    case "es":
      return "es";
    case "fr":
      return "fr";
    case "pt":
      return "pt-BR"; // the only Portuguese the app has; pt-PT readers get the closest
    default:
      return null;
  }
}

/**
 * The language to show. An explicit setting wins; `system` takes the first of
 * the OS/browser's preferred languages that the app has, else English.
 */
export function resolveLocale(setting: string, preferred: readonly string[]): Locale {
  if ((LOCALES as readonly string[]).includes(setting)) return setting as Locale;
  for (const tag of preferred) {
    const found = closest(tag);
    if (found) return found;
  }
  return FALLBACK_LOCALE;
}

type PluralRule = (choice: number, choicesLength: number, orgRule?: (choice: number, choicesLength: number) => number) => number;

/**
 * Which form of a "one | other" message to use, by CLDR rules (Intl.PluralRules):
 * English and Spanish use the singular only for 1; French and Brazilian
 * Portuguese also for 0 ("0 message", "0 mensagem"). vue-i18n's default picks
 * the plural for everything but 1, which is wrong at 0 for those two. Messages
 * with other than two forms keep vue-i18n's own rule.
 */
export const pluralRules: Record<Locale, PluralRule> = Object.fromEntries(
  LOCALES.map((locale) => {
    const cldr = new Intl.PluralRules(locale);
    const rule: PluralRule = (choice, choicesLength, orgRule) =>
      choicesLength === 2 || !orgRule ? (cldr.select(choice) === "one" ? 0 : 1) : orgRule(choice, choicesLength);
    return [locale, rule];
  }),
) as Record<Locale, PluralRule>;
