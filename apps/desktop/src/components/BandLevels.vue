<script setup lang="ts">
// TX level and Tune level per band (RIG-04), for the station in use. A
// change is stored at once and applied when the rig is on that band and the
// station is idle.
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import { rigUi, setBandLevels } from "../rig";
import { BANDS } from "./freqStep";

const { t } = useI18n();
const note = ref("");
const levels = (band: string) => rigUi.levels[band] ?? { txGainDb: 0, tuneDbfs: -20 };

async function change(band: string, field: "txGainDb" | "tuneDbfs", e: Event) {
  const v = Number((e.target as HTMLInputElement).value);
  note.value = "";
  if (await setBandLevels(band, { ...levels(band), [field]: v })) note.value = t("bandLevels.saved", { band });
}
</script>

<template>
  <section class="card stack band-levels" aria-labelledby="h-band-levels">
    <h2 id="h-band-levels">{{ t("bandLevels.title") }}</h2>
    <p class="muted small">{{ t("bandLevels.hint") }}</p>
    <table class="small">
      <thead>
        <tr><th>{{ t("bandLevels.band") }}</th><th>{{ t("bandLevels.tx") }}</th><th>{{ t("bandLevels.tune") }}</th></tr>
      </thead>
      <tbody>
        <tr v-for="[, , band] in BANDS" :key="band" :data-band="band">
          <td class="mono">{{ band }}</td>
          <td><input class="band-tx" type="number" min="-20" max="20" step="0.5" :value="levels(band).txGainDb" :aria-label="t('bandLevels.txAria', { band })" @change="change(band, 'txGainDb', $event)" /></td>
          <td><input class="band-tune" type="number" min="-60" max="0" step="1" :value="levels(band).tuneDbfs" :aria-label="t('bandLevels.tuneAria', { band })" @change="change(band, 'tuneDbfs', $event)" /></td>
        </tr>
      </tbody>
    </table>
    <p v-if="note" class="ok small" role="status">{{ note }}</p>
    <p v-if="rigUi.error" class="err" role="alert">{{ rigUi.error }}</p>
  </section>
</template>

<style scoped>
table { border-collapse: collapse; }
th, td { text-align: left; padding: 2px 8px; }
input { width: 6em; }
</style>
