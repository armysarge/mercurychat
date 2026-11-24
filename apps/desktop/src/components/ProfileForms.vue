<script setup lang="ts">
// Make a station profile and use it (M5 §5.1): the built-in Mercury
// (recommended: MercuryChat starts its own Mercury and rigctld), my own
// Mercury at host:port, or the simulator in development builds. The core
// validates every field (profile.rs); its errors are shown next to the
// fields they name. A built-in profile's ports are checked first: a taken
// one is named, with a free set to use instead. My own Mercury's endpoints
// can be checked one by one, read-only (SET-03). In the wizard (M5 §6) the
// callsign, name and locator come from its Station step, the simulator is
// not offered, and the new station starts in Receive only.
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { call, describeError } from "../api";
import type { AppError } from "../bindings/AppError";
import type { BuiltInPorts } from "../bindings/BuiltInPorts";
import type { EndpointCheck } from "../bindings/EndpointCheck";
import type { StationProfile } from "../bindings/StationProfile";
import type { WizardStation } from "../bindings/WizardStation";
import { builtin } from "../builtin";
import { info } from "../diag";
import { snapshot } from "../store";

const { t } = useI18n();
const emit = defineEmits<{ done: [profile: StationProfile] }>();
// A new station starts in Receive only when first-use setup says so (D18),
// and always in the wizard until its "How you operate" step.
const props = defineProps<{ receiveOnly?: boolean; station?: WizardStation | null; wizard?: boolean }>();

/** The Station step's answers, when it gave a callsign: the forms then do not ask again. */
const given = computed(() => (props.station?.callsign.trim() ? props.station : null));
const simCall = ref("");
const bi = ref({ callsign: "", control: 8300, broadcast: 8100, ui: 10000, rigctld: 4532 });
const taken = ref<{ port: number; suggestion: BuiltInPorts } | null>(null);
const builtinTitle = computed(() =>
  builtin.value?.mercury ? t("profile.builtInTitle", { version: builtin.value.mercury }) : t("profile.builtInTitleNoVersion"),
);
const ex = ref({
  callsign: "", host: "127.0.0.1", controlPort: 8300, dataPort: 8301,
  // Optional, prefilled with Mercury's defaults (-b 8100, -U 10000).
  broadcastPort: 8100 as number | "", telemetryUrl: "ws://127.0.0.1:10000/websocket",
});
// The spectrum address follows the host until the operator edits it.
let telemetryEdited = false;
const markTelemetryEdited = () => { telemetryEdited = true; };
const defaultTelemetry = (host: string) => {
  const h = host.trim() || "127.0.0.1";
  return `ws://${h.includes(":") && !h.startsWith("[") ? `[${h}]` : h}:10000/websocket`;
};
function hostChanged() {
  if (!telemetryEdited) ex.value.telemetryUrl = defaultTelemetry(ex.value.host);
}
const errors = ref<Record<string, string>>({});
const formError = ref("");
const busy = ref(false);
const checks = ref<EndpointCheck[] | null>(null);
const checking = ref(false);

/** A new profile's own fields: the Station step's, else the form's callsign. */
function fresh(callsign: string, modem: StationProfile["modem"]): StationProfile {
  const g = given.value;
  return {
    id: "", callsign: g?.callsign ?? callsign, name: g?.name ?? null, grid: g?.grid ?? null, modem, lab: modem.kind === "simulator",
    receiveOnly: props.receiveOnly ?? false,
  };
}
const wantedCall = (typed: string) => (given.value?.callsign ?? typed).trim().toUpperCase();
/** A saved station reused: in Receive only when asked to start so. */
const reuse = (p: StationProfile, modem: StationProfile["modem"]): StationProfile => ({
  ...p, modem, receiveOnly: props.receiveOnly ? true : p.receiveOnly,
});

async function use(draft: StationProfile, prefix: string) {
  busy.value = true;
  errors.value = {};
  formError.value = "";
  try {
    const saved = await call({ type: "saveProfile", profile: draft });
    if (saved.type !== "profile") throw new Error(t("profile.unexpectedReply", { type: saved.type }));
    await call({ type: "selectProfile", id: saved.id });
    const { type: _, ...profile } = saved;
    emit("done", profile);
  } catch (e) {
    const err = e as AppError;
    if (err && typeof err === "object" && err.code === "validation") {
      for (const f of err.errors) errors.value[`${prefix}.${f.field}`] = f.message;
    } else {
      formError.value = describeError(e);
    }
  } finally {
    busy.value = false;
  }
}

function useSimulator() {
  const wanted = wantedCall(simCall.value);
  const existing = snapshot.value?.profiles.find((p) => p.modem.kind === "simulator" && p.callsign === wanted);
  return use(existing ? reuse(existing, existing.modem) : fresh(simCall.value, { kind: "simulator" }), "sim");
}

async function useBuiltIn() {
  const v = bi.value;
  const ports: BuiltInPorts = { control: Number(v.control), broadcast: Number(v.broadcast), ui: Number(v.ui), rigctld: Number(v.rigctld) };
  const wanted = wantedCall(v.callsign);
  // The same callsign is the same station: its history stays reachable.
  const existing = snapshot.value?.profiles.find((p) => p.modem.kind === "builtIn" && p.callsign === wanted);
  const running = existing && existing.id === snapshot.value?.activeProfileId;
  taken.value = null;
  // Our own running Mercury holds these ports: nothing to check then.
  if (!running) {
    try {
      const r = await call({ type: "checkBuiltInPorts", ports });
      if (r.type === "portCheck" && r.taken !== null && r.suggestion) {
        taken.value = { port: r.taken, suggestion: r.suggestion };
        return;
      }
    } catch (e) {
      formError.value = describeError(e);
      return;
    }
  }
  const modem: StationProfile["modem"] = { kind: "builtIn", ports };
  return use(existing ? reuse(existing, modem) : fresh(v.callsign, modem), "bi");
}

function useFreeSet() {
  const s = taken.value?.suggestion;
  if (!s) return;
  Object.assign(bi.value, s);
  return useBuiltIn();
}

function existingModem(): StationProfile["modem"] {
  const v = ex.value;
  return {
    kind: "existing",
    host: v.host,
    controlPort: Number(v.controlPort),
    dataPort: Number(v.dataPort),
    broadcastPort: v.broadcastPort === "" ? null : Number(v.broadcastPort),
    telemetryUrl: v.telemetryUrl.trim() || null,
  };
}

function useExisting() {
  const v = ex.value;
  // The same Mercury for the same callsign is the same station: reuse it, so
  // its history stays reachable (as useSimulator does); its optional ports
  // follow the form.
  const existing = snapshot.value?.profiles.find(
    (p) =>
      p.modem.kind === "existing" &&
      p.callsign === wantedCall(v.callsign) &&
      p.modem.host.toLowerCase() === v.host.trim().toLowerCase() &&
      p.modem.controlPort === Number(v.controlPort) &&
      p.modem.dataPort === Number(v.dataPort),
  );
  const modem = existingModem();
  if (existing?.modem.kind === "existing" && modem.kind === "existing") {
    return use(reuse(existing, { ...existing.modem, broadcastPort: modem.broadcastPort, telemetryUrl: modem.telemetryUrl }), "ex");
  }
  return use(fresh(v.callsign, modem), "ex");
}

const tick = (ok: boolean) => (ok ? "✓" : "✗");
/** SET-03: each endpoint, read-only, named with what went wrong. */
async function checkExisting() {
  checking.value = true;
  checks.value = null;
  formError.value = "";
  try {
    const r = await call({ type: "checkEndpoints", modem: existingModem() });
    if (r.type === "endpoints") checks.value = r.results;
  } catch (e) {
    const err = e as AppError;
    if (err && typeof err === "object" && err.code === "validation") {
      const form = "ex";
      for (const f of err.errors) errors.value[`${form}.${f.field}`] = f.message;
    } else {
      formError.value = describeError(e);
    }
  } finally {
    checking.value = false;
  }
}
</script>

<template>
  <div class="forms">
    <p v-if="given" class="as-station muted small span-all">{{ t("profile.asStation", { callsign: given.callsign.trim().toUpperCase() }) }}</p>
    <span v-if="given && (errors['bi.callsign'] || errors['ex.callsign'])" class="field-error span-all">{{ errors["bi.callsign"] || errors["ex.callsign"] }}</span>

    <form v-if="builtin?.available" class="card stack builtin" @submit.prevent="useBuiltIn">
      <div class="title-row">
        <h2>{{ builtinTitle }}</h2>
        <span class="badge small">{{ t("profile.recommended") }}</span>
      </div>
      <p class="muted small">{{ t("profile.builtInBody") }}</p>
      <template v-if="!given">
        <label>{{ t("profile.callsign") }} <input v-model="bi.callsign" name="callsign" class="mono" maxlength="15" autocomplete="off" spellcheck="false" /></label>
        <span v-if="errors['bi.callsign']" class="field-error">{{ errors["bi.callsign"] }}</span>
      </template>
      <details class="advanced">
        <summary>{{ t("profile.advanced") }}</summary>
        <div class="ports">
          <label>{{ t("profile.controlPort") }} <input v-model.number="bi.control" name="control" class="mono" type="number" min="1" max="65534" /></label>
          <label>{{ t("profile.broadcastPort") }} <input v-model.number="bi.broadcast" name="broadcast" class="mono" type="number" min="1" max="65535" /></label>
          <label>{{ t("profile.uiPort") }} <input v-model.number="bi.ui" name="ui" class="mono" type="number" min="1" max="65535" /></label>
          <label>{{ t("profile.rigctldPort") }} <input v-model.number="bi.rigctld" name="rigctld" class="mono" type="number" min="1" max="65535" /></label>
        </div>
        <p class="muted small">{{ t("profile.portsHint") }}</p>
      </details>
      <template v-for="f in ['controlPort', 'broadcastPort', 'uiPort', 'rigctldPort', 'modemKind', 'name', 'grid']" :key="f">
        <span v-if="errors['bi.' + f]" class="field-error">{{ errors["bi." + f] }}</span>
      </template>
      <div v-if="taken" class="port-taken" role="alert">
        <p class="warn small">{{ t("profile.portTaken", { port: taken.port }) }}</p>
        <button type="button" class="use-free" :disabled="busy" @click="useFreeSet">
          {{ t("profile.useFreeSet", { control: taken.suggestion.control, broadcast: taken.suggestion.broadcast, ui: taken.suggestion.ui, rigctld: taken.suggestion.rigctld }) }}
        </button>
      </div>
      <div><button type="submit" class="primary" :disabled="busy">{{ t("profile.useBuiltIn") }}</button></div>
    </form>

    <section v-else-if="builtin && !builtin.available" class="card stack builtin-missing" aria-labelledby="builtin-missing-title">
      <h2 id="builtin-missing-title">{{ t("profile.builtInTitleNoVersion") }}</h2>
      <p class="muted small">{{ t("profile.builtInMissing") }}</p>
    </section>

    <form class="card stack existing" @submit.prevent="useExisting">
      <h2>{{ t("profile.existingTitle") }}</h2>
      <p class="muted small">{{ t("profile.existingBody") }}</p>
      <template v-if="!given">
        <label>{{ t("profile.callsign") }} <input v-model="ex.callsign" name="callsign" class="mono" maxlength="15" autocomplete="off" spellcheck="false" /></label>
        <span v-if="errors['ex.callsign']" class="field-error">{{ errors["ex.callsign"] }}</span>
      </template>
      <div class="row">
        <label>{{ t("profile.host") }} <input v-model="ex.host" name="host" class="mono" size="16" autocomplete="off" spellcheck="false" @input="hostChanged" /></label>
        <label>{{ t("profile.controlPort") }} <input v-model.number="ex.controlPort" name="controlPort" class="mono" type="number" min="1" max="65534" /></label>
        <label>{{ t("profile.dataPort") }} <input v-model.number="ex.dataPort" name="dataPort" class="mono" type="number" min="1" max="65535" /></label>
      </div>
      <div class="row">
        <label class="bport">{{ t("profile.broadcastPort") }} <input v-model.number="ex.broadcastPort" name="broadcastPort" class="mono" type="number" min="1" max="65535" /></label>
        <label class="turl">{{ t("profile.telemetryUrl") }} <input v-model="ex.telemetryUrl" name="telemetryUrl" class="mono" autocomplete="off" spellcheck="false" @input="markTelemetryEdited" /></label>
      </div>
      <p class="muted small">{{ t("profile.optionalHint") }}</p>
      <template v-for="f in ['host', 'controlPort', 'dataPort', 'broadcastPort', 'telemetryUrl', 'modemKind', 'name', 'grid']" :key="f">
        <span v-if="errors['ex.' + f]" class="field-error">{{ errors["ex." + f] }}</span>
      </template>
      <ul v-if="checks" class="endpoint-results small" role="status">
        <li v-for="c in checks" :key="c.name" :class="c.ok ? 'ok' : 'err'" :data-name="c.name">
          <span aria-hidden="true">{{ tick(c.ok) }}</span>
          <b>{{ t(`profile.endpoint.${c.name}`) }}</b> <span class="mono">{{ c.address }}</span>: {{ c.ok ? t("profile.endpointOk") : c.detail }}
        </li>
      </ul>
      <div class="buttons">
        <button type="submit" class="primary" :disabled="busy">{{ t("profile.useExisting") }}</button>
        <button type="button" class="check-endpoints" :disabled="checking" :title="t('profile.checkHint')" @click="checkExisting">
          {{ checking ? t("profile.checking") : t("profile.check") }}
        </button>
      </div>
    </form>

    <form v-if="info?.simulator !== false && !wizard" class="card stack simulator" @submit.prevent="useSimulator">
      <h2>{{ t("profile.simulatorTitle") }}</h2>
      <p class="muted small">{{ t("profile.simulatorBody") }}</p>
      <template v-if="!given">
        <label>{{ t("profile.callsign") }} <input v-model="simCall" name="callsign" class="mono" maxlength="15" autocomplete="off" spellcheck="false" /></label>
        <span v-if="errors['sim.callsign']" class="field-error">{{ errors["sim.callsign"] }}</span>
      </template>
      <div><button type="submit" class="primary" :disabled="busy">{{ t("profile.useSimulator") }}</button></div>
    </form>
    <p v-if="formError" class="err span-all" role="alert">{{ formError }}</p>
  </div>
</template>

<style scoped>
/* One column on a narrow window or tablet, side by side where they fit. */
.forms { display: grid; gap: calc(var(--space) * 2); grid-template-columns: repeat(auto-fit, minmax(min(100%, 300px), 1fr)); align-items: start; }
.span-all { grid-column: 1 / -1; margin: 0; }
input { width: 100%; }
.row label { flex: 1; min-width: 7em; }
.row label.turl { flex: 3; min-width: 16em; }
.title-row { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space); }
.title-row h2 { margin: 0; }
.badge { border: 1px solid var(--accent); color: var(--accent); border-radius: 999px; padding: 0 8px; }
.ports { display: grid; gap: var(--space); grid-template-columns: repeat(auto-fit, minmax(min(100%, 9em), 1fr)); margin-top: var(--space); }
summary { cursor: pointer; }
.port-taken { display: grid; gap: 6px; justify-items: start; }
.port-taken p { margin: 0; }
.buttons { display: flex; flex-wrap: wrap; gap: var(--space); }
.endpoint-results { list-style: none; margin: 0; padding: 0; display: grid; gap: 2px; overflow-wrap: anywhere; }
.endpoint-results .ok { color: var(--success); }
.endpoint-results .err { color: var(--danger); }
</style>
