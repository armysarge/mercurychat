<script setup lang="ts">
import { computed } from "vue";
// The right pane: Receive only switch (D18; listening is the default), the session, the open peer's
// queue, and the link. No telemetry exists yet, so none is shown: a missing
// metric is never drawn as a value.
import { activeProfile, chat, modemState, queuedBytes, session, setReceiveOnly, snapshot } from "../store";

// Receive only can change while idle and mid-session (LISTEN does not end a link).
const canSwitch = computed(() =>
  ["ready", "listening", "negotiating", "connected", "draining"].includes(modemState.value),
);
const since = (utc: string) => utc.slice(11, 16) + " UTC";
function where() {
  const p = activeProfile.value;
  if (!p) return "";
  return p.modem.kind === "simulator" ? "Simulator (no radio)" : `Mercury at ${p.modem.host}:${p.modem.controlPort}`;
}
</script>

<template>
  <div class="panel">
    <section class="card stack" aria-labelledby="h-listen">
      <span id="h-listen" class="label">Incoming calls</span>
      <label class="switch">
        <input
          class="listen"
          type="checkbox"
          :checked="snapshot?.receiveOnly ?? false"
          :disabled="!canSwitch"
          @change="setReceiveOnly(($event.target as HTMLInputElement).checked)"
        />
        Receive only
      </label>
      <p v-if="snapshot?.receiveOnly" class="muted small">Receive only: incoming calls are not answered.</p>
      <p v-else-if="snapshot?.listening" class="ok small">Incoming calls answered automatically</p>
      <p v-else class="muted small">Not answering calls right now.</p>
    </section>

    <section class="card stack" aria-labelledby="h-session">
      <span id="h-session" class="label">Session</span>
      <template v-if="session">
        <div class="stat"><span class="muted">Peer</span><span class="mono">{{ session.peer }}</span></div>
        <div class="stat"><span class="muted">Direction</span><span>{{ session.direction === "outgoing" ? "We called" : "They called" }}</span></div>
        <div class="stat"><span class="muted">Application</span><span>{{ session.peerApp ?? "unknown" }} · MCH/1</span></div>
        <div class="stat"><span class="muted">Since</span><span class="mono">{{ since(session.sinceUtc) }}</span></div>
      </template>
      <p v-else class="muted small">No session.</p>
      <div v-if="chat.open" class="stat">
        <span class="muted">Queued for {{ chat.open }}</span><span class="mono">{{ queuedBytes }} B</span>
      </div>
    </section>

    <section class="card stack" aria-labelledby="h-link">
      <span id="h-link" class="label">Link</span>
      <div class="stat"><span class="muted">Modem</span><span class="small">{{ where() }}</span></div>
      <div class="stat"><span class="muted">State</span><span class="mono">{{ modemState }}</span></div>
      <p class="muted small">Signal, audio and frequency readings are not available in this version.</p>
    </section>

    <p v-if="snapshot && !activeProfile" class="muted small">Choose a station in Settings.</p>
  </div>
</template>

<style scoped>
.panel { display: grid; gap: calc(var(--space) * 1.5); align-content: start; padding: calc(var(--space) * 2); overflow-y: auto; height: 100%; }
.switch { flex-direction: row; align-items: center; gap: 8px; color: var(--text); font-size: 14px; }
.stat { display: flex; justify-content: space-between; gap: 8px; border-top: 1px solid var(--border); padding-top: 6px; font-size: 13px; }
</style>
