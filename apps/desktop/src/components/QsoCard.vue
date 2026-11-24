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
const freq = computed(() => (qso.value?.freq ? (qso.value.freq.value / 1e6).toFixed(3) : "-"));
const bandwidth = computed(() => {
  const bw = qso.value?.bandwidth ?? (session.value?.peer === qso.value?.peer ? session.value?.bandwidth : null);
  return bw ? `BW${bw.replace(/^BW/i, "")}` : t("qso.bandwidthUnknown");
});
</script>

<template>
  <section v-if="shown && qso" class="card stack qso-card" aria-labelledby="h-qso">
    <div class="head">
      <h3 id="h-qso" class="qso-title">{{ t("qso.titleWith") }} <span class="mono qso-peer">{{ qso.peer }}</span></h3>
      <span :class="['qso-state', 'small', qso.state === 'final' ? 'final' : 'draft']">{{ t(qso.state === "final" ? "qso.state.final" : "qso.state.draft") }}</span>
    </div>
    <!-- The canvas's 2x2 grid: frequency, band, bandwidth, started. -->
    <dl class="facts">
      <div><dt>{{ t("qso.freqMhz") }}</dt><dd class="mono">{{ freq }}</dd></div>
      <div><dt>{{ t("qso.band") }}</dt><dd class="mono">{{ qso.band ?? "-" }}</dd></div>
      <div><dt>{{ t("qso.bandwidth") }}</dt><dd class="mono qso-bw">{{ bandwidth }}</dd></div>
      <div><dt>{{ t("qso.start") }}</dt><dd class="mono">{{ qso.startUtc.slice(11, 16) }} UTC</dd></div>
    </dl>
    <p v-if="fromPeer.length" class="small peer-filled">
      <span v-for="f in fromPeer" :key="f" class="badge peer-report" :data-field="f">{{ t("qso.peerReport", { field: t(`qso.field.${f}`) }) }}</span>
    </p>
    <QsoForm :qso="qso" />
  </section>
</template>

<style scoped>
.head { display: flex; justify-content: space-between; align-items: center; gap: 8px; }
.qso-title { margin: 0; font-size: calc(16px * var(--font-scale)); font-weight: 600; }
.facts { display: grid; grid-template-columns: 1fr 1fr; gap: 8px; margin: 0; }
.facts > div { background: var(--surface); border: 1px solid var(--border); border-radius: 10px; padding: 8px 10px; min-width: 0; }
.facts dt { font-size: calc(11px * var(--font-scale)); color: var(--text-2); text-transform: uppercase; letter-spacing: 0.05em; }
.facts dd { margin: 2px 0 0; overflow-wrap: anywhere; }
.qso-state.draft { color: var(--warning); border-color: color-mix(in srgb, var(--warning) 50%, var(--border-strong)); }
.peer-filled { display: flex; flex-wrap: wrap; gap: 4px; margin: 0; }
.peer-report { border: 1px solid var(--border-strong); border-radius: 999px; padding: 0 8px; color: var(--text-2); }
.qso-state { border: 1px solid var(--border-strong); border-radius: 999px; padding: 0 8px; color: var(--text-2); }
</style>
