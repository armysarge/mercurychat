<script setup lang="ts">
// One QSO record's edit form (LOG-01), shared by the QSO card and the
// Logbook: RST sent and received, the peer's name and QTH, my power, notes
// and the dial frequency. A typed frequency is stored with source
// `operator`; the frequency shows where it came from and how old it was.
// A finalised record is read-only.
import { reactive, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { describeError } from "../api";
import type { Qso } from "../bindings/Qso";
import type { QsoEdit } from "../bindings/QsoEdit";
import { finaliseQso, updateQso } from "../store";

const { t } = useI18n();
const props = defineProps<{ qso: Qso }>();

const TEXT = ["rstSent", "rstRcvd", "name", "qth", "myPower", "notes"] as const;
type Field = (typeof TEXT)[number];
const mhz = (hz: number | undefined | null) => (hz === undefined || hz === null ? "" : (hz / 1e6).toFixed(6).replace(/0{1,3}$/, ""));

const form = reactive({ rstSent: "", rstRcvd: "", name: "", qth: "", myPower: "", notes: "", freqMhz: "" });
const status = ref("");
const error = ref("");
const busy = ref(false);
const dirty = ref(false);

/** Show the record's values; an edit in progress is kept until it is saved. */
function fill(q: Qso, force: boolean) {
  if (dirty.value && !force) return;
  for (const k of TEXT) form[k] = q[k] ?? "";
  form.freqMhz = mhz(q.freq?.value);
  dirty.value = false;
}
watch(() => props.qso.id, () => {
  status.value = "";
  error.value = "";
  fill(props.qso, true);
});
watch(() => props.qso, (q) => fill(q, false), { immediate: true, deep: true });

const final = () => props.qso.state === "final";
const provenance = (q: Qso) => {
  const f = q.freq;
  if (!f) return t("qso.freqNone");
  const at = f.atUtc.slice(11, 19) + " UTC";
  return f.ageMs === null ? t("heard.from", { source: f.source, at }) : t("heard.fromAged", { source: f.source, at, age: (f.ageMs / 1000).toFixed(1) });
};

async function save() {
  error.value = "";
  status.value = "";
  const edit: QsoEdit = { grid: null, band: null, freqHz: null, rstSent: null, rstRcvd: null, name: null, qth: null, myPower: null, notes: null };
  for (const k of TEXT as readonly Field[]) {
    const v = form[k].trim();
    if (v && v !== (props.qso[k] ?? "")) edit[k] = v;
  }
  const typed = form.freqMhz.trim();
  if (typed && typed !== mhz(props.qso.freq?.value)) {
    const hz = Math.round(Number(typed.replace(",", ".")) * 1e6);
    if (!Number.isFinite(hz) || hz <= 0) {
      error.value = t("qso.freqInvalid");
      return;
    }
    edit.freqHz = hz;
  }
  busy.value = true;
  try {
    const saved = await updateQso(props.qso.id, edit);
    dirty.value = false;
    if (saved) fill(saved, true);
    status.value = t("qso.saved");
  } catch (e) {
    error.value = describeError(e);
  } finally {
    busy.value = false;
  }
}

async function finalise() {
  error.value = "";
  busy.value = true;
  try {
    if (dirty.value) await save();
    if (!error.value) await finaliseQso(props.qso.id);
    status.value = "";
  } catch (e) {
    error.value = describeError(e);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <form class="qso-form stack" @submit.prevent="save" @input="dirty = true; status = ''">
    <div class="pair">
      <label>{{ t("qso.rstSent") }} <input v-model="form.rstSent" name="rstSent" class="mono" maxlength="8" autocomplete="off" :disabled="final()" /></label>
      <label>{{ t("qso.rstRcvd") }} <input v-model="form.rstRcvd" name="rstRcvd" class="mono" maxlength="8" autocomplete="off" :disabled="final()" /></label>
    </div>
    <div class="pair">
      <label>{{ t("qso.name") }} <input v-model="form.name" name="name" maxlength="64" autocomplete="off" :disabled="final()" /></label>
      <label>{{ t("qso.qth") }} <input v-model="form.qth" name="qth" maxlength="64" autocomplete="off" :disabled="final()" /></label>
    </div>
    <div class="pair">
      <label>{{ t("qso.myPower") }} <input v-model="form.myPower" name="myPower" class="mono" maxlength="16" autocomplete="off" :disabled="final()" /></label>
      <label>
        {{ t("qso.freqMhz") }}
        <input v-model="form.freqMhz" name="freqMhz" class="mono" inputmode="decimal" maxlength="12" autocomplete="off" :disabled="final()" :title="provenance(qso)" />
      </label>
    </div>
    <p class="muted small freq-source">{{ provenance(qso) }}</p>
    <label>{{ t("qso.notes") }} <textarea v-model="form.notes" name="notes" rows="2" maxlength="1000" :disabled="final()"></textarea></label>
    <p v-if="error" class="err small" role="alert">{{ error }}</p>
    <div v-if="!final()" class="row">
      <button type="submit" class="qso-save" :disabled="busy">{{ t("qso.save") }}</button>
      <button type="button" class="qso-finalise" :disabled="busy" @click="finalise">{{ t("qso.finalise") }}</button>
      <span class="saved small ok" role="status">{{ status }}</span>
    </div>
    <p v-else class="muted small">{{ t("qso.finalNote") }}</p>
  </form>
</template>

<style scoped>
.pair { display: grid; grid-template-columns: 1fr 1fr; gap: var(--space); }
input, textarea { width: 100%; min-width: 0; }
textarea { resize: vertical; }
.row { flex-wrap: wrap; align-items: center; }
</style>
