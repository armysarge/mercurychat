<script setup lang="ts">
// The Logbook (LOG-01; docs/04 §2 rail): every QSO record of this station,
// drafts and finalised ones, in a table that sorts by any column. Choosing a
// record opens the same form as the QSO card; a finalised record is
// read-only. Export (M5 §7): ADIF for a logger such as Log4OM, or CSV, all
// records or the finalised ones, saved where the operator chooses.
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { describeError } from "../api";
import type { ExportFormat } from "../bindings/ExportFormat";
import type { Qso } from "../bindings/Qso";
import { exportLogbook } from "../exports";
import { adifModeOk, withoutBand } from "../logExport";
import QsoForm from "../components/QsoForm.vue";
import { activeProfile, logbook, reloadLogbook, setAdifMode, snapshot } from "../store";

const { t } = useI18n();
onMounted(() => void reloadLogbook());

const COLUMNS = ["start", "peer", "band", "freq", "bandwidth", "rstSent", "rstRcvd", "state"] as const;
type Column = (typeof COLUMNS)[number];
const sortBy = ref<Column>("start");
const ascending = ref(false);
const selected = ref<string | null>(null);

function value(q: Qso, c: Column): string | number {
  switch (c) {
    case "start":
      return q.startUtc;
    case "freq":
      return q.freq?.value ?? -1;
    case "bandwidth":
      return q.bandwidth ?? "";
    default:
      return (c === "peer" ? q.peer : c === "band" ? q.band : c === "state" ? q.state : q[c]) ?? "";
  }
}
const rows = computed(() =>
  [...logbook.qsos].sort((a, b) => {
    const x = value(a, sortBy.value), y = value(b, sortBy.value);
    const order = typeof x === "number" && typeof y === "number" ? x - y : String(x).localeCompare(String(y));
    return ascending.value ? order : -order;
  }),
);
const chosen = computed(() => logbook.qsos.find((q) => q.id === selected.value) ?? null);

function sort(c: Column) {
  if (sortBy.value === c) ascending.value = !ascending.value;
  else {
    sortBy.value = c;
    ascending.value = c !== "start";
  }
}
const ariaSort = (c: Column) => (sortBy.value !== c ? "none" : ascending.value ? "ascending" : "descending");
const when = (utc: string) => utc.slice(0, 10) + " " + utc.slice(11, 16);
const mhz = (q: Qso) => (q.freq ? (q.freq.value / 1e6).toFixed(3) : "");

const FORMATS: ExportFormat[] = ["adif", "csv"];
const format = ref<ExportFormat>("adif");
const finalisedOnly = ref(false);
const exporting = ref(false);
const exportNote = ref("");
const exportError = ref("");
/** QSOs with neither band nor frequency: loggers refuse them, so say so before an ADIF export. */
const missingBand = computed(() => (format.value === "adif" ? withoutBand(logbook.qsos, finalisedOnly.value) : 0));
/** The MODE the ADIF export writes; saved when the field is left with a valid value. */
const adifMode = ref(snapshot.value?.adifMode ?? "DYNAMIC");
const modeError = ref("");
async function saveAdifMode() {
  modeError.value = "";
  if (!adifModeOk(adifMode.value)) {
    modeError.value = t("logbook.export.modeInvalid");
    return;
  }
  try {
    await setAdifMode(adifMode.value);
    adifMode.value = adifMode.value.trim().toUpperCase();
  } catch (e) {
    modeError.value = describeError(e);
  }
}
async function exportNow() {
  exporting.value = true;
  exportNote.value = "";
  exportError.value = "";
  try {
    const n = await exportLogbook(format.value, finalisedOnly.value, activeProfile.value?.callsign ?? "logbook");
    if (n !== null) exportNote.value = t("logbook.export.done", { n }, n);
  } catch (e) {
    exportError.value = describeError(e);
  } finally {
    exporting.value = false;
  }
}
</script>

<template>
  <div class="logbook">
    <h1>{{ t("logbook.title") }}</h1>
    <p class="muted small">{{ t("logbook.intro") }}</p>
    <p v-if="logbook.error" class="err" role="alert">{{ logbook.error }}</p>
    <div class="export-bar" role="group" :aria-label="t('logbook.export.aria')">
      <label>
        {{ t("logbook.export.format") }}
        <select v-model="format" class="export-format">
          <option v-for="f in FORMATS" :key="f" :value="f">{{ t(`logbook.export.formatOpt.${f}`) }}</option>
        </select>
      </label>
      <label v-if="format === 'adif'" :title="t('logbook.export.modeHint')">
        {{ t("logbook.export.mode") }}
        <input v-model="adifMode" class="adif-mode mono" maxlength="20" spellcheck="false" autocomplete="off" :aria-invalid="!!modeError" @change="saveAdifMode" />
      </label>
      <label class="check"><span class="switch"><input v-model="finalisedOnly" type="checkbox" role="switch" class="export-final" /><span class="track" aria-hidden="true"></span></span> {{ t("logbook.export.finalisedOnly") }}</label>
      <button class="export-logbook" :disabled="exporting || !logbook.qsos.length" @click="exportNow">{{ t("logbook.export.button") }}</button>
      <span v-if="exportNote" class="ok small" role="status">{{ exportNote }}</span>
      <span v-if="exportError" class="err small" role="alert">{{ exportError }}</span>
      <span v-if="modeError" class="err small mode-error" role="alert">{{ modeError }}</span>
    </div>
    <p v-if="missingBand" class="warn small export-missing-band" role="status">{{ t("logbook.export.missingBand", { n: missingBand }, missingBand) }}</p>
    <div v-if="rows.length" class="layout">
      <div class="table-wrap">
        <table class="qsos">
          <thead>
            <tr>
              <th v-for="c in COLUMNS" :key="c" :aria-sort="ariaSort(c)" scope="col">
                <button class="quiet" :data-sort="c" @click="sort(c)">
                  {{ t(`logbook.col.${c}`) }}<span v-if="sortBy === c" aria-hidden="true">{{ ascending ? " ▴" : " ▾" }}</span>
                </button>
              </th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="q in rows"
              :key="q.id"
              :data-id="q.id"
              :data-state="q.state"
              :class="{ chosen: q.id === selected }"
              @click="selected = q.id"
            >
              <td class="mono start">{{ when(q.startUtc) }}</td>
              <td class="mono peer"><button class="link quiet" :aria-label="t('logbook.open', { peer: q.peer })" @click.stop="selected = q.id">{{ q.peer }}</button></td>
              <td class="mono band">{{ q.band ?? "" }}</td>
              <td class="mono freq" :title="q.freq ? q.freq.source : t('qso.freqNone')">{{ mhz(q) }}</td>
              <td class="mono bandwidth">{{ q.bandwidth ?? "" }}</td>
              <td class="mono rst-sent">{{ q.rstSent ?? "" }}</td>
              <td class="mono rst-rcvd">{{ q.rstRcvd ?? "" }}</td>
              <td class="state">{{ t(q.state === "final" ? "qso.state.final" : "qso.state.draft") }}</td>
            </tr>
          </tbody>
        </table>
      </div>
      <section v-if="chosen" class="card stack editor" :aria-label="t('logbook.editAria', { peer: chosen.peer })">
        <span class="label">{{ t("logbook.editTitle", { peer: chosen.peer, start: when(chosen.startUtc) }) }}</span>
        <QsoForm :qso="chosen" />
      </section>
    </div>
    <p v-else class="muted">{{ t("logbook.empty") }}</p>
  </div>
</template>

<style scoped>
.logbook { padding: calc(var(--space) * 3); display: grid; gap: calc(var(--space) * 1.5); align-content: start; overflow-y: auto; height: 100%; }
h1 { font-size: calc(20px * var(--font-scale)); }
.layout { display: grid; grid-template-columns: minmax(0, 1fr) minmax(260px, 340px); gap: calc(var(--space) * 2); align-items: start; }
.table-wrap { overflow-x: auto; }
table { border-collapse: collapse; width: 100%; font-size: calc(13px * var(--font-scale)); }
th { text-align: left; border-bottom: 1px solid var(--border-strong); }
th button { min-height: 0; padding: 4px 6px; border-color: transparent; font-weight: 600; white-space: nowrap; }
td { padding: 5px 6px; border-bottom: 1px solid var(--border); white-space: nowrap; }
tbody tr { cursor: pointer; }
tbody tr:hover, tbody tr.chosen { background: var(--raised); }
tr[data-state="final"] .state { color: var(--text-2); }
.export-bar { display: flex; flex-wrap: wrap; gap: var(--space); align-items: center; }
.export-bar label { display: inline-flex; gap: 6px; align-items: center; }
.link { min-height: 0; padding: 0; border: 0; background: transparent; font: inherit; color: var(--accent); }
@media (max-width: 1099px) {
  .layout { grid-template-columns: minmax(0, 1fr); }
}
</style>
