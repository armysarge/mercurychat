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
const PEER_FIELDS = ["rstRcvd", "name", "qth"] as const;
/** Fields the peer's report filled (B6): each says so until the operator changes it. */
const fromPeer = computed(() => {
  const q = qso.value;
  if (!q) return [];
  const source = { rstRcvd: q.rstRcvdSource, name: q.nameSource, qth: q.qthSource };
  return PEER_FIELDS.filter((f) => source[f] === "peer report");
});
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
    <p v-if="fromPeer.length" class="small peer-filled">
      <span v-for="f in fromPeer" :key="f" class="badge peer-report" :data-field="f">{{ t("qso.peerReport", { field: t(`qso.field.${f}`) }) }}</span>
    </p>
    <QsoForm :qso="qso" />
  </section>
</template>

<style scoped>
.head { display: flex; justify-content: space-between; align-items: baseline; }
.peer-filled { display: flex; flex-wrap: wrap; gap: 4px; margin: 0; }
.peer-report { border: 1px solid var(--border-strong); border-radius: 999px; padding: 0 8px; color: var(--text-2); }
.qso-state { border: 1px solid var(--border-strong); border-radius: 999px; padding: 0 8px; color: var(--text-2); }
.stat { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 8px; border-top: 1px solid var(--border); padding-top: 6px; font-size: calc(13px * var(--font-scale)); }
</style>
