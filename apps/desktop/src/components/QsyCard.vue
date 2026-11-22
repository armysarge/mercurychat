<script setup lang="ts">
// The QSY card (M3c B3). Incoming: the candidates and the answer window's
// countdown, with Accept and Decline. Ours: waiting for the answer. Agreed:
// the move, then the window on the new frequency, with the manual path
// ("Tune to … now" and a Tuned button that only acknowledges) when this
// station's rig can't be set. The core runs every step; this only shows it.
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { answerQsy, describeTuning, extras, qsy, qsyLeft, qsyTuned } from "../extras";
import { session } from "../store";

const { t } = useI18n();
const now = ref(Date.now());
let clock: ReturnType<typeof setInterval> | undefined;
onMounted(() => (clock = setInterval(() => (now.value = Date.now()), 1000)));
onUnmounted(() => clearInterval(clock));

const pick = ref(0);
const left = computed(() => qsyLeft(now.value));
const peer = computed(() => session.value?.peer ?? extras.notice?.peer ?? "");
const STAGES = ["proposed", "offered", "accepted", "agreed", "moving", "fallingBack"] as const;
const stage = computed(() => {
  const s = qsy.value?.stage ?? "";
  return (STAGES as readonly string[]).includes(s) ? (s as (typeof STAGES)[number]) : null;
});
const chosen = computed(() => {
  const q = qsy.value;
  return q && q.chosen !== null ? q.candidates[q.chosen] ?? null : null;
});
const noticeKeys = ["declined", "declinedRig", "ownRig", "noAnswer", "timedOut", "fellBack"] as const;
const notice = computed(() => {
  const n = extras.notice;
  if (!n) return "";
  const key = n.key.replace(/^qsy\.notify\./, "");
  if (!(noticeKeys as readonly string[]).includes(key)) return "";
  return t(`qsy.notify.${key}`, { peer: n.peer, original: n.original ?? "" });
});
</script>

<template>
  <section v-if="qsy && stage" class="card stack qsy-card" :data-stage="stage" aria-labelledby="h-qsy" aria-live="polite">
    <div class="head">
      <span id="h-qsy" class="label">{{ t("qsy.title") }}</span>
      <span v-if="left !== null" class="mono qsy-countdown" :title="t('qsy.countdownTitle')">{{ t("qsy.seconds", { n: left }) }}</span>
    </div>
    <p class="qsy-stage small">{{ t(`qsy.stage.${stage}`, { peer }) }}</p>

    <template v-if="stage === 'offered'">
      <fieldset class="candidates">
        <legend class="sr-only">{{ t("qsy.candidates") }}</legend>
        <label v-for="(c, i) in qsy.candidates" :key="i">
          <input v-model="pick" type="radio" name="qsy-candidate" :value="i" />
          <span class="mono">{{ describeTuning(c) }}</span>
          <span v-if="c.name" class="muted small">{{ c.name }}</span>
        </label>
      </fieldset>
      <div class="row">
        <button class="primary qsy-accept" @click="answerQsy(pick)">{{ t("qsy.accept") }}</button>
        <button class="qsy-decline" @click="answerQsy(null)">{{ t("qsy.decline") }}</button>
      </div>
    </template>

    <ul v-else-if="stage === 'proposed'" class="plain small">
      <li v-for="(c, i) in qsy.candidates" :key="i" class="mono">{{ describeTuning(c) }} <span class="muted">{{ c.name }}</span></li>
    </ul>

    <p v-else-if="chosen" class="mono qsy-target">{{ describeTuning(chosen) }}</p>

    <div v-if="qsy.manual" class="qsy-manual stack" role="alert">
      <p>{{ t("qsy.tuneNow", { freq: describeTuning(qsy.manual) }) }}</p>
      <div><button class="qsy-tuned" @click="qsyTuned()">{{ t("qsy.tuned") }}</button></div>
    </div>
    <p v-if="extras.error" class="err small" role="alert">{{ extras.error }}</p>
  </section>
  <p v-else-if="notice" class="card small qsy-notice" role="status">{{ notice }}</p>
</template>

<style scoped>
.head { display: flex; justify-content: space-between; align-items: baseline; }
.candidates { border: 0; margin: 0; padding: 0; display: grid; gap: 4px; }
.candidates label { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.plain { list-style: none; margin: 0; padding: 0; display: grid; gap: 2px; }
.qsy-manual { border: 1px solid var(--warning); border-radius: 8px; padding: 6px 8px; }
.qsy-notice { margin: 0; }
</style>
