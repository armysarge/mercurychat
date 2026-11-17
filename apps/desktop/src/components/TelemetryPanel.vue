<script setup lang="ts">
// The modem's link quality (OBS-01; docs/04 §2): SNR, TX/RX mode, queued
// bytes, throughput, audio and the radio's frequency with its age. A value
// the modem did not give reads "unavailable", never 0. The telemetry link is
// shown: Lost is a warning (never an error) while a session is up, because the
// chat goes on without it.
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import type { Reading } from "../bindings/Reading";
import { inSession, snapshot } from "../store";

const { t } = useI18n();
const tel = computed(() => snapshot.value?.telemetry ?? null);
const linkKey = computed(() => `telemetry.link.${tel.value?.link.type ?? "unconfigured"}`);
const lostWhileUp = computed(() => tel.value?.link.type === "lost" && inSession.value);

/** Its source and when it was taken, with its age where the source gave one. */
function provenance(r: Reading<number>): string {
  const at = r.atUtc.slice(11, 19) + " UTC";
  if (r.ageMs === null) return t("heard.from", { source: r.source, at });
  return t("heard.fromAged", { source: r.source, at, age: (r.ageMs / 1000).toFixed(1) });
}
const mhz = (hz: number) => (hz / 1e6).toFixed(6).replace(/(\.\d{3})(\d{3})$/, "$1 $2");
const age = (r: Reading<number>) => (r.ageMs === null ? null : (r.ageMs / 1000).toFixed(1));
</script>

<template>
  <section class="card stack telemetry" aria-labelledby="h-telemetry">
    <span id="h-telemetry" class="label">{{ t("telemetry.title") }}</span>
    <template v-if="tel">
      <div class="stat link">
        <span class="muted">{{ t("telemetry.linkLabel") }}</span>
        <span :class="['tel-link', tel.link.type, { warn: tel.link.type === 'lost' }]" :title="tel.link.type === 'lost' ? tel.link.reason : undefined">{{ t(linkKey) }}</span>
      </div>
      <p v-if="lostWhileUp" class="warn small" role="status">{{ t("telemetry.lostInSession") }}</p>
      <div class="stat snr">
        <span class="muted">{{ t("telemetry.snr") }}</span>
        <span v-if="tel.snrDb" class="mono" :title="provenance(tel.snrDb)">{{ t("heard.snr", { db: tel.snrDb.value.toFixed(1) }) }}</span>
        <span v-else class="muted unavailable">{{ t("telemetry.unavailable") }}</span>
      </div>
      <div class="stat mode">
        <span class="muted">{{ t("telemetry.mode") }}</span>
        <span v-if="tel.txMode || tel.rxMode" class="mono">{{ t("telemetry.txRx", { tx: tel.txMode ?? "—", rx: tel.rxMode ?? "—" }) }}</span>
        <span v-else class="muted unavailable">{{ t("telemetry.unavailable") }}</span>
      </div>
      <div class="stat queued">
        <span class="muted">{{ t("telemetry.queued") }}</span>
        <span v-if="tel.queuedBytes !== null" class="mono">{{ t("session.bytesShort", tel.queuedBytes) }}</span>
        <span v-else class="muted unavailable">{{ t("telemetry.unavailable") }}</span>
      </div>
      <div class="stat throughput">
        <span class="muted">{{ t("telemetry.throughput") }}</span>
        <span v-if="tel.ackedBps !== null" class="mono" :title="t('telemetry.throughputTitle')">{{ t("telemetry.bps", { n: tel.ackedBps.toFixed(1) }) }}</span>
        <span v-else class="muted unavailable">{{ t("telemetry.unavailable") }}</span>
      </div>
      <div class="stat audio">
        <span class="muted">{{ t("telemetry.audio") }}</span>
        <span v-if="tel.audio" :class="tel.audio.ok ? 'ok' : 'warn'" :title="tel.audio.error ?? undefined">{{ t(tel.audio.ok ? "telemetry.audioOk" : "telemetry.audioFault") }}</span>
        <span v-else class="muted unavailable">{{ t("telemetry.unavailable") }}</span>
      </div>
      <div class="stat frequency">
        <span class="muted">{{ t("telemetry.frequency") }}</span>
        <span v-if="tel.frequency" class="mono" :title="provenance(tel.frequency)">
          {{ t("telemetry.mhz", { mhz: mhz(tel.frequency.value) }) }}<template v-if="age(tel.frequency) !== null"> · {{ t("telemetry.ageS", { s: age(tel.frequency) }) }}</template>
        </span>
        <span v-else class="muted unavailable">{{ t("telemetry.unavailable") }}</span>
      </div>
    </template>
  </section>
</template>

<style scoped>
.stat { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 8px; border-top: 1px solid var(--border); padding-top: 6px; font-size: calc(13px * var(--font-scale)); }
.tel-link.up { color: var(--success); }
.tel-link.connecting { color: var(--warning); }
</style>
