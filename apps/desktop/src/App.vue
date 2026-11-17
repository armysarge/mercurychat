<script setup lang="ts">
// The MercuryChat window (docs/04 §2, M2 parts): header with the explicit
// mode, navigation rail, persistent recovery banners and the views. Every
// action is a typed command (api.ts); state comes back as events (store.ts).
import { invoke } from "@tauri-apps/api/core";
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { call, describeError } from "./api";
import { info, measureIpc } from "./diag";
import { onEvent as notifyOn, startNotifier, stopNotifier } from "./notifier";
import { t } from "./i18n";
import {
  activeProfile, app, chat, detail, mirror, modemLabel, modemState, modeWord, onAppEvent, openPeer, reloadChat, reloadDiscovery, session, snapshot,
  stopTimers, type ViewName,
} from "./store";
import Conversations from "./views/Conversations.vue";
import Diagnostics from "./views/Diagnostics.vue";
import Logbook from "./views/Logbook.vue";
import Operate from "./views/Operate.vue";
import Settings from "./views/Settings.vue";
import Welcome from "./views/Welcome.vue";
import AppLogo from "./components/AppLogo.vue";
import ContextMenu from "./components/ContextMenu.vue";
import Sidebar from "./components/Sidebar.vue";
import StationStrip from "./components/StationStrip.vue";
import WindowControls from "./components/WindowControls.vue";

const startupError = ref("");
const now = ref(new Date());
let clock: ReturnType<typeof setInterval> | undefined;

const RAIL: { page: ViewName; label: string; icon: string }[] = [
  { page: "operate", label: "nav.operate", icon: "M12 8v13M9 21h6M3 8h18M6 5v6M10 4v8M14 5v6M18 6v4" },
  { page: "conversations", label: "nav.conversations", icon: "M7 4h10a4 4 0 0 1 4 4v6a4 4 0 0 1-4 4H9l-5 3v-4a4 4 0 0 1-1-3V8a4 4 0 0 1 4-4Zm0 6h10M7 13h6" },
  { page: "logbook", label: "nav.logbook", icon: "M5 4h11a3 3 0 0 1 3 3v13H8a3 3 0 0 1-3-3V4Zm0 13a3 3 0 0 1 3-3h11M9 8h6" },
  { page: "settings", label: "nav.settings", icon: "M4 6h5m4 0h7M4 12h9m4 0h3M4 18h3m4 0h9M9 4h4v4H9zM13 10h4v4h-4zM7 16h4v4H7z" },
  { page: "diagnostics", label: "nav.diagnostics", icon: "M3 13h4l3-7 4 12 3-5h4" },
];

const firstUse = computed(() => snapshot.value !== null && snapshot.value.profiles.length === 0);
const utc = computed(() => now.value.toISOString().slice(11, 19) + " UTC");
const where = computed(() => {
  const p = activeProfile.value;
  if (!p) return "";
  return p.modem.kind === "simulator" ? t("where.simulator") : t("where.mercury", { host: p.modem.host, port: p.modem.controlPort });
});

/** The persistent banner for the modem's state (docs/04 §3 Recover). */
const banner = computed(() => {
  switch (modemState.value) {
    case "recovering":
      return { tone: "warn", role: "alert", text: t("banner.recovering") };
    case "faulted":
      return { tone: "err", role: "alert", text: t("banner.fault", { detail: detail.value ?? t("banner.faultUnknown") }) };
    case "draining":
      return detail.value === "aborting"
        ? { tone: "warn", role: "status", text: t("banner.aborting") }
        : { tone: "info", role: "status", text: t("banner.finishing") };
    default:
      if (detail.value?.startsWith("aborted:")) {
        return { tone: "warn", role: "status", text: t("banner.aborted", { reason: detail.value.slice(8).trim() }) };
      }
      return null;
  }
});
/** The station's mode, as the core reports it (D18): never ahead of the core. */
const receiveOnly = computed(() => snapshot.value?.receiveOnly ?? false);
/** Chosen during a call or session: it takes effect when the link ends (the live link is never touched). */
const receiveOnlyPending = computed(() => snapshot.value?.receiveOnlyPending ?? false);
const modeWordKey = computed(() => (receiveOnlyPending.value ? "mode.receiveOnlyPending" : receiveOnly.value ? "mode.receiveOnly" : "mode.answering"));
/** The switch's tooltip: while pending, this session still runs (it answered or called), so it does not say "never answers". */
const switchTitleKey = computed(() =>
  receiveOnlyPending.value ? "mode.receiveOnlyPendingTitle" : receiveOnly.value ? "mode.receiveOnlyTitle" : "mode.answeringTitle",
);
/** The full status for the header's tooltip: the state, its detail and what it means. */
const stateTitle = computed(() => [...new Set([modemLabel.value, detail.value, modeWord.value].filter((x): x is string => !!x))].join(" · "));
const rxError = ref("");

/** Ask the core to switch; the switch itself only follows the snapshot, so a refused command leaves it as it was. */
async function toggleReceiveOnly(e: Event) {
  const box = e.target as HTMLInputElement;
  const wanted = box.checked;
  box.checked = receiveOnly.value;
  rxError.value = "";
  try {
    await call({ type: "setReceiveOnly", on: wanted });
  } catch (err) {
    rxError.value = describeError(err);
  } finally {
    box.checked = receiveOnly.value;
  }
}
const otherSession = computed(() => (session.value && session.value.peer !== chat.open ? session.value : null));

function go(page: ViewName) {
  app.page = page;
  app.drawer = null;
}
function onKey(e: KeyboardEvent) {
  // Escape closes a drawer; it never aborts a session (docs/04 §5).
  if (e.key === "Escape" && app.drawer) {
    app.drawer = null;
    e.preventDefault();
  }
}

watch(
  () => activeProfile.value?.id,
  (id, old) => {
    if (id !== old) {
      void reloadChat();
      void reloadDiscovery();
    }
  },
);

onMounted(async () => {
  clock = setInterval(() => (now.value = new Date()), 1000);
  document.addEventListener("keydown", onKey);
  try {
    info.value = await invoke("app_info");
    if (info.value?.startup_error) {
      // No retry loop: the operator reads this and closes the window.
      startupError.value = info.value.startup_error;
      return;
    }
    onAppEvent(notifyOn);
    startNotifier();
    await mirror.start();
    const selfMeasure = await invoke<boolean>("ui_ready");
    if (selfMeasure) await measureIpc(200);
  } catch (e) {
    startupError.value = describeError(e);
    // The window is still hidden behind the splash: show it now, with the error.
    void invoke("ui_ready").catch(() => {});
  }
});
onUnmounted(() => {
  mirror.stop();
  stopTimers();
  stopNotifier();
  clearInterval(clock);
  document.removeEventListener("keydown", onKey);
});
</script>

<template>
  <div v-if="startupError" class="failed">
    <section class="card" role="alert" aria-labelledby="h-fail">
      <h2 id="h-fail">{{ t("app.startFailed") }}</h2>
      <p class="err">{{ startupError }}</p>
      <p class="muted">{{ t("app.closeToExit") }}</p>
    </section>
  </div>

  <div v-else class="shell">
    <!-- This header is the frameless window's title bar. The drag attributes
         handle dragging on platforms without WebView2's native drag region. -->
    <header data-tauri-drag-region>
      <div class="brand" data-tauri-drag-region>
        <AppLogo class="mark" :size="28" data-tauri-drag-region />
        <b>MercuryChat</b><span class="alpha small">{{ t("app.alpha") }}</span>
      </div>
      <button v-if="!firstUse" class="toggle-sidebar quiet" :aria-expanded="app.drawer === 'peers'" @click="app.drawer = app.drawer === 'peers' ? null : 'peers'">{{ t("sidebar.toggle") }}</button>
      <div v-if="activeProfile" class="station" data-tauri-drag-region>
        <b class="mono">{{ activeProfile.callsign }}</b>
        <span class="muted small">{{ where }}</span>
      </div>
      <span v-if="activeProfile" class="station-mode" role="status" aria-live="polite">
        <span :class="['mode-word', { rx: receiveOnly }]">{{ t(modeWordKey) }}</span>
        <label class="switch warn" :title="t(switchTitleKey)">
          <input type="checkbox" role="switch" :checked="receiveOnly" :disabled="modemState === 'unavailable'" :aria-label="t('mode.receiveOnly')" @change="toggleReceiveOnly" />
          <span class="track" aria-hidden="true"></span>
        </label>
      </span>
      <span class="grow" data-tauri-drag-region></span>
      <!-- The modem's state, the one place it is said (owner, 17 Nov): a long detail is cut with an ellipsis; hover shows it all. -->
      <span class="state" role="status" aria-live="polite" :title="stateTitle" data-tauri-drag-region>
        <span :class="['pill', modemState]">{{ modemLabel }}</span>
        <span v-if="detail" class="detail muted small">{{ detail }}</span>
      </span>
      <span class="utc mono small muted" :aria-label="t('app.utcAria')" data-tauri-drag-region>{{ utc }}</span>
      <WindowControls />
    </header>

    <p v-if="rxError" class="banner err" role="alert">
      {{ t("banner.receiveOnlyFailed", { reason: rxError }) }}
      <button class="quiet" @click="rxError = ''">{{ t("banner.dismiss") }}</button>
    </p>
    <p v-if="banner" :class="['banner', banner.tone]" :role="banner.role">{{ banner.text }}</p>
    <p v-if="app.view?.callFailed" class="banner err" role="alert">
      {{ t("banner.callFailed", { reason: app.view.callFailed }) }}
      <button class="quiet" @click="app.view && (app.view.callFailed = null)">{{ t("banner.dismiss") }}</button>
    </p>
    <p v-if="otherSession && app.page === 'operate'" class="banner info" role="status">
      <i18n-t keypath="banner.otherSession" tag="span" scope="global">
        <template #peer><b class="mono">{{ otherSession.peer }}</b></template>
        <template #direction>{{ t(otherSession.direction === "incoming" ? "direction.incomingInline" : "direction.outgoingInline") }}</template>
      </i18n-t>
      <button class="quiet" @click="openPeer(otherSession.peer)">{{ t("banner.open") }}</button>
    </p>

    <!-- The station strip: full width above the sidebar and the workspace, on Operate only. -->
    <StationStrip v-if="snapshot && !firstUse" :now="now.getTime()" />

    <div :class="['body', { solo: firstUse || !snapshot }]">
      <nav class="rail" :aria-label="t('nav.label')">
        <button
          v-for="r in RAIL"
          :key="r.page"
          :class="{ selected: app.page === r.page && !firstUse }"
          :aria-current="app.page === r.page ? 'page' : undefined"
          :title="t(r.label)"
          :aria-label="t(r.label)"
          @click="go(r.page)"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="r.icon" /></svg>
          {{ t(r.label) }}
        </button>
      </nav>
      <Sidebar v-if="snapshot && !firstUse" :class="{ open: app.drawer === 'peers' }" />
      <div v-if="app.drawer === 'peers'" class="scrim" @click="app.drawer = null"></div>
      <main class="view">
        <p v-if="!snapshot" class="muted loading">{{ t("app.loading") }}</p>
        <Welcome v-else-if="firstUse" />
        <Operate v-else-if="app.page === 'operate'" />
        <Conversations v-else-if="app.page === 'conversations'" />
        <Logbook v-else-if="app.page === 'logbook'" />
        <Settings v-else-if="app.page === 'settings'" />
        <Diagnostics v-else />
      </main>
    </div>
  </div>
  <ContextMenu />
</template>

<style scoped>
.failed { padding: calc(var(--space) * 3); }
/* A column: the banners come and go, and the body always takes what is left. */
.shell { display: flex; flex-direction: column; height: 100%; }
header {
  display: flex; align-items: center; gap: calc(var(--space) * 2); min-height: 46px; flex: none;
  padding: 0 0 0 calc(var(--space) * 2); background: var(--surface); border-bottom: 1px solid var(--border);
  user-select: none; -webkit-user-select: none;
  app-region: drag;
}
header > * { flex: none; }
/* With the whole header a native drag region, every control in it must opt out or it stops taking clicks. */
header button, header input, header label, header select, header a { app-region: no-drag; }
header > .grow { flex: 1; align-self: stretch; }
header > .state { flex: 0 1 auto; min-width: 0; }
.utc { margin-right: var(--space); }
.brand { display: flex; align-items: center; gap: var(--space); font-weight: 650; }
.mark { display: block; -webkit-user-drag: none; }
.alpha { color: var(--warning); border: 1px solid currentColor; border-radius: 999px; padding: 0 8px; }
.station { display: flex; flex-direction: column; line-height: 1.2; border-left: 1px solid var(--border); padding-left: calc(var(--space) * 2); }
.station-mode { display: inline-flex; align-items: center; gap: var(--space); }
.mode-word { font-weight: 600; color: var(--success); }
.mode-word.rx { color: var(--warning); }
.toggle-sidebar { display: none; }
.scrim { display: none; }
.state { display: flex; align-items: center; gap: var(--space); min-width: 0; cursor: default; }
.detail { min-width: 0; max-width: 36ch; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.banner {
  flex: none; margin: 0; padding: 6px calc(var(--space) * 2); border-bottom: 1px solid var(--border);
  background: var(--raised); display: flex; gap: var(--space); align-items: center; flex-wrap: wrap;
}
.banner.warn { border-left: 3px solid var(--warning); }
.banner.err { border-left: 3px solid var(--danger); }
.banner.info { border-left: 3px solid var(--accent); }

.body { flex: 1; display: grid; grid-template-columns: minmax(84px, 112px) 260px minmax(0, 1fr); min-height: 0; }
.body.solo { grid-template-columns: minmax(84px, 112px) minmax(0, 1fr); }
.rail { display: flex; flex-direction: column; gap: 6px; padding: var(--space) 6px; border-right: 1px solid var(--border); background: var(--surface); }
.rail button {
  display: grid; justify-items: center; gap: 2px; padding: 8px 2px; font-size: calc(11px * var(--font-scale));
  border-color: transparent; background: transparent; color: var(--text-2);
  overflow-wrap: anywhere; hyphens: auto; text-align: center;
}
.rail svg {
  width: 18px; height: 18px; fill: none; stroke: currentColor;
  stroke-width: 1.7; stroke-linecap: round; stroke-linejoin: round;
}
.rail button.selected { background: var(--raised); color: var(--accent); }
.rail button:nth-child(3) { margin-top: auto; }
.view { min-height: 0; min-width: 0; overflow: hidden; }
.loading { padding: calc(var(--space) * 3); }
@media (max-width: 1099px) {
  .detail, .station .muted { display: none; }
  header { gap: var(--space); }
}
/* Up to 960 px the sidebar is a drawer (Sidebar.vue): the grid loses its column. */
@media (max-width: 960px) {
  .body, .body.solo { grid-template-columns: minmax(84px, 112px) minmax(0, 1fr); }
  .toggle-sidebar { display: inline-block; }
  .scrim { display: block; position: fixed; inset: 0; background: var(--scrim); z-index: 20; }
}
@media (max-width: 900px) {
  .body, .body.solo { grid-template-columns: minmax(64px, 84px) minmax(0, 1fr); }
  .rail button { font-size: calc(9px * var(--font-scale)); }
  .alpha, .utc { display: none; }
}
</style>
