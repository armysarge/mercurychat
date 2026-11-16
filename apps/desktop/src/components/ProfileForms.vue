<script setup lang="ts">
// Make a station profile and use it: the simulator, or an existing Mercury
// at host:port. The core validates every field (profile.rs); its errors are
// shown next to the fields they name.
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import { call, describeError } from "../api";
import type { AppError } from "../bindings/AppError";
import type { StationProfile } from "../bindings/StationProfile";
import { snapshot } from "../store";

const { t } = useI18n();
const emit = defineEmits<{ done: [] }>();

const simCall = ref("");
const ex = ref({ callsign: "", host: "127.0.0.1", controlPort: 8300, dataPort: 8301 });
const errors = ref<Record<string, string>>({});
const formError = ref("");
const busy = ref(false);

async function use(draft: StationProfile, prefix: string) {
  busy.value = true;
  errors.value = {};
  formError.value = "";
  try {
    const saved = await call({ type: "saveProfile", profile: draft });
    if (saved.type !== "profile") throw new Error(t("profile.unexpectedReply", { type: saved.type }));
    await call({ type: "selectProfile", id: saved.id });
    emit("done");
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
  const wanted = simCall.value.trim().toUpperCase();
  const existing = snapshot.value?.profiles.find((p) => p.modem.kind === "simulator" && p.callsign === wanted);
  const draft: StationProfile = existing ?? {
    id: "", callsign: simCall.value, name: null, grid: null, modem: { kind: "simulator" }, lab: true, receiveOnly: false,
  };
  return use(draft, "sim");
}

function useExisting() {
  const v = ex.value;
  // The same Mercury for the same callsign is the same station: reuse it, so
  // its history stays reachable (as useSimulator does).
  const existing = snapshot.value?.profiles.find(
    (p) =>
      p.modem.kind === "existing" &&
      p.callsign === v.callsign.trim().toUpperCase() &&
      p.modem.host.toLowerCase() === v.host.trim().toLowerCase() &&
      p.modem.controlPort === Number(v.controlPort) &&
      p.modem.dataPort === Number(v.dataPort),
  );
  const draft: StationProfile = existing ?? {
    id: "",
    callsign: v.callsign,
    name: null,
    grid: null,
    modem: {
      kind: "existing",
      host: v.host,
      controlPort: Number(v.controlPort),
      dataPort: Number(v.dataPort),
      broadcastPort: null,
      telemetryUrl: null,
    },
    lab: false,
    receiveOnly: false,
  };
  return use(draft, "ex");
}
</script>

<template>
  <div class="forms">
    <form class="card stack simulator" @submit.prevent="useSimulator">
      <h2>{{ t("profile.simulatorTitle") }}</h2>
      <p class="muted small">{{ t("profile.simulatorBody") }}</p>
      <label>{{ t("profile.callsign") }} <input v-model="simCall" name="callsign" class="mono" maxlength="15" autocomplete="off" spellcheck="false" /></label>
      <span v-if="errors['sim.callsign']" class="field-error">{{ errors["sim.callsign"] }}</span>
      <div><button type="submit" class="primary" :disabled="busy">{{ t("profile.useSimulator") }}</button></div>
    </form>

    <form class="card stack existing" @submit.prevent="useExisting">
      <h2>{{ t("profile.existingTitle") }}</h2>
      <p class="muted small">{{ t("profile.existingBody") }}</p>
      <label>{{ t("profile.callsign") }} <input v-model="ex.callsign" name="callsign" class="mono" maxlength="15" autocomplete="off" spellcheck="false" /></label>
      <span v-if="errors['ex.callsign']" class="field-error">{{ errors["ex.callsign"] }}</span>
      <div class="row">
        <label>{{ t("profile.host") }} <input v-model="ex.host" name="host" class="mono" size="16" autocomplete="off" spellcheck="false" /></label>
        <label>{{ t("profile.controlPort") }} <input v-model.number="ex.controlPort" name="controlPort" class="mono" type="number" min="1" max="65534" /></label>
        <label>{{ t("profile.dataPort") }} <input v-model.number="ex.dataPort" name="dataPort" class="mono" type="number" min="1" max="65535" /></label>
      </div>
      <span v-for="f in ['host', 'controlPort', 'dataPort', 'modemKind']" :key="f">
        <span v-if="errors['ex.' + f]" class="field-error">{{ errors["ex." + f] }}</span>
      </span>
      <div><button type="submit" class="primary" :disabled="busy">{{ t("profile.useExisting") }}</button></div>
    </form>
    <p v-if="formError" class="err" role="alert">{{ formError }}</p>
  </div>
</template>

<style scoped>
.forms { display: grid; gap: calc(var(--space) * 2); grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); align-items: start; }
input { width: 100%; }
.row label { flex: 1; min-width: 7em; }
</style>
