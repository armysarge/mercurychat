<script setup lang="ts">
// One file transfer (M4), in the conversation and in the Files view. An
// incoming offer: who, how big, its airtime, the time left to answer, and
// Accept / Decline / More time (once). Under way: progress, the time left
// only once it is measured, and Cancel. Ended: the state and why. A received
// file is never opened from here: only "Show in folder" (FIL-04). Text only.
import { computed, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { TransferView } from "../bindings/TransferView";
import { fileAct, session } from "../store";
import { airtime, formatSize, isFinal, offerAirtimeS, reasonKey } from "../transfers";

const props = defineProps<{ v: TransferView }>();
const { t, te } = useI18n();

const incoming = computed(() => props.v.direction === "in");
const offer = computed(() => incoming.value && props.v.state === "offered");
const moving = computed(() => !isFinal(props.v.state) && !offer.value);
const extended = ref(false);

// The answer countdown runs locally from the core's "seconds left".
const deadline = ref(0);
const now = ref(Date.now());
let clock: ReturnType<typeof setInterval> | undefined;
watch(
  () => props.v.offerExpiresS,
  (s) => {
    deadline.value = s == null ? 0 : Date.now() + s * 1000;
    clearInterval(clock);
    if (s != null && offer.value) clock = setInterval(() => (now.value = Date.now()), 1000);
  },
  { immediate: true },
);
onUnmounted(() => clearInterval(clock));
const left = computed(() => (deadline.value ? Math.max(0, Math.round((deadline.value - now.value) / 1000)) : null));

const words = (a: { key: string; n: number }) => t(a.key, { n: a.n });
const offerAir = computed(() => words(airtime(offerAirtimeS(props.v.wireSize, session.value?.bandwidth ?? ""))));
const pct = computed(() => (props.v.wireSize ? Math.min(100, Math.round((props.v.bytesDone / props.v.wireSize) * 100)) : 0));
const reason = computed(() => {
  const r = props.v.reason;
  if (!r) return "";
  const k = reasonKey(r);
  return k && te(k) ? t(k) : r;
});
const renamed = computed(() => !!props.v.savedName && props.v.savedName !== props.v.originalName);

async function extend() {
  if (await fileAct({ type: "extendFile", id: props.v.id })) extended.value = true;
}
</script>

<template>
  <section :class="['transfer', 'card', 'stack', v.direction]" :data-transfer="v.id" :data-state="v.state">
    <div class="head">
      <span class="name mono">{{ v.originalName }}</span>
      <span :class="['state', v.state]">{{ t(`files.state.${v.state}`) }}</span>
    </div>
    <div class="small muted">
      {{ incoming ? t("files.card.from", { peer: v.peer }) : t("files.card.to", { peer: v.peer }) }} ·
      {{ t("files.card.sizes", { size: formatSize(Number(v.size)), wire: formatSize(Number(v.wireSize)) }) }}
    </div>
    <template v-if="offer">
      <div class="small">{{ t("files.card.airtime", { time: offerAir }) }}</div>
      <div v-if="left !== null" class="small countdown">{{ t("files.card.answerWithin", { n: left }) }}</div>
      <div class="actions">
        <button class="accept" @click="fileAct({ type: 'acceptFile', id: v.id })">{{ t("files.card.accept") }}</button>
        <button class="quiet decline" @click="fileAct({ type: 'declineFile', id: v.id })">{{ t("files.card.decline") }}</button>
        <button v-if="!extended" class="quiet extend" @click="extend">{{ t("files.card.extend") }}</button>
      </div>
    </template>
    <template v-else-if="moving">
      <progress :value="pct" max="100" :aria-label="t('files.card.progress')"></progress>
      <div class="small muted">
        {{ t("files.card.done", { done: formatSize(Number(v.bytesDone)), wire: formatSize(Number(v.wireSize)) }) }}
        <span v-if="v.etaS != null"> · {{ t("files.card.left", { time: words(airtime(v.etaS)) }) }}</span>
      </div>
      <div class="actions">
        <button class="quiet cancel" @click="fileAct({ type: 'cancelFile', id: v.id })">{{ t("files.card.cancel") }}</button>
      </div>
    </template>
    <template v-else>
      <div v-if="reason" class="small reason">{{ reason }}</div>
      <template v-if="v.state === 'complete' && incoming">
        <div class="small ok">{{ t("files.card.verified") }}</div>
        <div v-if="renamed" class="small">{{ t("files.card.savedAs", { name: v.savedName }) }}</div>
        <div class="actions">
          <button class="quiet show" @click="fileAct({ type: 'showInFolder', id: v.id })">{{ t("files.card.showInFolder") }}</button>
        </div>
      </template>
    </template>
  </section>
</template>

<style scoped>
.transfer { max-width: 46ch; }
.transfer.out { margin-left: auto; }
.head { display: flex; justify-content: space-between; gap: 8px; align-items: baseline; }
.name { overflow-wrap: anywhere; font-weight: 600; }
.state { font-size: calc(12px * var(--font-scale)); color: var(--text-2); }
.state.complete { color: var(--success); }
.state.failed, .state.interrupted { color: var(--danger); }
.actions { display: flex; flex-wrap: wrap; gap: 8px; }
progress { width: 100%; }
</style>
