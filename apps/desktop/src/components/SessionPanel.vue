<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
// The right pane: Receive only switch (D18; listening is the default), the session, the open peer's
// queue, and the link. No telemetry exists yet, so none is shown: a missing
// metric is never drawn as a value.
import { activeProfile, chat, modemLabel, modemState, queuedBytes, session, setReceiveOnly, snapshot } from "../store";

// Receive only is a safety switch: it can change in any state with a station
// (mid-session, calling, recovering); the modem acts on it at the earliest moment.
const { t } = useI18n();
const canSwitch = computed(() => modemState.value !== "unavailable");
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
      <label class="switch">
        <input
          class="listen"
          type="checkbox"
          :checked="snapshot?.receiveOnly ?? false"
          :disabled="!canSwitch"
          @change="setReceiveOnly(($event.target as HTMLInputElement).checked)"
        />
        {{ t("session.receiveOnly") }}
      </label>
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

    <section class="card stack" aria-labelledby="h-link">
      <span id="h-link" class="label">{{ t("session.link") }}</span>
      <div class="stat"><span class="muted">{{ t("session.modem") }}</span><span class="small">{{ where() }}</span></div>
      <div class="stat"><span class="muted">{{ t("session.state") }}</span><span class="mono">{{ modemLabel }}</span></div>
      <p class="muted small">{{ t("session.noReadings") }}</p>
    </section>

    <p v-if="snapshot && !activeProfile" class="muted small">{{ t("session.chooseStation") }}</p>
  </div>
</template>

<style scoped>
.panel { display: grid; gap: calc(var(--space) * 1.5); align-content: start; padding: calc(var(--space) * 2); overflow-y: auto; height: 100%; }
.switch { flex-direction: row; align-items: center; gap: 8px; color: var(--text); font-size: 14px; }
.stat { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 8px; border-top: 1px solid var(--border); padding-top: 6px; font-size: 13px; }
</style>
