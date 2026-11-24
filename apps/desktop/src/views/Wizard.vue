<script setup lang="ts">
// The first-run wizard (M5 §6; 05 §M5 steps 1–6 and 8, and a tour). Each
// step but Welcome, Modem and Done can be skipped; the progress is kept by
// the core, so a restart resumes here. Its station stays in Receive only
// until "How you operate" is answered (the core holds it). Nothing transmits
// except on "Key the radio for 3 s" (SET-02). The settings it shows are the
// same components as Settings, so a step revisited later looks the same.
import { invoke } from "@tauri-apps/api/core";
import { computed, onUnmounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { call, describeError } from "../api";
import type { AppError } from "../bindings/AppError";
import type { StationProfile } from "../bindings/StationProfile";
import type { WizardState } from "../bindings/WizardState";
import type { WizardStep } from "../bindings/WizardStep";
import AudioSettings from "../components/AudioSettings.vue";
import ProfileForms from "../components/ProfileForms.vue";
import RigSettings from "../components/RigSettings.vue";
import Waterfall from "../components/Waterfall.vue";
import { builtin, settingsTabRequest } from "../builtin";
import { app, snapshot } from "../store";
import { back, canSkip, next, skip, stepsFor, type WizardPath } from "../wizard";
import { saveWizard, wizard } from "../wizardState";

const { t } = useI18n();

const state = computed<WizardState>(() => wizard.value ?? { step: "welcome", draftProfileId: null, done: false, skipped: [], station: null });
const draft = computed(() => snapshot.value?.profiles.find((p) => p.id === state.value.draftProfileId) ?? null);
const pathOf = (p: StationProfile | null): WizardPath => (p?.modem.kind === "builtIn" ? "builtIn" : p?.modem.kind === "existing" ? "existing" : null);
const path = computed(() => pathOf(draft.value));
const steps = computed(() => stepsFor(path.value));
const step = computed(() => state.value.step);
const position = computed(() => Math.max(0, steps.value.indexOf(step.value)) + 1);

const error = ref("");
const busy = ref(false);
async function save(s: WizardState) {
  busy.value = true;
  error.value = "";
  try {
    await saveWizard(s);
  } catch (e) {
    error.value = describeError(e);
  } finally {
    busy.value = false;
  }
}
const toBack = () => save({ ...state.value, step: back(step.value, path.value) });
const toNext = () => save({ ...state.value, step: next(step.value, path.value) });
const toSkip = () => save(skip(state.value, path.value));

// ---- Station: what fills the macros, the QSO defaults and the profile.
const st = ref({
  callsign: state.value.station?.callsign ?? draft.value?.callsign ?? "",
  name: state.value.station?.name ?? draft.value?.name ?? "",
  grid: state.value.station?.grid ?? draft.value?.grid ?? "",
});
const stErrors = ref<Record<string, string>>({});
async function stationNext() {
  stErrors.value = {};
  const station = { callsign: st.value.callsign.trim(), name: st.value.name.trim() || null, grid: st.value.grid.trim() || null };
  // The core checks it as it will the profile: the same messages, here.
  const probe: StationProfile = {
    id: "", callsign: station.callsign, name: station.name, grid: station.grid, lab: false, receiveOnly: true,
    modem: { kind: "existing", host: "127.0.0.1", controlPort: 8300, dataPort: 8301, broadcastPort: null, telemetryUrl: null },
  };
  try {
    await call({ type: "validateProfile", profile: probe });
  } catch (e) {
    const err = e as AppError;
    if (err && typeof err === "object" && err.code === "validation") {
      for (const f of err.errors) stErrors.value[f.field] = f.message;
      return;
    }
    error.value = describeError(e);
    return;
  }
  await save({ ...state.value, station, step: next("station", path.value) });
}

// ---- Modem: the forms make and select the station, in Receive only.
function modemDone(p: StationProfile) {
  return save({ ...state.value, draftProfileId: p.id, step: next("modem", pathOf(p)) });
}

// ---- Rig step: the transmit test only on an explicit press (SET-02).
const keyLeft = ref(0);
let keyTimer: ReturnType<typeof setInterval> | null = null;
async function unkey() {
  if (keyTimer) clearInterval(keyTimer);
  keyTimer = null;
  keyLeft.value = 0;
  try {
    await call({ type: "tune", on: false });
  } catch (e) {
    error.value = describeError(e);
  }
}
async function keyRadio() {
  error.value = "";
  try {
    await call({ type: "tune", on: true });
  } catch (e) {
    error.value = describeError(e);
    return;
  }
  keyLeft.value = 3;
  keyTimer = setInterval(() => {
    keyLeft.value -= 1;
    if (keyLeft.value <= 0) void unkey();
  }, 1000);
}
onUnmounted(() => {
  if (keyTimer) void unkey();
});

// ---- How you operate (D18): the choice is applied once the step is passed.
/** Nothing preselected: the operator says which (the station is held in Receive only until then). */
const answering = ref<boolean | null>(null);
async function operateNext() {
  if (answering.value === null) return;
  // Past this step the core no longer holds Receive only; then the choice.
  await save({ ...state.value, step: next("operate", path.value) });
  if (error.value) return;
  try {
    await call({ type: "setReceiveOnly", on: !answering.value });
  } catch (e) {
    error.value = describeError(e);
  }
}

// ---- Done.
async function finish(then?: () => void) {
  await save({ ...state.value, step: "done", done: true });
  if (error.value) return;
  app.page = "operate";
  then?.();
}
/** The system browser, through the shell's named links (as About): never a URL from the page. */
async function openLink(link: "project" | "releases") {
  error.value = "";
  try {
    await invoke("open_link", { link });
  } catch (e) {
    error.value = describeError(e);
  }
}
const openAbout = () =>
  finish(() => {
    settingsTabRequest.value = "about";
    app.page = "settings";
  });
/** "Set up without the guide": the plain setup screen (or Operate, with a station). */
const leave = () => save({ ...state.value, step: "done", done: true });

const titleOf = (s: WizardStep) => t(`wizard.steps.${s}`);
</script>

<template>
  <div class="wizard">
    <nav class="progress" :aria-label="t('wizard.progressAria')">
      <ol>
        <li
          v-for="(s, i) in steps"
          :key="s"
          :class="{ current: s === step, past: i + 1 < position, skipped: state.skipped.includes(s) }"
          :aria-current="s === step ? 'step' : undefined"
        >
          <span class="n" aria-hidden="true">{{ i + 1 }}</span>
          <span>{{ titleOf(s) }}</span>
          <span v-if="state.skipped.includes(s)" class="muted small">{{ t("wizard.skippedMark") }}</span>
        </li>
      </ol>
    </nav>

    <section class="body stack" :data-step="step" aria-live="polite">
      <p class="muted small count">{{ t("wizard.stepOf", { n: position, total: steps.length }) }}</p>

      <!-- 1. Welcome -->
      <template v-if="step === 'welcome'">
        <h1>{{ t("wizard.welcome.title") }}</h1>
        <p>{{ t("wizard.welcome.what") }}</p>
        <p class="muted">{{ t("wizard.welcome.need") }}</p>
        <ul class="need">
          <li>{{ t("wizard.welcome.needRadio") }}</li>
          <li>{{ t("wizard.welcome.needAudio") }}</li>
          <li>
            {{ builtin?.available && builtin.mercury ? t("wizard.welcome.needMercuryBuiltIn", { version: builtin.mercury }) : t("wizard.welcome.needMercury") }}
          </li>
        </ul>
        <p class="legal small">{{ t("wizard.welcome.legal") }}</p>
      </template>

      <!-- 2. Station -->
      <template v-else-if="step === 'station'">
        <h1>{{ t("wizard.station.title") }}</h1>
        <p class="muted">{{ t("wizard.station.body") }}</p>
        <form class="fields" @submit.prevent="stationNext">
          <label>{{ t("profile.callsign") }} <input v-model="st.callsign" name="callsign" class="mono" maxlength="15" autocomplete="off" spellcheck="false" /></label>
          <span v-if="stErrors.callsign" class="field-error">{{ stErrors.callsign }}</span>
          <label>{{ t("wizard.station.name") }} <input v-model="st.name" name="name" maxlength="64" autocomplete="off" /></label>
          <span v-if="stErrors.name" class="field-error">{{ stErrors.name }}</span>
          <label>{{ t("wizard.station.grid") }} <input v-model="st.grid" name="grid" class="mono" maxlength="8" autocomplete="off" spellcheck="false" /></label>
          <span v-if="stErrors.grid" class="field-error">{{ stErrors.grid }}</span>
          <p class="muted small">{{ t("wizard.station.more") }}</p>
          <button type="submit" hidden></button>
        </form>
      </template>

      <!-- 3. Modem -->
      <template v-else-if="step === 'modem'">
        <h1>{{ t("wizard.modem.title") }}</h1>
        <p class="muted">{{ t("wizard.modem.body") }}</p>
        <p v-if="draft" class="ok small kept">{{ t("wizard.modem.kept", { callsign: draft.callsign }) }}</p>
        <ProfileForms wizard receive-only :station="state.station" @done="modemDone" />
      </template>

      <!-- 4. Audio (built-in) -->
      <template v-else-if="step === 'audio'">
        <h1>{{ t("wizard.audio.title") }}</h1>
        <p class="muted">{{ t("wizard.audio.body") }}</p>
        <AudioSettings />
        <section class="card stack rx-check">
          <h2>{{ t("wizard.audio.rxTitle") }}</h2>
          <p class="muted small">{{ t("wizard.audio.rxBody") }}</p>
          <Waterfall :shown="true" />
        </section>
      </template>

      <!-- 5. Rig (built-in) -->
      <template v-else-if="step === 'rig'">
        <h1>{{ t("wizard.rig.title") }}</h1>
        <p class="muted">{{ t("wizard.rig.body") }}</p>
        <div class="rig-grid"><RigSettings basic /></div>
        <!-- Owner, 24 Nov: the transmit test belongs with the rig (PTT is set here). -->
        <section class="card stack tx-test">
          <h2>{{ t("wizard.audio.txTitle") }}</h2>
          <p class="muted small">{{ t("wizard.audio.txBody") }}</p>
          <div class="buttons">
            <button class="key-radio warn" :disabled="keyLeft > 0" @click="keyRadio">{{ t("wizard.audio.key") }}</button>
            <button v-if="keyLeft > 0" class="unkey" @click="unkey">{{ t("wizard.audio.stop") }}</button>
            <span v-if="keyLeft > 0" class="mono" role="timer" aria-live="assertive">{{ t("wizard.audio.left", { n: keyLeft }) }}</span>
          </div>
        </section>
      </template>

      <!-- 6. How you operate -->
      <template v-else-if="step === 'operate'">
        <h1>{{ t("wizard.operate.title") }}</h1>
        <div class="choices" role="radiogroup" :aria-label="t('wizard.operate.title')">
          <label :class="['choice', 'card', { chosen: answering === true }]">
            <input v-model="answering" type="radio" name="operate" class="op-answering" :value="true" />
            <b>{{ t("mode.answering") }}</b>
            <span class="muted small">{{ t("wizard.operate.answeringBody") }}</span>
          </label>
          <label :class="['choice', 'card', { chosen: answering === false }]">
            <input v-model="answering" type="radio" name="operate" class="op-receive-only" :value="false" />
            <b>{{ t("mode.receiveOnly") }}</b>
            <span class="muted small">{{ t("wizard.operate.receiveOnlyBody") }}</span>
          </label>
        </div>
        <p class="muted small">{{ t("wizard.operate.beacons") }}</p>
      </template>

      <!-- 7. First contact: the tour -->
      <template v-else-if="step === 'tour'">
        <h1>{{ t("wizard.tour.title") }}</h1>
        <ol class="tour">
          <li><b>{{ t("wizard.tour.sidebarTitle") }}</b> <span class="muted">{{ t("wizard.tour.sidebar") }}</span></li>
          <li><b>{{ t("wizard.tour.callTitle") }}</b> <span class="muted">{{ t("wizard.tour.call") }}</span></li>
          <li><b>{{ t("wizard.tour.cqTitle") }}</b> <span class="muted">{{ t("wizard.tour.cq") }}</span></li>
          <li><b>{{ t("wizard.tour.chatTitle") }}</b> <span class="muted">{{ t("wizard.tour.chat") }}</span></li>
        </ol>
      </template>

      <!-- 8. Done -->
      <template v-else>
        <h1>{{ t("wizard.done.title") }}</h1>
        <p class="muted">{{ t("wizard.done.body") }}</p>
        <p v-if="state.skipped.length" class="small">{{ t("wizard.done.skipped", { steps: state.skipped.map(titleOf).join(", ") }) }}</p>
        <ul class="links">
          <li><button class="quiet open-project" @click="openLink('project')">{{ t("wizard.done.manual") }}</button></li>
          <li><button class="quiet open-releases" @click="openLink('releases')">{{ t("wizard.done.updates") }}</button></li>
          <li><button class="quiet open-about" :disabled="busy" @click="openAbout">{{ t("wizard.done.about") }}</button></li>
        </ul>
      </template>

      <p v-if="error" class="err" role="alert">{{ error }}</p>

      <div class="nav">
        <button v-if="step !== 'welcome'" class="back" :disabled="busy" @click="toBack">{{ t("wizard.back") }}</button>
        <button v-else class="quiet skip-wizard" :disabled="busy" @click="leave">{{ t("wizard.leave") }}</button>
        <span class="grow"></span>
        <button v-if="canSkip(step)" class="quiet skip" :disabled="busy" @click="toSkip">{{ t("wizard.skip") }}</button>
        <button v-if="step === 'station'" class="primary next" :disabled="busy" @click="stationNext">{{ t("wizard.next") }}</button>
        <button v-else-if="step === 'modem'" class="primary next" :disabled="busy || !draft" :title="draft ? undefined : t('wizard.modem.chooseFirst')" @click="toNext">{{ t("wizard.next") }}</button>
        <button v-else-if="step === 'operate'" class="primary next" :disabled="busy || answering === null" @click="operateNext">{{ t("wizard.next") }}</button>
        <button v-else-if="step === 'done'" class="primary finish" :disabled="busy" @click="finish()">{{ t("wizard.finish") }}</button>
        <button v-else class="primary next" :disabled="busy" @click="toNext">{{ t("wizard.next") }}</button>
      </div>
    </section>
  </div>
</template>

<style scoped>
.wizard { height: 100%; overflow-y: auto; display: grid; grid-template-columns: minmax(180px, 220px) minmax(0, 1fr); align-items: start; align-content: start; }
.progress { position: sticky; top: 0; padding: calc(var(--space) * 3) calc(var(--space) * 2); }
.progress ol { list-style: none; margin: 0; padding: 0; display: grid; gap: 4px; }
.progress li { display: flex; gap: var(--space); align-items: center; padding: 6px 8px; border-radius: var(--radius); color: var(--text-2); }
.progress li.current { background: var(--raised); color: var(--accent); font-weight: 600; }
.progress li.past { color: var(--text); }
.progress .n {
  width: 1.6em; height: 1.6em; border-radius: 999px; display: inline-grid; place-items: center; flex: none;
  border: 1px solid currentColor; font-size: 0.85em;
}
.body { padding: calc(var(--space) * 3) calc(var(--space) * 4); max-width: 1000px; min-width: 0; }
.body h1 { font-size: calc(22px * var(--font-scale)); margin: 0; }
.count { margin: 0; }
.need, .tour, .links { margin: 0; padding-left: 1.2em; display: grid; gap: 6px; }
.legal { border-left: 3px solid var(--warning); padding-left: var(--space); }
.fields { display: grid; gap: 6px; max-width: 420px; }
.fields input { width: 100%; }
.choices { display: grid; gap: calc(var(--space) * 2); grid-template-columns: repeat(auto-fit, minmax(min(100%, 260px), 1fr)); }
.choice { display: grid; grid-template-columns: auto 1fr; gap: 6px var(--space); align-items: center; cursor: pointer; }
.choice input { margin: 0; }
.choice span { grid-column: 1 / -1; }
.choice.chosen { border-color: var(--accent); }
.buttons { display: flex; gap: var(--space); align-items: center; flex-wrap: wrap; }
.rig-grid { display: grid; gap: calc(var(--space) * 2); }
.nav { display: flex; gap: var(--space); align-items: center; flex-wrap: wrap; padding-top: var(--space); border-top: 1px solid var(--border); }
.nav .grow { flex: 1; }
/* A tablet in portrait: the steps become a row above the page. */
@media (max-width: 900px) {
  .wizard { grid-template-columns: minmax(0, 1fr); }
  .progress { position: static; padding: var(--space) calc(var(--space) * 2) 0; }
  .progress ol { display: flex; flex-wrap: wrap; gap: 4px; }
  .progress li { padding: 2px 6px; }
  .progress li:not(.current) span:not(.n) { display: none; }
  .body { padding: calc(var(--space) * 2); }
}
</style>
