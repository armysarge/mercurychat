<script setup lang="ts">
// The main workspace (docs/04 §2, M2 parts): conversations | conversation |
// session. At 1100–1439 px the session pane is a drawer; at ≤1099 px the
// conversations pane is one too. The peer, the mode and Disconnect / Abort
// stay in the conversation header so they are always reachable (docs/04 §7).
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import Composer from "../components/Composer.vue";
import ConversationList from "../components/ConversationList.vue";
import SessionPanel from "../components/SessionPanel.vue";
import Transcript from "../components/Transcript.vue";
import {
  abort, app, cancelCall, canAbort, canCall, chat, disconnect, inSession, linkPeer, modemState, placeCall, session,
} from "../store";

const { t } = useI18n();
const toggle = (d: "peers" | "session") => (app.drawer = app.drawer === d ? null : d);

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
    <aside :class="['conv-pane', { open: app.drawer === 'peers' }]" :aria-label="t('operate.conversationsAria')">
      <ConversationList @opened="app.drawer = null" />
    </aside>

    <section class="chat-pane" :aria-label="t('operate.conversationAria')">
      <div class="peer-header">
        <div class="who">
          <h1 class="mono">{{ chat.open || t("operate.noConversation") }}</h1>
          <p class="muted small">{{ chat.open ? subtitle : t("operate.typeCallsign") }}</p>
        </div>
        <div class="actions row">
          <button class="toggle-peers quiet" :aria-expanded="app.drawer === 'peers'" @click="toggle('peers')">{{ t("operate.peers") }}</button>
          <button v-if="modemState === 'calling'" class="quiet" @click="cancelCall">{{ t("operate.cancel") }}</button>
          <button v-else class="primary" :disabled="!canCall || !chat.open" @click="placeCall">{{ t("operate.call") }}</button>
          <button class="disconnect" :disabled="!inSession" @click="disconnect">{{ t("operate.disconnect") }}</button>
          <button class="abort danger" :disabled="!canAbort" @click="abort">{{ t("operate.abort") }}</button>
          <button class="toggle-session quiet" :aria-expanded="app.drawer === 'session'" @click="toggle('session')">{{ t("operate.session") }}</button>
        </div>
      </div>
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
      <Transcript class="grow-area" />
      <p v-if="chat.error" class="err small errline" role="alert">{{ chat.error }}</p>
      <Composer />
    </section>

    <aside :class="['session-pane', { open: app.drawer === 'session' }]" :aria-label="t('operate.sessionAria')">
      <SessionPanel />
    </aside>
    <div v-if="app.drawer" class="scrim" @click="app.drawer = null"></div>
  </div>
</template>

<style scoped>
.operate { display: grid; grid-template-columns: 260px minmax(0, 1fr) 300px; height: 100%; min-height: 0; }
.conv-pane { border-right: 1px solid var(--border); min-height: 0; background: var(--canvas); }
.session-pane { border-left: 1px solid var(--border); min-height: 0; background: var(--canvas); }
.chat-pane { display: flex; flex-direction: column; min-width: 0; min-height: 0; background: linear-gradient(180deg, var(--surface), var(--canvas) 45%); }
.peer-header {
  display: flex; flex-wrap: wrap; gap: var(--space); align-items: center; justify-content: space-between;
  padding: calc(var(--space) * 1.5) calc(var(--space) * 2); border-bottom: 1px solid var(--border);
}
.peer-header h1 { font-size: 20px; font-weight: 600; overflow-wrap: anywhere; }
.actions { justify-content: flex-end; flex-wrap: wrap; }
.toggle-peers, .toggle-session { display: none; }
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
  .operate { grid-template-columns: 260px minmax(0, 1fr); }
  .toggle-session { display: inline-block; }
  .session-pane {
    position: fixed; top: 0; right: 0; bottom: 0; width: min(320px, 92vw); z-index: 30;
    transform: translateX(100%); visibility: hidden; box-shadow: -8px 0 24px rgb(0 0 0 / 0.35);
    transition: transform 120ms ease-out, visibility 120ms;
  }
  .session-pane.open { transform: none; visibility: visible; }
  .scrim { display: block; position: fixed; inset: 0; background: rgb(0 0 0 / 0.35); z-index: 20; }
}
@media (max-width: 1099px) {
  .operate { grid-template-columns: minmax(0, 1fr); }
  .toggle-peers { display: inline-block; }
  .conv-pane {
    position: fixed; top: 0; left: 0; bottom: 0; width: min(300px, 92vw); z-index: 30;
    transform: translateX(-100%); visibility: hidden; box-shadow: 8px 0 24px rgb(0 0 0 / 0.35);
    transition: transform 120ms ease-out, visibility 120ms;
  }
  .conv-pane.open { transform: none; visibility: visible; }
  .peer-header { padding: var(--space) calc(var(--space) * 1.5); }
  .peer-header h1 { font-size: 17px; }
}
@media (max-height: 560px) {
  .peer-header .who p { display: none; }
}
</style>
