<script setup lang="ts">
// The QSO card (LOG-01, docs/04 §8.1): the record of the session in progress,
// and of the last one until it is finalised. Peer, bandwidth and start time
// come from the session; the rest is the operator's to fill in. Nothing here
// is sent to the peer (R2: signal reports are typed, not exchanged).
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import QsoForm from "./QsoForm.vue";
import { inSession, session, snapshot } from "../store";

const { t } = useI18n();
const qso = computed(() => snapshot.value?.currentQso ?? null);
const shown = computed(() => !!qso.value && (inSession.value || qso.value.state === "draft"));
const bandwidth = computed(() => {
  const bw = qso.value?.bandwidth ?? (session.value?.peer === qso.value?.peer ? session.value?.bandwidth : null);
  return bw ? `BW${bw.replace(/^BW/i, "")}` : t("qso.bandwidthUnknown");
});
</script>

<template>
  <section v-if="shown && qso" class="card stack qso-card" aria-labelledby="h-qso">
    <div class="head">
      <span id="h-qso" class="label">{{ t("qso.title") }}</span>
      <span class="qso-state small">{{ t(qso.state === "final" ? "qso.state.final" : "qso.state.draft") }}</span>
    </div>
    <div class="stat"><span class="muted">{{ t("qso.peer") }}</span><span class="mono qso-peer">{{ qso.peer }}</span></div>
    <div class="stat"><span class="muted">{{ t("qso.bandwidth") }}</span><span class="mono qso-bw">{{ bandwidth }}</span></div>
    <div class="stat"><span class="muted">{{ t("qso.start") }}</span><span class="mono">{{ qso.startUtc.slice(11, 16) }} UTC</span></div>
    <QsoForm :qso="qso" />
  </section>
</template>

<style scoped>
.head { display: flex; justify-content: space-between; align-items: baseline; }
.qso-state { border: 1px solid var(--border-strong); border-radius: 999px; padding: 0 8px; color: var(--text-2); }
.stat { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 8px; border-top: 1px solid var(--border); padding-top: 6px; font-size: calc(13px * var(--font-scale)); }
</style>
