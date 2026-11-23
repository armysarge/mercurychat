<script setup lang="ts">
// The main workspace (docs/04 §2, M2 parts): the conversation and the session
// pane (the conversation list is the app's sidebar, App.vue). At ≤1439 px the
// session pane is a drawer. With no conversation open, the station home
// (M5 Task 11b) stands where the transcript would be. The peer, the mode and Disconnect / Abort stay in
// the conversation header so they are always reachable (docs/04 §7).
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { describeError } from "../api";
import Avatar from "../components/Avatar.vue";
import Composer from "../components/Composer.vue";
import { exportConversation } from "../exports";
import SessionPanel from "../components/SessionPanel.vue";
import StationHome from "../components/StationHome.vue";
import Transcript from "../components/Transcript.vue";
import {
  abort, app, cancelCall, canAbort, canCall, chat, disconnect, inSession, linkPeer, modemState, placeCall, session,
} from "../store";

const { t } = useI18n();
const toggleSession = () => (app.drawer = app.drawer === "session" ? null : "session");

/** M5 §7: this conversation as text, saved where the operator chooses. */
const exporting = ref(false);
const exportNote = ref("");
const exportFailed = ref(false);
async function exportChat() {
  const peer = chat.open;
  if (!peer) return;
  exporting.value = true;
  exportNote.value = "";
  exportFailed.value = false;
  try {
    const n = await exportConversation(peer);
    if (n !== null) exportNote.value = t("operate.exported", { n, peer }, n);
  } catch (e) {
    exportFailed.value = true;
    exportNote.value = describeError(e);
  } finally {
    exporting.value = false;
  }
}

const STEPS = ["calling", "negotiating", "connected"] as const;
const progress = computed(() =>
  linkPeer.value === chat.open && STEPS.includes(modemState.value as (typeof STEPS)[number]) ? modemState.value : null,
);
const STEP_LABEL = {
  calling: "operate.steps.calling",
  negotiating: "operate.steps.negotiating",
  connected: "operate.steps.connected",
} as const;
const reached = (step: string) => progress.value !== null && STEPS.indexOf(step as never) <= STEPS.indexOf(progress.value as never);

/** A link is up with the open conversation's station. */
const live = computed(() => !!session.value && session.value.peer === chat.open);

const subtitle = computed(() => {
  const s = session.value;
  if (s && s.peer === chat.open) {
    const direction = t(s.direction === "outgoing" ? "direction.outgoingInline" : "direction.incomingInline");
    return t("operate.subtitle", { app: s.peerApp ?? t("operate.unknownApp"), direction });
  }
  if (linkPeer.value === chat.open && chat.open) return t("operate.settingUp");
  return t("operate.historyOnly");
});
</script>

<template>
  <div class="operate">
    <section class="chat-pane" :aria-label="t('operate.conversationAria')">
      <div class="peer-header">
        <Avatar v-if="chat.open" :callsign="chat.open" :live="live" :size="40" />
        <div class="who">
          <h1 class="mono">{{ chat.open || t("operate.noConversation") }}</h1>
          <p class="muted small"><span v-if="live" class="live-dot" aria-hidden="true"></span>{{ chat.open ? subtitle : t("operate.typeCallsign") }}</p>
        </div>
        <div class="actions row">
          <button v-if="modemState === 'calling'" class="quiet" @click="cancelCall">{{ t("operate.cancel") }}</button>
          <button v-else class="primary" :disabled="!canCall || !chat.open" @click="placeCall">{{ t("operate.call") }}</button>
          <button class="disconnect" :disabled="!inSession" @click="disconnect">{{ t("operate.disconnect") }}</button>
          <button class="abort danger" :disabled="!canAbort" @click="abort">{{ t("operate.abort") }}</button>
          <button class="toggle-session quiet" :aria-expanded="app.drawer === 'session'" @click="toggleSession">{{ t("operate.session") }}</button>
          <button class="export-conversation quiet" :disabled="!chat.open || exporting" :title="t('operate.exportHint')" @click="exportChat">{{ t("operate.export") }}</button>
        </div>
      </div>
      <p v-if="exportNote" :class="['export-note', 'small', exportFailed ? 'err' : 'ok']" :role="exportFailed ? 'alert' : 'status'">{{ exportNote }}</p>
      <ol v-if="progress" class="progress small" :aria-label="t('operate.progressAria')">
        <li v-for="s in STEPS" :key="s" :class="{ done: reached(s), now: s === progress }">
          {{ t(STEP_LABEL[s]) }}
        </li>
      </ol>
      <p v-if="session && session.peer !== chat.open && chat.open" class="other small">
        <i18n-t keypath="operate.otherSession" tag="span" scope="global">
          <template #peer><b class="mono">{{ session.peer }}</b></template>
        </i18n-t>
      </p>
      <Transcript v-if="chat.open" class="grow-area" />
      <StationHome v-else />
      <p v-if="chat.error" class="err small errline" role="alert">{{ chat.error }}</p>
      <Composer />
    </section>

    <aside :class="['session-pane', { open: app.drawer === 'session' }]" :aria-label="t('operate.sessionAria')">
      <SessionPanel />
    </aside>
    <div v-if="app.drawer === 'session'" class="scrim" @click="app.drawer = null"></div>
  </div>
</template>

<style scoped>
.operate { display: grid; grid-template-columns: minmax(0, 1fr) 300px; height: 100%; min-height: 0; }
.session-pane { border-left: 1px solid var(--border); min-height: 0; background: var(--canvas); }
.chat-pane { display: flex; flex-direction: column; min-width: 0; min-height: 0; background: linear-gradient(180deg, var(--surface), var(--canvas) 45%); }
.peer-header {
  display: flex; flex-wrap: wrap; gap: var(--space) calc(var(--space) * 1.5); align-items: center;
  padding: calc(var(--space) * 1.5) calc(var(--space) * 2); border-bottom: 1px solid var(--border);
  background: var(--surface);
}
.peer-header .who { flex: 1 1 18ch; min-width: 0; }
.live-dot { display: inline-block; width: 8px; height: 8px; border-radius: 50%; background: var(--success); margin-right: 6px; vertical-align: 1px; }
.peer-header h1 { font-size: calc(20px * var(--font-scale)); font-weight: 600; overflow-wrap: break-word; }
.actions { justify-content: flex-end; flex-wrap: wrap; margin-left: auto; }
.toggle-session { display: none; }
.export-note { margin: 0; padding: 4px calc(var(--space) * 2); border-bottom: 1px solid var(--border); }
.progress { list-style: none; display: flex; flex-wrap: wrap; gap: var(--space); margin: 0; padding: 6px calc(var(--space) * 2); border-bottom: 1px solid var(--border); }
.progress li { color: var(--text-2); }
.progress li + li::before { content: "→ "; color: var(--text-2); }
.progress li.done { color: var(--text); }
.progress li.now { color: var(--accent); font-weight: 600; }
.other { padding: 6px calc(var(--space) * 2); color: var(--warning); border-bottom: 1px solid var(--border); }
.grow-area { flex: 1; }
.errline { padding: 0 calc(var(--space) * 2) 6px; }
.scrim { display: none; }

@media (max-width: 1439px) {
  .operate { grid-template-columns: minmax(0, 1fr); }
  .toggle-session { display: inline-block; }
  .session-pane {
    position: fixed; top: 0; right: 0; bottom: 0; width: min(320px, 92vw); z-index: 30;
    transform: translateX(100%); visibility: hidden; box-shadow: -8px 0 24px var(--shadow);
    transition: transform 120ms ease-out, visibility 120ms;
  }
  .session-pane.open { transform: none; visibility: visible; }
  .scrim { display: block; position: fixed; inset: 0; background: var(--scrim); z-index: 20; }
}
@media (max-width: 1099px) {
  .peer-header { padding: var(--space) calc(var(--space) * 1.5); }
  .peer-header h1 { font-size: calc(17px * var(--font-scale)); }
}
@media (max-height: 560px) {
  .peer-header .who p { display: none; }
}
</style>
