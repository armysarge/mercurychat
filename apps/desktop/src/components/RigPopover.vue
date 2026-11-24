<script setup lang="ts">
// The rig's less-used controls, behind the strip's "Rig" button so the strip
// stays one row (M3b): VFO and split, RF power, this band's TX level and
// Tune level, and the meters while transmitting. Each control shows only if
// the rig has it, and is disabled with the core's reason while it would refuse.
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { currentBand, refusalText, rigRefusal, rigSet, rigUi, rigView, setBandLevels, setTuneLevel, tuneView } from "../rig";
import Popover from "./Popover.vue";

defineProps<{ open: boolean; anchor: HTMLElement | null }>();
const emit = defineEmits<{ close: [] }>();
const { t } = useI18n();

const caps = computed(() => rigView.value?.caps ?? null);
const why = (c: Parameters<typeof rigRefusal>[0]) => refusalText(rigRefusal(c));
const vfos = ["VFOA", "VFOB"];
const power = computed(() => Math.round((rigView.value?.rfPower?.value ?? 0) * 100));
const levels = computed(() => (currentBand.value ? rigUi.levels[currentBand.value] : undefined) ?? { txGainDb: 0, tuneDbfs: -20 });
const tuneLevel = computed(() => (tuneView.value?.active ? tuneView.value.levelDbfs : levels.value.tuneDbfs));
const keyed = computed(() => rigView.value?.ptt === "on");
const meters = computed(() => rigView.value?.meters ?? null);
const txResult = computed(() => {
  const r = rigView.value?.txLevelApplied;
  if (!r) return "";
  if (r.type === "applied") return t("rigPanel.txApplied", { band: r.band, db: r.db.toFixed(1) });
  if (r.type === "pending") return t("rigPanel.txPending", { band: r.band });
  return t("rigPanel.txNotApplied", { band: r.band, reason: r.reason });
});

function setVfo(vfo: string) {
  void rigSet({ type: "vfo", vfo });
}
function setSplit(on: boolean) {
  const tx = rigView.value?.vfo === "VFOB" ? "VFOA" : "VFOB";
  void rigSet({ type: "split", on, txVfo: tx });
}
function setPower(pct: number) {
  void rigSet({ type: "rfPower", level: pct / 100 });
}
function setTx(db: number) {
  if (currentBand.value) void setBandLevels(currentBand.value, { ...levels.value, txGainDb: db });
}
function setTuneDb(db: number) {
  if (tuneView.value?.active) void setTuneLevel(db);
  else if (currentBand.value) void setBandLevels(currentBand.value, { ...levels.value, tuneDbfs: db });
}
const num = (e: Event) => Number((e.target as HTMLInputElement).value);
</script>

<template>
  <Popover :open="open" :anchor="anchor" :label="t('rigPanel.title')" @close="emit('close')">
    <label v-if="caps?.canSetVfo" class="row">
      <span>{{ t("rigPanel.vfo") }}</span>
      <select class="vfo small" :value="rigView?.vfo ?? ''" :disabled="!!why('vfo')" :title="why('vfo')" @change="setVfo(($event.target as HTMLSelectElement).value)">
        <option v-for="v in vfos" :key="v" :value="v">{{ v }}</option>
      </select>
    </label>
    <label v-if="caps?.canSplit" class="row split">
      <span>{{ t("rigPanel.split") }}</span>
      <span class="switch"><input type="checkbox" role="switch" :checked="!!rigView?.split" :disabled="!!why('split')" :title="why('split')" @change="setSplit(($event.target as HTMLInputElement).checked)" /><span class="track" aria-hidden="true"></span></span>
    </label>
    <label v-if="caps?.canSetPower" class="row">
      <span>{{ t("rigPanel.rfPower") }}</span>
      <input class="rf-power" type="range" min="0" max="100" step="1" :value="power" :disabled="!!why('rfPower')" :title="why('rfPower')" @change="setPower(num($event))" />
      <span class="mono value">{{ t("rigPanel.percent", { pct: power }) }}</span>
    </label>
    <label class="row">
      <span>{{ currentBand ? t("rigPanel.txLevel", { band: currentBand }) : t("rigPanel.txLevelNoBand") }}</span>
      <input class="tx-level" type="range" min="-20" max="20" step="0.5" :value="levels.txGainDb" :disabled="!currentBand || !!why('txLevel')" :title="why('txLevel')" @change="setTx(num($event))" />
      <span class="mono value">{{ t("rigPanel.db", { db: levels.txGainDb.toFixed(1) }) }}</span>
    </label>
    <p v-if="txResult" class="muted tx-result">{{ txResult }}</p>
    <label class="row">
      <span>{{ t("rigPanel.tuneLevel") }}</span>
      <input class="tune-level" type="range" min="-60" max="0" step="1" :value="tuneLevel" :disabled="!tuneView?.active && !currentBand" @change="setTuneDb(num($event))" />
      <span class="mono value">{{ t("rigPanel.dbfs", { db: tuneLevel.toFixed(0) }) }}</span>
    </label>
    <div class="meters" role="group" :aria-label="t('rigPanel.meters')">
      <template v-if="keyed && meters">
        <span v-if="meters.swr" class="mono">{{ t("rigPanel.swr", { value: meters.swr.value.toFixed(1) }) }}</span>
        <span v-if="meters.alc" class="mono">{{ t("rigPanel.alc", { value: meters.alc.value.toFixed(2) }) }}</span>
        <span v-if="meters.power" class="mono">{{ t("rigPanel.power", { value: meters.power.value.toFixed(2) }) }}</span>
      </template>
      <span v-else class="muted">{{ t("rigPanel.metersIdle") }}</span>
    </div>
    <p v-if="rigUi.error" class="warn rig-panel-error" role="alert">{{ rigUi.error }}</p>
  </Popover>
</template>

<style scoped>
.row { display: grid; grid-template-columns: 9em 1fr auto; align-items: center; gap: 8px; }
.value { min-width: 6ch; text-align: right; }
.meters { display: flex; gap: 12px; }
.tx-result, .rig-panel-error { margin: 0; }
</style>
