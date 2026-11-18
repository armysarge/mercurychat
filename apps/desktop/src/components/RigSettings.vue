<script setup lang="ts">
// Settings → Rig (M3b): how MercuryChat reads the rig, the rigctld address,
// the step-by-step link test (its own connection, reads only), the region for
// suggested frequencies, restore-on-startup, then the frequency list, the
// per-band levels and the QSY schedule.
import { ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { call, describeError } from "../api";
import type { RigConfig } from "../bindings/RigConfig";
import type { RigTestStep } from "../bindings/RigTestStep";
import type { Schedule } from "../bindings/Schedule";
import { activeProfile } from "../store";
import BandLevelsEditor from "./BandLevels.vue";
import FrequencyListEditor from "./FrequencyListEditor.vue";
import ScheduleEditor from "./ScheduleEditor.vue";
import Switch from "./Switch.vue";

const { t } = useI18n();
const ARRANGEMENTS = ["none", "rigctld", "mercury"] as const;
const STEPS = ["connect", "caps", "frequency", "mode", "ptt", "owner"] as const;
const REGIONS = [1, 2, 3];

const config = ref<RigConfig>({ arrangement: "mercury", host: "127.0.0.1", port: 4532, restoreOnStartup: false });
const region = ref(1);
const schedule = ref<Schedule>({ enabled: false, slots: [] });
const error = ref("");
const saved = ref(false);
const steps = ref<RigTestStep[] | null>(null);
const testing = ref(false);

async function load() {
  steps.value = null;
  if (!activeProfile.value) return;
  try {
    const r = await call({ type: "rigSettings" });
    if (r.type === "rigSettings") {
      config.value = r.config;
      region.value = r.region;
      schedule.value = r.schedule;
    }
  } catch (e) {
    error.value = describeError(e);
  }
}
watch(() => activeProfile.value?.id, load, { immediate: true });

const current = () => ({ ...config.value, port: Number(config.value.port) });
async function save() {
  error.value = "";
  saved.value = false;
  try {
    await call({ type: "setRigConfig", config: current() });
    saved.value = true;
  } catch (e) {
    error.value = describeError(e);
  }
}
async function test() {
  testing.value = true;
  steps.value = null;
  error.value = "";
  try {
    const r = await call({ type: "testRig", config: current() });
    if (r.type === "rigTest") steps.value = r.steps;
  } catch (e) {
    error.value = describeError(e);
  } finally {
    testing.value = false;
  }
}
async function chooseRegion(r: number) {
  error.value = "";
  try {
    await call({ type: "setRegion", region: r });
    region.value = r;
  } catch (e) {
    error.value = describeError(e);
  }
}
const mark = (ok: boolean | null) => (ok === true ? "✓" : ok === false ? "✗" : "–");
const tone = (ok: boolean | null) => (ok === true ? "ok" : ok === false ? "err" : "muted");
const stepName = (name: string) => ((STEPS as readonly string[]).includes(name) ? t(`rigSettings.step.${name}`) : name);
</script>

<template>
  <section class="card stack rig-settings" aria-labelledby="h-rig">
    <h2 id="h-rig">{{ t("rigSettings.title") }}</h2>
    <p v-if="!activeProfile" class="muted">{{ t("rigSettings.noStation") }}</p>
    <template v-else>
      <fieldset class="arrangement">
        <legend>{{ t("rigSettings.arrangement") }}</legend>
        <label v-for="a in ARRANGEMENTS" :key="a" class="radio">
          <input v-model="config.arrangement" type="radio" name="rig-arrangement" :value="a" />
          <span>{{ t(`rigSettings.arrangementOpt.${a}`) }}</span>
        </label>
        <p class="muted small">{{ t("rigSettings.hint") }}</p>
      </fieldset>
      <div v-if="config.arrangement === 'rigctld'" class="address">
        <label>{{ t("rigSettings.host") }} <input v-model="config.host" class="rig-host" spellcheck="false" /></label>
        <label>{{ t("rigSettings.port") }} <input v-model.number="config.port" class="rig-port" type="number" min="1" max="65535" /></label>
      </div>
      <label class="switch-row">
        <span>{{ t("rigSettings.restore") }}</span>
        <Switch input-class="rig-restore" :checked="config.restoreOnStartup" @change="config.restoreOnStartup = ($event.target as HTMLInputElement).checked" />
      </label>
      <div class="buttons">
        <button class="rig-save" @click="save">{{ t("rigSettings.save") }}</button>
        <button v-if="config.arrangement === 'rigctld'" class="rig-test" :disabled="testing" :title="t('rigSettings.testHint')" @click="test">{{ testing ? t("rigSettings.testing") : t("rigSettings.test") }}</button>
        <span v-if="saved" class="ok small" role="status">{{ t("rigSettings.saved") }}</span>
      </div>
      <ol v-if="steps" class="rig-test-steps small">
        <li v-for="s in steps" :key="s.name" :class="['step', tone(s.ok)]" :data-step="s.name">
          <span class="mark" aria-hidden="true">{{ mark(s.ok) }}</span>
          <b>{{ stepName(s.name) }}</b>
          <span class="detail">{{ s.detail }}</span>
        </li>
      </ol>
      <label class="region">
        {{ t("rigSettings.region") }}
        <select class="rig-region" :value="region" @change="chooseRegion(Number(($event.target as HTMLSelectElement).value))">
          <option v-for="r in REGIONS" :key="r" :value="r">{{ t("rigSettings.regionOpt", { n: r }) }}</option>
        </select>
      </label>
      <p v-if="error" class="err" role="alert">{{ error }}</p>
    </template>
  </section>
  <FrequencyListEditor v-if="activeProfile" />
  <BandLevelsEditor v-if="activeProfile" />
  <ScheduleEditor v-if="activeProfile" :initial="schedule" />
</template>

<style scoped>
.arrangement { border: 0; padding: 0; margin: 0; display: grid; gap: 4px; }
.radio { display: flex; gap: 8px; align-items: center; }
.address { display: flex; gap: calc(var(--space) * 2); flex-wrap: wrap; }
.rig-port { width: 7em; }
.buttons { display: flex; gap: var(--space); align-items: center; }
.rig-test-steps { margin: 0; padding-left: 0; list-style: none; display: grid; gap: 2px; }
.step { display: grid; grid-template-columns: 1.5em 9em 1fr; gap: 6px; }
.region select { max-width: 12em; }
</style>
