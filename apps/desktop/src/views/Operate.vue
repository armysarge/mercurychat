<script setup lang="ts">
// The main workspace (docs/04 §2, M2 parts): the conversation and the session
// pane (the conversation list is the app's sidebar, App.vue). At ≤1439 px the
// session pane is a drawer. With no conversation open, the station home
// (M5 Task 11b) stands alone: no peer header, no composer (Task 11c, owner).
// The peer, the mode and Hang up / Abort stay in the conversation header so
// they are always reachable (docs/04 §7); while a link is up the header stays
// even with no conversation open. The rest is in its "…" (the station menu).
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import Composer from "../components/Composer.vue";
import { openFrom } from "../stationMenu";
import SessionPanel from "../components/SessionPanel.vue";
import StationHome from "../components/StationHome.vue";
import Transcript from "../components/Transcript.vue";
import {
  abort, app, cancelCall, canAbort, canCall, chat, disconnect, inSession, linkPeer, modemState, placeCall, session,
} from "../store";

const { t } = useI18n();
/** Wide windows dock the QSO record panel; narrower ones open it as a drawer. */
const WIDE = "(min-width: 1440px)";
const wide = ref(window.matchMedia(WIDE).matches);
const onWide = (e: MediaQueryListEvent) => (wide.value = e.matches);
onMounted(() => window.matchMedia(WIDE).addEventListener("change", onWide));
onUnmounted(() => window.matchMedia(WIDE).removeEventListener("change", onWide));
/** The docked panel can be hidden too (canvas: the "QSO record" toggle); remembered. */
const read = () => { try { return localStorage.getItem("mc.qsoPanelHidden") === "1"; } catch { return false; } };
const panelHidden = ref(read());
const panelShown = computed(() => (wide.value ? !panelHidden.value : app.drawer === "session"));
function toggleSession() {
  if (!wide.value) {
    app.drawer = app.drawer === "session" ? null : "session";
    return;
  }
  panelHidden.value = !panelHidden.value;
  try { localStorage.setItem("mc.qsoPanelHidden", panelHidden.value ? "1" : "0"); } catch { /* per-viewer convenience only */ }
}

/** The header's station: the open conversation's, else a live link's (Abort stays reachable). */
const peer = computed(() => chat.open || (canAbort.value ? session.value?.peer ?? linkPeer.value ?? "" : ""));

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
const live = computed(() => !!session.value && session.value.peer === peer.value);

const subtitle = computed(() => {
  const s = session.value;
  if (s && s.peer === peer.value) {
    const direction = t(s.direction === "outgoing" ? "direction.outgoingInline" : "direction.incomingInline");
    return t("operate.subtitleLive", { app: s.peerApp ?? t("operate.unknownApp"), direction, since: s.sinceUtc.slice(11, 16) });
  }
  if (linkPeer.value === peer.value && peer.value) return t("operate.settingUp");
  return t("operate.historyOnly");
});
</script>

<template>
  <div :class="['operate', { 'panel-hidden': wide && panelHidden }]">
    <section class="chat-pane" :aria-label="t('operate.conversationAria')">
      <div v-if="peer" class="peer-header">
        <div class="who">
          <h1 class="mono">{{ peer }}</h1>
          <p class="muted small"><span v-if="live" class="live-dot" aria-hidden="true"></span>{{ subtitle }}</p>
        </div>
        <div class="actions row">
          <button v-if="modemState === 'calling'" class="quiet" @click="cancelCall">{{ t("operate.cancel") }}</button>
          <button v-else-if="!inSession" class="primary" :disabled="!canCall || !chat.open" @click="placeCall">{{ t("operate.call") }}</button>
          <button class="disconnect" :disabled="!inSession" @click="disconnect">{{ t("operate.disconnect") }}</button>
          <!-- Abort (owner, 24 Nov: hide it): only while a disconnect is still draining, the way out of a stuck close. -->
          <button v-if="modemState === 'draining'" class="abort danger" :disabled="!canAbort" @click="abort">{{ t("operate.abort") }}</button>
          <button class="toggle-session quiet" :aria-pressed="panelShown" @click="toggleSession">{{ t("operate.qsoRecord") }}</button>
          <button
            class="more quiet"
            aria-haspopup="menu"
            :aria-label="t('stationMenu.more', { callsign: peer })"
            :title="t('stationMenu.more', { callsign: peer })"
            @click="openFrom($event.currentTarget as HTMLElement, peer, 'header')"
          >…</button>
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
      <Transcript v-if="chat.open" class="grow-area" />
      <StationHome v-else />
      <p v-if="chat.error" class="err small errline" role="alert">{{ chat.error }}</p>
      <Composer v-if="chat.open" />
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
.operate.panel-hidden { grid-template-columns: minmax(0, 1fr); }
.operate.panel-hidden .session-pane { display: none; }
.more { font-size: calc(18px * var(--font-scale)); line-height: 1; padding-inline: 12px; }
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
