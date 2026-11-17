<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { useI18n } from "vue-i18n";
// The right pane: whether incoming calls are answered (the Receive only switch
// is in the window header, D18), the session, the open peer's queue, the
// QSO record (Task 11), the link, the modem's telemetry, beacons and the
// activity log (M3a Task 10). A missing metric is never drawn as a value.
import ActivityDrawer from "./ActivityDrawer.vue";
import BeaconControl from "./BeaconControl.vue";
import QsoCard from "./QsoCard.vue";
import TelemetryPanel from "./TelemetryPanel.vue";
import { activeProfile, chat, modemLabel, queuedBytes, session, snapshot } from "../store";

const { t } = useI18n();
const now = ref(Date.now());
let clock: ReturnType<typeof setInterval> | undefined;
onMounted(() => (clock = setInterval(() => (now.value = Date.now()), 1000)));
onUnmounted(() => clearInterval(clock));
const since = (utc: string) => utc.slice(11, 16) + " UTC";
function where() {
  const p = activeProfile.value;
  if (!p) return "";
  return p.modem.kind === "simulator" ? t("session.simulator") : t("session.mercuryAt", { host: p.modem.host, port: p.modem.controlPort });
}
</script>

<template>
  <div class="panel">
    <section class="card stack" aria-labelledby="h-listen">
      <span id="h-listen" class="label">{{ t("session.incomingCalls") }}</span>
      <p v-if="snapshot?.receiveOnly" class="muted small">{{ t("session.receiveOnlyNote") }}</p>
      <p v-else-if="snapshot?.listening" class="ok small">{{ t("session.answeredAuto") }}</p>
      <p v-else class="muted small">{{ t("session.notAnswering") }}</p>
    </section>

    <section class="card stack" aria-labelledby="h-session">
      <span id="h-session" class="label">{{ t("session.title") }}</span>
      <template v-if="session">
        <div class="stat"><span class="muted">{{ t("session.peer") }}</span><span class="mono">{{ session.peer }}</span></div>
        <div class="stat"><span class="muted">{{ t("session.direction") }}</span><span>{{ t(session.direction === "outgoing" ? "direction.outgoing" : "direction.incoming") }}</span></div>
        <div class="stat"><span class="muted">{{ t("session.application") }}</span><span>{{ session.peerApp ?? t("session.unknownApp") }} · MCH/1</span></div>
        <div class="stat"><span class="muted">{{ t("session.since") }}</span><span class="mono">{{ since(session.sinceUtc) }}</span></div>
      </template>
      <p v-else class="muted small">{{ t("session.none") }}</p>
      <div v-if="chat.open" class="stat">
        <span class="muted">{{ t("session.queuedFor", { peer: chat.open }) }}</span><span class="mono">{{ t("session.bytesShort", queuedBytes) }}</span>
      </div>
    </section>

    <QsoCard />

    <section class="card stack" aria-labelledby="h-link">
      <span id="h-link" class="label">{{ t("session.link") }}</span>
      <div class="stat"><span class="muted">{{ t("session.modem") }}</span><span class="small">{{ where() }}</span></div>
      <div class="stat"><span class="muted">{{ t("session.state") }}</span><span class="mono">{{ modemLabel }}</span></div>
    </section>

    <TelemetryPanel />
    <BeaconControl :now="now" />
    <ActivityDrawer />

    <p v-if="snapshot && !activeProfile" class="muted small">{{ t("session.chooseStation") }}</p>
  </div>
</template>

<style scoped>
.panel { display: grid; gap: calc(var(--space) * 1.5); align-content: start; padding: calc(var(--space) * 2); overflow-y: auto; height: 100%; }
.stat { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 8px; border-top: 1px solid var(--border); padding-top: 6px; font-size: calc(13px * var(--font-scale)); }
</style>
