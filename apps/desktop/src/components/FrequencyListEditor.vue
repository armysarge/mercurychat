<script setup lang="ts">
// The frequency list (D15), shared by every station: add, edit, delete,
// reorder and restore the region's suggestions. Suggested entries are a
// starting point, labelled so, and editable like any other.
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { call, describeError } from "../api";
import type { FrequencyEntry } from "../bindings/FrequencyEntry";
import { loadFrequencies, rigUi, rigView } from "../rig";
import { bandFor, parseFreqInput } from "./freqStep";

const { t } = useI18n();
const PURPOSES = ["beacon", "cq", "chat"] as const;
const DEFAULT_MODES = ["USB", "LSB", "PKTUSB", "PKTLSB", "CW", "AM", "FM", "RTTY"];
const modes = computed(() => (rigView.value?.caps?.modes.length ? rigView.value.caps.modes : DEFAULT_MODES));

const blank = (): FrequencyEntry => ({
  id: "", name: "", freqHz: 0, mode: "PKTUSB", passbandHz: null, purpose: [], notes: "",
  suggested: false, presetKey: null, region: null, sort: 0, band: null, outOfBand: false,
});
const form = ref<FrequencyEntry>(blank());
const freqText = ref("");
const error = ref("");
const note = ref("");
const formBand = computed(() => bandFor(parseFreqInput(freqText.value)));

function edit(e: FrequencyEntry) {
  form.value = { ...e, purpose: [...e.purpose] };
  freqText.value = (e.freqHz / 1e6).toFixed(6).replace(/0+$/, "").replace(/\.$/, "");
  error.value = "";
}
function cancel() {
  form.value = blank();
  freqText.value = "";
  error.value = "";
}
function togglePurpose(p: string, on: boolean) {
  const set = new Set(form.value.purpose);
  if (on) set.add(p);
  else set.delete(p);
  form.value.purpose = PURPOSES.filter((x) => set.has(x));
}
async function save() {
  error.value = "";
  note.value = "";
  const hz = parseFreqInput(freqText.value);
  if (hz === null) {
    error.value = t("freqList.freqInvalid");
    return;
  }
  try {
    const r = await call({ type: "saveFrequency", entry: { ...form.value, freqHz: hz } });
    if (r.type === "frequencies") rigUi.entries = r.entries;
    cancel();
  } catch (e) {
    error.value = describeError(e);
  }
}
async function remove(e: FrequencyEntry) {
  error.value = "";
  try {
    await call({ type: "deleteFrequency", id: e.id });
    await loadFrequencies();
  } catch (err) {
    error.value = describeError(err);
  }
}
/** Move an entry up or down: the list is stored in its new order. */
async function move(i: number, by: -1 | 1) {
  const list = [...rigUi.entries];
  const j = i + by;
  if (j < 0 || j >= list.length) return;
  [list[i], list[j]] = [list[j], list[i]];
  error.value = "";
  try {
    for (const [k, e] of list.entries()) {
      if (e.sort !== k) await call({ type: "saveFrequency", entry: { ...e, sort: k } });
    }
    await loadFrequencies();
  } catch (err) {
    error.value = describeError(err);
  }
}
async function restore() {
  error.value = "";
  try {
    const r = await call({ type: "restoreSuggested" });
    if (r.type === "restored") note.value = t("freqList.restored", { n: r.added });
    await loadFrequencies();
  } catch (e) {
    error.value = describeError(e);
  }
}
const UP = "↑";
const DOWN = "↓";
const purposes = (e: FrequencyEntry) => e.purpose.map((p) => t(`freqList.purposeOpt.${p}`)).join(", ");
const mhz = (hz: number) => t("freqList.mhz", { mhz: (hz / 1e6).toFixed(4) });
</script>

<template>
  <section class="card stack freq-list-editor" aria-labelledby="h-freq-list">
    <h2 id="h-freq-list">{{ t("freqList.title") }}</h2>
    <p class="muted small">{{ t("freqList.hint") }}</p>
    <div class="table-wrap">
      <table v-if="rigUi.entries.length" class="small">
        <thead>
          <tr>
            <th>{{ t("freqList.name") }}</th><th>{{ t("freqList.freq") }}</th><th>{{ t("freqList.mode") }}</th>
            <th>{{ t("freqList.band") }}</th><th>{{ t("freqList.purpose") }}</th><th>{{ t("freqList.notes") }}</th>
            <th><span class="sr-only">{{ t("freqList.actions") }}</span></th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="(e, i) in rigUi.entries" :key="e.id" :data-id="e.id" :class="{ suggested: e.suggested }">
            <td>{{ e.name }} <span v-if="e.suggested" class="badge">{{ t("freqList.suggested") }}</span></td>
            <td class="mono">{{ mhz(e.freqHz) }}</td>
            <td class="mono">{{ e.mode }}</td>
            <td class="mono">{{ e.band ?? t("freqList.outOfBand") }}</td>
            <td>{{ purposes(e) }}</td>
            <td class="notes">{{ e.notes }}</td>
            <td class="actions">
              <button class="quiet small" :disabled="i === 0" :title="t('freqList.up')" :aria-label="t('freqList.up')" @click="move(i, -1)">{{ UP }}</button>
              <button class="quiet small" :disabled="i === rigUi.entries.length - 1" :title="t('freqList.down')" :aria-label="t('freqList.down')" @click="move(i, 1)">{{ DOWN }}</button>
              <button class="quiet small edit" @click="edit(e)">{{ t("freqList.edit") }}</button>
              <button class="quiet small delete" @click="remove(e)">{{ t("freqList.delete") }}</button>
            </td>
          </tr>
        </tbody>
      </table>
      <p v-else class="muted">{{ t("freqList.empty") }}</p>
    </div>
    <form class="entry-form" @submit.prevent="save">
      <label>{{ t("freqList.name") }} <input v-model="form.name" class="entry-name" maxlength="64" /></label>
      <label>{{ t("freqList.freq") }} <input v-model="freqText" class="entry-freq mono" inputmode="decimal" /></label>
      <label>{{ t("freqList.mode") }}
        <select v-model="form.mode" class="entry-mode">
          <option v-for="m in modes" :key="m" :value="m">{{ m }}</option>
        </select>
      </label>
      <fieldset class="purposes">
        <legend>{{ t("freqList.purpose") }}</legend>
        <label v-for="p in PURPOSES" :key="p"><input type="checkbox" :checked="form.purpose.includes(p)" @change="togglePurpose(p, ($event.target as HTMLInputElement).checked)" /> {{ t(`freqList.purposeOpt.${p}`) }}</label>
      </fieldset>
      <label class="wide">{{ t("freqList.notes") }} <input v-model="form.notes" class="entry-notes" maxlength="500" /></label>
      <label v-if="freqText && !formBand"><input v-model="form.outOfBand" type="checkbox" class="entry-oob" /> {{ t("freqList.outOfBand") }}</label>
      <div class="buttons">
        <button type="submit" class="entry-save">{{ form.id ? t("freqList.update") : t("freqList.add") }}</button>
        <button v-if="form.id" type="button" class="quiet" @click="cancel">{{ t("freqList.cancel") }}</button>
        <button type="button" class="restore-suggested" @click="restore">{{ t("freqList.restore") }}</button>
        <span v-if="note" class="ok small restored" role="status">{{ note }}</span>
      </div>
    </form>
    <p v-if="error" class="err" role="alert">{{ error }}</p>
  </section>
</template>

<style scoped>
/* relative: the header's sr-only label stays inside the scroller, or it
   widens the page past a tablet's width (M5 responsive check). */
.table-wrap { overflow-x: auto; position: relative; }
table { width: 100%; border-collapse: collapse; }
th, td { text-align: left; padding: 3px 6px; border-bottom: 1px solid var(--border); white-space: nowrap; }
td.notes { white-space: normal; max-width: 16em; }
.actions { display: flex; gap: 2px; }
.badge { font-size: 0.85em; padding: 0 5px; border-radius: 999px; border: 1px solid var(--border-strong); color: var(--text-2); }
.entry-form { display: flex; flex-wrap: wrap; gap: var(--space); align-items: end; }
.entry-form .wide { flex: 1 1 14em; }
.entry-freq { width: 9em; }
.purposes { border: 0; padding: 0; margin: 0; display: flex; gap: 8px; }
.buttons { display: flex; gap: var(--space); align-items: center; flex-basis: 100%; }
</style>
