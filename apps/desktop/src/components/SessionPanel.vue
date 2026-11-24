<script setup lang="ts">
import { info } from "../diag";
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
// The right pane: whether incoming calls are answered (the Receive only switch
// is in the window header, D18), the session, the open peer's queue, the
// QSO record (Task 11), the link, the modem's telemetry, beacons and the
// activity log (M3a Task 10). A missing metric is never drawn as a value.
import ActivityDrawer from "./ActivityDrawer.vue";
import BeaconControl from "./BeaconControl.vue";
import QsoCard from "./QsoCard.vue";
import QsyCard from "./QsyCard.vue";
import SessionMenu from "./SessionMenu.vue";
import TelemetryPanel from "./TelemetryPanel.vue";
import { activeProfile, chat, files, inSession, modemLabel, pickFile, queuedBytes, sendFileBlock, session, snapshot } from "../store";

const { t } = useI18n();
const now = ref(Date.now());
let clock: ReturnType<typeof setInterval> | undefined;
onMounted(() => (clock = setInterval(() => (now.value = Date.now()), 1000)));
onUnmounted(() => clearInterval(clock));
/** The canvas's two tabs: the QSO record, and the session's own details. */
const hasQso = computed(() => !!snapshot.value?.currentQso && (inSession.value || snapshot.value.currentQso.state === "draft"));
const tab = ref<"qso" | "session">(hasQso.value ? "qso" : "session");
watch(hasQso, (on) => (tab.value = on ? "qso" : "session"));
const TABS = ["qso", "session"] as const;
function onTabKey(e: KeyboardEvent) {
  const i = TABS.indexOf(tab.value);
  const next = e.key === "ArrowRight" ? i + 1 : e.key === "ArrowLeft" ? i - 1 : e.key === "Home" ? 0 : e.key === "End" ? 1 : null;
  if (next === null) return;
  e.preventDefault();
  tab.value = TABS[(next + 2) % 2];
  document.getElementById(`panel-tab-${tab.value}`)?.focus();
}
const since = (utc: string) => utc.slice(11, 16) + " UTC";
function where() {
  const p = activeProfile.value;
  if (!p) return "";
  if (p.modem.kind === "simulator") return info.value?.simulator === false ? t("profile.simulatorUnavailable") : t("session.simulator");
  if (p.modem.kind === "builtIn") return t("session.builtIn");
  return t("session.mercuryAt", { host: p.modem.host, port: p.modem.controlPort });
}
</script>

<template>
  <div class="panel">
    <QsyCard />
    <div class="panel-tabs" role="tablist" :aria-label="t('session.tabsAria')" @keydown="onTabKey">
      <button
        v-for="name in TABS"
        :id="`panel-tab-${name}`"
        :key="name"
        role="tab"
        type="button"
        :aria-selected="tab === name"
        :aria-controls="`panel-pane-${name}`"
        :tabindex="tab === name ? 0 : -1"
        :class="{ on: tab === name }"
        @click="tab = name"
      >{{ t(name === "qso" ? "session.tabQso" : "session.tabSession") }}</button>
    </div>
    <div v-show="tab === 'qso'" id="panel-pane-qso" class="pane" role="tabpanel" aria-labelledby="panel-tab-qso">
      <QsoCard />
      <p v-if="!hasQso" class="muted small qso-empty">{{ t("session.noQso") }}</p>
    </div>
    <div v-show="tab === 'session'" id="panel-pane-session" class="pane" role="tabpanel" aria-labelledby="panel-tab-session">
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
        <SessionMenu />
        <div class="send-file-row">
          <button
            class="send-file-open"
            :disabled="!!sendFileBlock"
            :title="sendFileBlock ?? undefined"
            :aria-describedby="sendFileBlock ? 'send-file-why' : undefined"
            @click="pickFile"
          >
            {{ t("files.send.button") }}
          </button>
          <span v-if="sendFileBlock" id="send-file-why" class="muted small why">{{ sendFileBlock }}</span>
        </div>
        <p v-if="files.error" class="err small" role="alert">{{ files.error }}</p>
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
    </section>

    <TelemetryPanel />
    <BeaconControl :now="now" />
    <ActivityDrawer />

    <p v-if="snapshot && !activeProfile" class="muted small">{{ t("session.chooseStation") }}</p>
    </div>
  </div>
</template>

<style scoped>
.panel { display: grid; gap: calc(var(--space) * 1.5); align-content: start; padding: calc(var(--space) * 2); overflow-y: auto; height: 100%; }
.pane { display: grid; gap: calc(var(--space) * 1.5); align-content: start; }
.panel-tabs { display: flex; gap: 4px; border-bottom: 1px solid var(--border); }
.panel-tabs button { border: 0; border-bottom: 2px solid transparent; border-radius: 8px 8px 0 0; background: transparent; color: var(--text-2); padding: 6px 14px; font-weight: 600; }
.panel-tabs button.on { color: var(--accent); border-bottom-color: var(--accent); }
.send-file-row { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
.stat { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 8px; border-top: 1px solid var(--border); padding-top: 6px; font-size: calc(13px * var(--font-scale)); }
</style>
