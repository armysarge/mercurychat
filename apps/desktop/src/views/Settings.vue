<script setup lang="ts">
import { info } from "../diag";
// Settings (docs/04 §4), in tabs (owner request, 18 Nov): Station (its
// information, macros and the Logbook preference), Profiles (and new ones from
// the same forms as first use), Rig (M3b), Notifications, Appearance and
// About. The tab chosen is remembered in this window's storage. Areas built
// after 1.0 (email, AI, GPS, EmComm) get their own tabs when they exist.
import { computed, nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { call, describeError } from "../api";
import type { StationProfile } from "../bindings/StationProfile";
import About from "../components/About.vue";
import MacroEditor from "../components/MacroEditor.vue";
import NotificationSettings from "../components/NotificationSettings.vue";
import ProfileForms from "../components/ProfileForms.vue";
import AudioSettings from "../components/AudioSettings.vue";
import BeaconSettings from "../components/BeaconSettings.vue";
import FileSettings from "../components/FileSettings.vue";
import RigSettings from "../components/RigSettings.vue";
import StationInfo from "../components/StationInfo.vue";
import ThemePicker from "../components/ThemePicker.vue";
import { DRAFT_LOCALES, LANGUAGE_SETTINGS, NATIVE_NAMES, type Locale } from "../i18n/locale";
import { activeProfile, mirror, setFinaliseOnDisconnect, snapshot } from "../store";
import Switch from "../components/Switch.vue";

const { t } = useI18n();

const TABS = ["station", "profiles", "rig", "audio", "beacon", "files", "notifications", "appearance", "about"] as const;
type Tab = (typeof TABS)[number];
const TAB_KEY = "mc.settingsTab";
const tab = ref<Tab>(
  (() => {
    try {
      const v = localStorage.getItem(TAB_KEY);
      return (TABS as readonly string[]).includes(v ?? "") ? (v as Tab) : "station";
    } catch {
      return "station";
    }
  })(),
);
watch(tab, (v) => {
  try {
    localStorage.setItem(TAB_KEY, v);
  } catch {
    // Storage may be unavailable: the tab then lasts for this run only.
  }
});
const tabList = ref<HTMLElement | null>(null);
/** Arrow keys, Home and End move between tabs (and choose them). */
async function onTabKey(e: KeyboardEvent) {
  const i = TABS.indexOf(tab.value);
  const moves: Record<string, number> = { ArrowRight: i + 1, ArrowLeft: i - 1, Home: 0, End: TABS.length - 1 };
  const next = moves[e.key];
  if (next === undefined) return;
  e.preventDefault();
  tab.value = TABS[(next + TABS.length) % TABS.length];
  await nextTick();
  tabList.value?.querySelector<HTMLElement>(`[aria-selected="true"]`)?.focus();
}

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
const where = (p: StationProfile) =>
  p.modem.kind === "simulator"
    ? info.value?.simulator === false
      ? t("profile.simulatorUnavailable")
      : t("settings.simulator")
    : p.modem.kind === "builtIn"
      ? t("settings.builtIn")
      : `${p.modem.host}:${p.modem.controlPort}`;

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

const finaliseError = ref("");
async function chooseFinalise(on: boolean) {
  finaliseError.value = "";
  try {
    await setFinaliseOnDisconnect(on);
  } catch (e) {
    finaliseError.value = describeError(e);
  }
}
</script>

<template>
  <div class="settings">
    <h1>{{ t("settings.title") }}</h1>
    <div ref="tabList" class="tabs" role="tablist" :aria-label="t('settings.tabs.aria')" @keydown="onTabKey">
      <button
        v-for="name in TABS"
        :id="`tab-${name}`"
        :key="name"
        role="tab"
        :class="['tab', `tab-${name}`]"
        :aria-selected="tab === name"
        :aria-controls="`panel-${name}`"
        :tabindex="tab === name ? 0 : -1"
        @click="tab = name"
      >{{ t(`settings.tabs.${name}`) }}</button>
    </div>
    <div v-show="tab === 'appearance'" id="panel-appearance" class="panel" role="tabpanel" aria-labelledby="tab-appearance">
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
    </div>
    <div v-show="tab === 'station'" id="panel-station" class="panel" role="tabpanel" aria-labelledby="tab-station">
    <StationInfo />
    <section v-if="activeProfile" class="card stack" aria-labelledby="h-logbook">
      <h2 id="h-logbook">{{ t("logbook.title") }}</h2>
      <label class="switch-row">
        <span>{{ t("logbook.finaliseOnDisconnect") }}</span>
        <Switch input-class="finalise-on-disconnect" :checked="snapshot?.finaliseOnDisconnect ?? false" @change="chooseFinalise(($event.target as HTMLInputElement).checked)" />
      </label>
      <p class="muted small">{{ t("logbook.finaliseNote") }}</p>
      <p v-if="finaliseError" class="err" role="alert">{{ finaliseError }}</p>
    </section>
    <MacroEditor class="span-all" />
    </div>
    <div v-show="tab === 'profiles'" id="panel-profiles" class="panel" role="tabpanel" aria-labelledby="tab-profiles">
    <section class="card stack span-all" aria-labelledby="h-profiles">
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
    <h2 class="span-all">{{ t("settings.addStation") }}</h2>
    <ProfileForms class="span-all" />
    </div>
    <div v-show="tab === 'rig'" id="panel-rig" class="panel" role="tabpanel" aria-labelledby="tab-rig">
      <RigSettings />
    </div>
    <div v-show="tab === 'audio'" id="panel-audio" class="panel" role="tabpanel" aria-labelledby="tab-audio">
      <!-- Mounted only while shown: it asks the core for Mercury's devices every 2 s. -->
      <AudioSettings v-if="tab === 'audio'" />
    </div>
    <div v-show="tab === 'beacon'" id="panel-beacon" class="panel" role="tabpanel" aria-labelledby="tab-beacon">
      <BeaconSettings v-if="tab === 'beacon'" />
    </div>
    <div v-show="tab === 'files'" id="panel-files" class="panel" role="tabpanel" aria-labelledby="tab-files">
    <FileSettings />
    </div>
    <div v-show="tab === 'notifications'" id="panel-notifications" class="panel" role="tabpanel" aria-labelledby="tab-notifications">
      <NotificationSettings />
    </div>
    <div v-show="tab === 'about'" id="panel-about" class="panel" role="tabpanel" aria-labelledby="tab-about">
      <About />
    </div>
  </div>
</template>

<style scoped>
.settings { padding: calc(var(--space) * 3); display: grid; gap: calc(var(--space) * 2); overflow-y: auto; overflow-x: hidden; height: 100%; align-content: start; min-width: 0; }
.tabs { display: flex; flex-wrap: wrap; gap: 2px; border-bottom: 1px solid var(--border); }
.tab { border: 1px solid transparent; border-bottom: 0; border-radius: var(--radius) var(--radius) 0 0; background: transparent; padding: 4px 12px; color: var(--text-2); }
.tab[aria-selected="true"] { border-color: var(--border); background: var(--surface); color: var(--text); font-weight: 600; margin-bottom: -1px; }
/* Sections sit side by side where they fit (owner, 20 Nov); lists and
   multi-part forms take the whole width. */
.panel {
  display: grid; gap: calc(var(--space) * 1.5); align-content: start; align-items: start; min-width: 0;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 440px), 1fr));
}
.panel > .span-all, .panel :deep(.span-all) { grid-column: 1 / -1; }
h1 { font-size: calc(20px * var(--font-scale)); }
.profiles { list-style: none; margin: 0; padding: 0; display: grid; gap: 6px; }
.profiles li { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space); padding: 6px 8px; border-radius: 8px; background: var(--raised); }
.profiles li.active { outline: 1px solid var(--success); }
.call { font-weight: 650; }
.lang select { max-width: 24em; }
.confirm { flex-basis: 100%; color: var(--warning); }
</style>
