<script setup lang="ts">
// Settings → Rig (M3b.1 design §5), laid out after VarAC's rig screen (owner,
// 21 Nov 2026): PTT and Frequency control as two dropdowns, MercuryChat's own
// rigctld's CAT settings, the external rigctld and FLRig boxes, a PTT test
// (Mercury's Tune) and a frequency test, and the CAT log. Mercury keys the
// radio in every case; MercuryChat sets Mercury's PTT to match. Below: the
// per-band levels, the frequency list and the QSY schedule.
import { computed, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { call, describeError } from "../api";
import type { HamlibModelView } from "../bindings/HamlibModelView";
import type { RigConfig } from "../bindings/RigConfig";
import type { RigTestStep } from "../bindings/RigTestStep";
import type { Schedule } from "../bindings/Schedule";
import {
  BAUDS,
  DATA_MODES,
  defaultRigConfig,
  FREQ_CONTROLS,
  LINE_STATES,
  LINKS,
  PARITIES,
  pttChoices,
  PTT_LINES,
  READ_EVERY,
  visibleBoxes,
} from "../rigForm";
import { activeProfile, snapshot } from "../store";
import BandLevelsEditor from "./BandLevels.vue";
import FrequencyListEditor from "./FrequencyListEditor.vue";
import ScheduleEditor from "./ScheduleEditor.vue";
import Switch from "./Switch.vue";

const { t } = useI18n();
const STEPS = ["connect", "caps", "frequency", "mode", "ptt", "owner"] as const;
const REGIONS = [1, 2, 3];
const HAMLIB_RELEASES = "https://github.com/Hamlib/Hamlib/releases";

const config = ref<RigConfig>(defaultRigConfig());
const region = ref(1);
const schedule = ref<Schedule>({ enabled: false, slots: [] });
const error = ref("");
const saved = ref(false);
const steps = ref<RigTestStep[] | null>(null);
const testing = ref(false);
const models = ref<HamlibModelView[]>([]);
const hamlibFound = ref<boolean | null>(null);
const modelQuery = ref("");
const catLog = ref<string[]>([]);
const logOpen = ref(false);
const copied = ref(false);
const testHz = ref(14_105_000);
const freqResult = ref("");

const boxes = computed(() => visibleBoxes(config.value));
const pttOptions = computed(() => pttChoices(config.value.freqControl));
const rig = computed(() => snapshot.value?.rig ?? null);
const tuning = computed(() => snapshot.value?.tune.active ?? false);

const modelLabel = (m: HamlibModelView) => `${m.id}  ${m.mfg} ${m.model}`.trim();
function syncModelQuery() {
  const m = models.value.find((x) => x.id === config.value.cat.model);
  modelQuery.value = m ? modelLabel(m) : config.value.cat.model != null ? String(config.value.cat.model) : "";
}
function pickModel() {
  const id = Number.parseInt(modelQuery.value, 10);
  config.value.cat.model = Number.isFinite(id) && id > 0 ? id : null;
}

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
  syncModelQuery();
}
watch(() => activeProfile.value?.id, load, { immediate: true });

async function loadModels() {
  try {
    const r = await call({ type: "hamlibModels", hamlibDir: config.value.hamlibDir });
    if (r.type === "hamlibModels") {
      models.value = r.models;
      hamlibFound.value = r.found;
      syncModelQuery();
    }
  } catch (e) {
    hamlibFound.value = false;
    error.value = describeError(e);
  }
}
watch(
  () => config.value.freqControl === "managedRigctld",
  (on) => {
    if (on && hamlibFound.value === null) void loadModels();
  },
  { immediate: true },
);
// CAT PTT is not offered with FLRig or no rig link: fall back to Mercury's own setting.
watch(pttOptions, (opts) => {
  if (!opts.includes(config.value.ptt.method)) config.value.ptt.method = "leaveMercury";
});

/** The form as the core takes it: numbers as numbers. */
function current(): RigConfig {
  const c = config.value;
  const n = (v: unknown, d: number) => (v !== "" && Number.isFinite(Number(v)) ? Number(v) : d);
  return {
    ...c,
    external: { ...c.external, port: n(c.external.port, 4532) },
    flrig: { ...c.flrig, port: n(c.flrig.port, 12345) },
    cat: {
      ...c.cat,
      baud: n(c.cat.baud, 9600),
      dataBits: n(c.cat.dataBits, 8),
      stopBits: n(c.cat.stopBits, 1),
      waitMs: n(c.cat.waitMs, 0),
    },
    ptt: { ...c.ptt, cm108Gpio: n(c.ptt.cm108Gpio, 3) },
    offsetHz: Math.round(n(c.offsetHz, 0)),
    readEveryS: n(c.readEveryS, 2),
  };
}
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
async function pttTest(on: boolean) {
  error.value = "";
  try {
    await call({ type: "tune", on });
  } catch (e) {
    error.value = describeError(e);
  }
}
async function freqTest() {
  error.value = "";
  freqResult.value = "";
  try {
    await call({ type: "rigSet", change: { type: "frequency", hz: Math.round(Number(testHz.value)) } });
    await call({ type: "rigSet", change: { type: "mode", mode: config.value.dataMode, passbandHz: 0 } });
    setTimeout(() => {
      const f = rig.value?.frequency?.value;
      freqResult.value =
        f == null ? t("rigSettings.freqTestNone") : t("rigSettings.freqTestResult", { freq: f, mode: rig.value?.mode ?? "?" });
    }, 2000);
  } catch (e) {
    error.value = describeError(e);
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

async function refreshLog() {
  try {
    const r = await call({ type: "catLog" });
    if (r.type === "catLog") catLog.value = r.lines;
  } catch {
    // The log is a convenience: a failed read leaves the last one shown.
  }
}
let logTimer: ReturnType<typeof setInterval> | null = null;
watch(logOpen, (open) => {
  if (logTimer) clearInterval(logTimer);
  logTimer = null;
  if (open) {
    void refreshLog();
    logTimer = setInterval(refreshLog, 2000);
  }
});
onUnmounted(() => {
  if (logTimer) clearInterval(logTimer);
});
async function copyLog() {
  try {
    await navigator.clipboard.writeText(catLog.value.join("\n"));
    copied.value = true;
    setTimeout(() => (copied.value = false), 1500);
  } catch {
    copied.value = false;
  }
}

/** MercuryChat's own rigctld, as the rig view reports it. */
const daemonText = computed(() => {
  const d = rig.value?.daemon;
  if (!d || config.value.freqControl !== "managedRigctld") return "";
  if (d.startsWith("failed: ")) return t("rigSettings.daemon.failed", { reason: d.slice(8) });
  switch (d) {
    case "starting":
      return t("rigSettings.daemon.starting");
    case "running":
      return t("rigSettings.daemon.running");
    case "reused":
      return t("rigSettings.daemon.reused");
    case "restarting":
      return t("rigSettings.daemon.restarting");
    case "notFound":
      return t("rigSettings.daemon.notFound");
    default:
      return d;
  }
});

const mark = (ok: boolean | null) => (ok === true ? "✓" : ok === false ? "✗" : "–");
const tone = (ok: boolean | null) => (ok === true ? "ok" : ok === false ? "err" : "muted");
const stepName = (name: string) => ((STEPS as readonly string[]).includes(name) ? t(`rigSettings.step.${name}`) : name);
</script>

<template>
  <section class="card stack rig-settings span-all" aria-labelledby="h-rig">
    <h2 id="h-rig">{{ t("rigSettings.title") }}</h2>
    <p v-if="!activeProfile" class="muted">{{ t("rigSettings.noStation") }}</p>
    <div v-else class="cols">
      <!-- Left: PTT, then the CAT link of MercuryChat's own rigctld, then the log. -->
      <div class="stack">
        <fieldset class="box">
          <legend>{{ t("rigSettings.ptt") }}</legend>
          <label class="row">
            <span>{{ t("rigSettings.pttMethod") }}</span>
            <select v-model="config.ptt.method" class="rig-ptt">
              <option v-for="m in pttOptions" :key="m" :value="m">{{ t(`rigSettings.pttOpt.${m}`) }}</option>
            </select>
          </label>
          <div v-if="boxes.has('serialPtt')" class="grid3">
            <label>
              {{ t("rigSettings.pttLine") }}
              <select v-model="config.ptt.line" class="rig-ptt-line">
                <option v-for="l in PTT_LINES" :key="l" :value="l">{{ t(`rigSettings.pttLineOpt.${l}`) }}</option>
              </select>
            </label>
            <label>
              {{ t("rigSettings.pttPort") }}
              <input v-model="config.ptt.pttPort" class="rig-ptt-port" spellcheck="false" :placeholder="t('rigSettings.pttPortHint')" />
            </label>
            <label class="check"><input v-model="config.ptt.invert" type="checkbox" class="rig-ptt-invert" /> {{ t("rigSettings.invert") }}</label>
          </div>
          <label v-if="boxes.has('cm108')" class="row">
            <span>{{ t("rigSettings.cm108Gpio") }}</span>
            <select v-model.number="config.ptt.cm108Gpio" class="rig-cm108">
              <option v-for="g in [1, 2, 3, 4]" :key="g" :value="g">{{ g }}</option>
            </select>
          </label>
          <div class="buttons">
            <button class="rig-ptt-test" :title="t('rigSettings.pttTestHint')" :disabled="tuning" @click="pttTest(true)">{{ t("rigSettings.pttTest") }}</button>
            <button class="rig-ptt-stop" :disabled="!tuning" @click="pttTest(false)">{{ t("rigSettings.pttStop") }}</button>
          </div>
          <p class="muted small">{{ t("rigSettings.pttHint") }}</p>
        </fieldset>

        <fieldset v-if="boxes.has('cat')" class="box">
          <legend>{{ t("rigSettings.cat") }}</legend>
          <label class="row">
            <span>{{ t("rigSettings.model") }}</span>
            <input v-model="modelQuery" class="rig-model" list="rig-models" spellcheck="false" :placeholder="t('rigSettings.modelSearch')" @change="pickModel" />
          </label>
          <datalist id="rig-models">
            <option v-for="m in models" :key="m.id" :value="modelLabel(m)" />
          </datalist>
          <p v-if="hamlibFound === false" class="err small" role="status">
            {{ t("rigSettings.hamlibNotFound") }}
            <a :href="HAMLIB_RELEASES" target="_blank" rel="noopener">{{ t("rigSettings.getHamlib") }}</a>
          </p>
          <label class="row">
            <span>{{ t("rigSettings.hamlibDir") }}</span>
            <input v-model="config.hamlibDir" class="rig-hamlib-dir" spellcheck="false" @change="loadModels" />
          </label>
          <div class="grid3">
            <label>
              {{ t("rigSettings.link") }}
              <select v-model="config.cat.link" class="rig-link">
                <option v-for="l in LINKS" :key="l" :value="l">{{ t(`rigSettings.linkOpt.${l}`) }}</option>
              </select>
            </label>
            <label v-if="config.cat.link === 'serial'">
              {{ t("rigSettings.serialPort") }}
              <input v-model="config.cat.serialPort" class="rig-serial-port" spellcheck="false" :placeholder="t('rigSettings.serialPortExample')" />
            </label>
            <label v-else>
              {{ t("rigSettings.tcpAddr") }}
              <input v-model="config.cat.tcpAddr" class="rig-tcp-addr" spellcheck="false" :placeholder="t('rigSettings.tcpAddrExample')" />
            </label>
            <label v-if="config.cat.link === 'serial'">
              {{ t("rigSettings.baud") }}
              <select v-model.number="config.cat.baud" class="rig-baud">
                <option v-for="b in BAUDS" :key="b" :value="b">{{ b }}</option>
              </select>
            </label>
          </div>
          <div v-if="config.cat.link === 'serial'" class="grid3">
            <label>
              {{ t("rigSettings.parity") }}
              <select v-model="config.cat.parity" class="rig-parity">
                <option v-for="p in PARITIES" :key="p" :value="p">{{ t(`rigSettings.parityOpt.${p}`) }}</option>
              </select>
            </label>
            <label>
              {{ t("rigSettings.dataBits") }}
              <select v-model.number="config.cat.dataBits" class="rig-data-bits">
                <option :value="7">7</option>
                <option :value="8">8</option>
              </select>
            </label>
            <label>
              {{ t("rigSettings.stopBits") }}
              <select v-model.number="config.cat.stopBits" class="rig-stop-bits">
                <option :value="1">1</option>
                <option :value="2">2</option>
              </select>
            </label>
            <label>
              {{ t("rigSettings.waitMs") }}
              <input v-model.number="config.cat.waitMs" class="rig-wait" type="number" min="0" max="1000" />
            </label>
            <label>
              {{ t("rigSettings.dtr") }}
              <select v-model="config.cat.dtr" class="rig-dtr">
                <option v-for="s in LINE_STATES" :key="s" :value="s">{{ t(`rigSettings.lineState.${s}`) }}</option>
              </select>
            </label>
            <label>
              {{ t("rigSettings.rts") }}
              <select v-model="config.cat.rts" class="rig-rts">
                <option v-for="s in LINE_STATES" :key="s" :value="s">{{ t(`rigSettings.lineState.${s}`) }}</option>
              </select>
            </label>
          </div>
          <p v-if="daemonText" class="small" role="status">{{ daemonText }}</p>
        </fieldset>

        <details class="box cat-log" @toggle="logOpen = ($event.target as HTMLDetailsElement).open">
          <summary>{{ t("rigSettings.catLog") }}</summary>
          <pre class="log small">{{ catLog.length ? catLog.join("\n") : t("rigSettings.logEmpty") }}</pre>
          <button class="rig-log-copy" :disabled="!catLog.length" @click="copyLog">{{ copied ? t("rigSettings.copied") : t("rigSettings.copy") }}</button>
        </details>
      </div>

      <!-- Right: Frequency control and what goes with it. -->
      <div class="stack">
        <fieldset class="box">
          <legend>{{ t("rigSettings.freqControl") }}</legend>
          <select v-model="config.freqControl" class="rig-freq-control" :aria-label="t('rigSettings.freqControl')">
            <option v-for="f in FREQ_CONTROLS" :key="f" :value="f">{{ t(`rigSettings.freqControlOpt.${f}`) }}</option>
          </select>
          <label class="switch-row">
            <span>{{ t("rigSettings.restore") }}</span>
            <Switch input-class="rig-restore" :checked="config.restoreOnStartup" @change="config.restoreOnStartup = ($event.target as HTMLInputElement).checked" />
          </label>
          <div class="grid3">
            <label>
              {{ t("rigSettings.offset") }}
              <input v-model.number="config.offsetHz" class="rig-offset" type="number" step="1" />
            </label>
            <label>
              {{ t("rigSettings.readEvery") }}
              <select v-model.number="config.readEveryS" class="rig-read-every">
                <option v-for="s in READ_EVERY" :key="s" :value="s">{{ t("rigSettings.seconds", { n: s }) }}</option>
              </select>
            </label>
            <label>
              {{ t("rigSettings.dataMode") }}
              <select v-model="config.dataMode" class="rig-data-mode">
                <option v-for="m in DATA_MODES" :key="m" :value="m">{{ m }}</option>
              </select>
            </label>
          </div>
          <div v-if="config.freqControl !== 'none'" class="buttons">
            <label>
              {{ t("rigSettings.freqTestHz") }}
              <input v-model.number="testHz" class="rig-test-hz" type="number" min="1" />
            </label>
            <button class="rig-freq-test" :title="t('rigSettings.freqTestHint')" @click="freqTest">{{ t("rigSettings.freqTest") }}</button>
          </div>
          <p v-if="freqResult" class="small" role="status">{{ freqResult }}</p>
        </fieldset>

        <fieldset v-if="boxes.has('external')" class="box">
          <legend>{{ t("rigSettings.external") }}</legend>
          <div class="grid3">
            <label>
              {{ t("rigSettings.host") }}
              <input v-model="config.external.host" class="rig-host" spellcheck="false" />
            </label>
            <label>
              {{ t("rigSettings.port") }}
              <input v-model.number="config.external.port" class="rig-port" type="number" min="1" max="65535" />
            </label>
          </div>
          <div class="buttons">
            <button class="rig-test" :disabled="testing" :title="t('rigSettings.testHint')" @click="test">{{ testing ? t("rigSettings.testing") : t("rigSettings.test") }}</button>
          </div>
          <ol v-if="steps" class="rig-test-steps small">
            <li v-for="s in steps" :key="s.name" :class="['step', tone(s.ok)]" :data-step="s.name">
              <span class="mark" aria-hidden="true">{{ mark(s.ok) }}</span>
              <b>{{ stepName(s.name) }}</b>
              <span class="detail">{{ s.detail }}</span>
            </li>
          </ol>
        </fieldset>

        <fieldset v-if="boxes.has('flrig')" class="box">
          <legend>{{ t("rigSettings.flrig") }}</legend>
          <div class="grid3">
            <label>
              {{ t("rigSettings.host") }}
              <input v-model="config.flrig.host" class="rig-flrig-host" spellcheck="false" />
            </label>
            <label>
              {{ t("rigSettings.port") }}
              <input v-model.number="config.flrig.port" class="rig-flrig-port" type="number" min="1" max="65535" />
            </label>
          </div>
          <p class="muted small">{{ t("rigSettings.flrigHint") }}</p>
        </fieldset>

        <label class="row">
          <span>{{ t("rigSettings.region") }}</span>
          <select class="rig-region" :value="region" @change="chooseRegion(Number(($event.target as HTMLSelectElement).value))">
            <option v-for="r in REGIONS" :key="r" :value="r">{{ t("rigSettings.regionOpt", { n: r }) }}</option>
          </select>
        </label>
        <div class="buttons">
          <button class="rig-save" @click="save">{{ t("rigSettings.save") }}</button>
          <span v-if="saved" class="ok small" role="status">{{ t("rigSettings.saved") }}</span>
        </div>
        <p v-if="error" class="err" role="alert">{{ error }}</p>
      </div>
    </div>
  </section>
  <BandLevelsEditor v-if="activeProfile" />
  <FrequencyListEditor v-if="activeProfile" class="span-all" />
  <ScheduleEditor v-if="activeProfile" :initial="schedule" />
</template>

<style scoped>
.cols { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 320px), 1fr)); gap: calc(var(--space) * 2); align-items: start; }
.box { border: 1px solid var(--border); border-radius: var(--radius); padding: var(--space) calc(var(--space) * 1.5); margin: 0; display: grid; gap: 6px; }
.box legend { padding: 0 4px; font-weight: 600; }
.row { display: grid; grid-template-columns: minmax(8em, auto) 1fr; gap: 8px; align-items: center; }
.grid3 { display: grid; grid-template-columns: repeat(auto-fit, minmax(7.5em, 1fr)); gap: 6px 10px; }
.grid3 label { display: grid; gap: 2px; }
.check { display: flex; gap: 6px; align-items: center; align-self: end; }
.buttons { display: flex; gap: var(--space); align-items: end; flex-wrap: wrap; }
.rig-test-steps { margin: 0; padding-left: 0; list-style: none; display: grid; gap: 2px; }
.step { display: grid; grid-template-columns: 1.5em 9em 1fr; gap: 6px; }
.log { max-height: 14em; overflow: auto; white-space: pre-wrap; margin: 4px 0; font-family: var(--font-mono); }
.cat-log summary { cursor: pointer; font-weight: 600; }
.rig-test-hz { width: 9em; }
</style>
