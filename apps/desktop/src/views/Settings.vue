<script setup lang="ts">
// Settings (docs/04 §4): Appearance (language) and Station: profiles, and new
// ones from the same forms as first use.
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { call, describeError } from "../api";
import type { StationProfile } from "../bindings/StationProfile";
import ProfileForms from "../components/ProfileForms.vue";
import ThemePicker from "../components/ThemePicker.vue";
import { DRAFT_LOCALES, LANGUAGE_SETTINGS, NATIVE_NAMES, type Locale } from "../i18n/locale";
import { mirror, snapshot } from "../store";

const { t } = useI18n();

const error = ref("");
const working = ref(false);

async function act(step: () => Promise<unknown>) {
  working.value = true;
  error.value = "";
  try {
    await step();
  } catch (e) {
    error.value = describeError(e);
  } finally {
    working.value = false;
  }
}
const select = (id: string | null) => act(() => call({ type: "selectProfile", id }));
/** The profile whose Delete is waiting for a yes (docs/04 §5: destructive
 *  actions are explicit). */
const confirming = ref<string | null>(null);
const remove = (id: string) => {
  confirming.value = null;
  return act(() => call({ type: "deleteProfile", id }));
};
const where = (p: StationProfile) => (p.modem.kind === "simulator" ? t("settings.simulator") : `${p.modem.host}:${p.modem.controlPort}`);

/** Each language in its own name (with "(draft)" while its file is unreviewed); System default in this language. */
const languages = computed(() =>
  LANGUAGE_SETTINGS.map((value) => {
    if (value === "system") return { value, label: t("appearance.system") };
    const draft = DRAFT_LOCALES.includes(value as Locale) ? ` ${t("appearance.draft")}` : "";
    return { value, label: NATIVE_NAMES[value as Locale] + draft };
  }),
);
const language = computed(() => snapshot.value?.uiLanguage ?? "system");
const languageError = ref("");
async function chooseLanguage(value: string) {
  languageError.value = "";
  try {
    await call({ type: "setUiLanguage", language: value });
    mirror.setUiLanguage(value); // the core keeps it; the window changes language now
  } catch (e) {
    languageError.value = `${t("appearance.saveFailed")}: ${describeError(e)}`;
  }
}
</script>

<template>
  <div class="settings">
    <h1>{{ t("settings.title") }}</h1>
    <section class="card stack" aria-labelledby="h-appearance">
      <h2 id="h-appearance">{{ t("appearance.title") }}</h2>
      <label class="lang">
        {{ t("appearance.language") }}
        <select :value="language" @change="chooseLanguage(($event.target as HTMLSelectElement).value)">
          <option v-for="l in languages" :key="l.value" :value="l.value">{{ l.label }}</option>
        </select>
      </label>
      <p class="muted small">{{ t("appearance.hint") }}</p>
      <p v-if="languageError" class="err" role="alert">{{ languageError }}</p>
    </section>
    <ThemePicker />
    <section class="card stack" aria-labelledby="h-profiles">
      <h2 id="h-profiles">{{ t("settings.stationProfiles") }}</h2>
      <ul v-if="snapshot?.profiles.length" class="profiles">
        <li v-for="p in snapshot.profiles" :key="p.id" :class="{ active: p.id === snapshot.activeProfileId }">
          <span class="mono call">{{ p.callsign }}</span>
          <span class="muted small">{{ where(p) }}</span>
          <span v-if="p.id === snapshot.activeProfileId" class="ok small">{{ t("settings.inUse") }}</span>
          <span class="grow"></span>
          <template v-if="confirming === p.id">
            <p class="confirm small" role="alert">
              {{ t("settings.deleteQuestion", { callsign: p.callsign }) }}
              <template v-if="p.id === snapshot.activeProfileId">{{ t("settings.deleteInUse") }} </template>
              {{ t("settings.deleteKeeps") }}
            </p>
            <button class="danger" :disabled="working" @click="remove(p.id)">{{ t("settings.deleteConfirm", { callsign: p.callsign }) }}</button>
            <button :disabled="working" @click="confirming = null">{{ t("settings.cancel") }}</button>
          </template>
          <template v-else>
            <button v-if="p.id === snapshot.activeProfileId" :disabled="working" @click="select(null)">{{ t("settings.stopUsing") }}</button>
            <button v-else :disabled="working" @click="select(p.id)">{{ t("settings.use") }}</button>
            <button class="quiet" :disabled="working" :aria-label="t('settings.deleteAria', { callsign: p.callsign })" @click="confirming = p.id">{{ t("settings.delete") }}</button>
          </template>
        </li>
      </ul>
      <p v-else class="muted">{{ t("settings.noProfile") }}</p>
      <p v-if="error" class="err" role="alert">{{ error }}</p>
    </section>
    <h2>{{ t("settings.addStation") }}</h2>
    <ProfileForms />
  </div>
</template>

<style scoped>
.settings { padding: calc(var(--space) * 3); display: grid; gap: calc(var(--space) * 2); overflow-y: auto; height: 100%; align-content: start; }
h1 { font-size: calc(20px * var(--font-scale)); }
.profiles { list-style: none; margin: 0; padding: 0; display: grid; gap: 6px; }
.profiles li { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space); padding: 6px 8px; border-radius: 8px; background: var(--raised); }
.profiles li.active { outline: 1px solid var(--success); }
.call { font-weight: 650; }
.lang select { max-width: 24em; }
.confirm { flex-basis: 100%; color: var(--warning); }
</style>
