<script setup lang="ts">
// The MercuryChat window (docs/04 §2, M2 parts): header with the explicit
// mode, navigation rail, persistent recovery banners and the views. Every
// action is a typed command (api.ts); state comes back as events (store.ts).
import { invoke } from "@tauri-apps/api/core";
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { describeError } from "./api";
import { info, measureIpc } from "./diag";
import {
  activeProfile, app, chat, detail, mirror, modemState, modeWord, openPeer, reloadChat, session, snapshot, stopTimers,
  type ViewName,
} from "./store";
import Conversations from "./views/Conversations.vue";
import Diagnostics from "./views/Diagnostics.vue";
import Operate from "./views/Operate.vue";
import Settings from "./views/Settings.vue";
import Welcome from "./views/Welcome.vue";
import WindowControls from "./components/WindowControls.vue";

const startupError = ref("");
const now = ref(new Date());
let clock: ReturnType<typeof setInterval> | undefined;

const RAIL: { page: ViewName; label: string; icon: string }[] = [
  { page: "operate", label: "Operate", icon: "M12 12.5V21m-4 0h8M8.5 6.5a5 5 0 0 0 0 7m7 0a5 5 0 0 0 0-7M5 3a10 10 0 0 0 0 14m14 0a10 10 0 0 0 0-14M12 9.5a1.5 1.5 0 1 0 0 3 1.5 1.5 0 0 0 0-3Z" },
  { page: "conversations", label: "Conversations", icon: "M7 4h10a4 4 0 0 1 4 4v6a4 4 0 0 1-4 4H9l-5 3v-4a4 4 0 0 1-1-3V8a4 4 0 0 1 4-4Zm0 6h10M7 13h6" },
  { page: "settings", label: "Settings", icon: "M4 6h5m4 0h7M4 12h9m4 0h3M4 18h3m4 0h9M9 4h4v4H9zM13 10h4v4h-4zM7 16h4v4H7z" },
  { page: "diagnostics", label: "Diagnostics", icon: "M3 13h4l3-7 4 12 3-5h4" },
];

const firstUse = computed(() => snapshot.value !== null && snapshot.value.profiles.length === 0);
const utc = computed(() => now.value.toISOString().slice(11, 19) + " UTC");
const where = computed(() => {
  const p = activeProfile.value;
  if (!p) return "";
  return p.modem.kind === "simulator" ? "Simulator · no radio" : `Mercury ${p.modem.host}:${p.modem.controlPort}`;
});

/** The persistent banner for the modem's state (docs/04 §3 Recover). */
const banner = computed(() => {
  switch (modemState.value) {
    case "recovering":
      return { tone: "warn", role: "alert", text: "Reconnecting to the modem after the connection was lost. Nothing is resent automatically." };
    case "faulted":
      return { tone: "err", role: "alert", text: `Modem fault: ${detail.value ?? "unknown"}. Use the station again in Settings to retry.` };
    case "draining":
      return detail.value === "aborting"
        ? { tone: "warn", role: "status", text: "Aborting. RF activity may continue briefly." }
        : { tone: "info", role: "status", text: "Finishing queued traffic, then disconnecting. Abort stops at once." };
    default:
      if (detail.value?.startsWith("aborted:")) {
        return { tone: "warn", role: "status", text: `Session aborted: ${detail.value.slice(8).trim()}. Check the messages marked uncertain.` };
      }
      return null;
  }
});
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
    if (id !== old) void reloadChat();
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
    await mirror.start();
    const selfMeasure = await invoke<boolean>("ui_ready");
    if (selfMeasure) await measureIpc(200);
  } catch (e) {
    startupError.value = describeError(e);
  }
});
onUnmounted(() => {
  mirror.stop();
  stopTimers();
  clearInterval(clock);
  document.removeEventListener("keydown", onKey);
});
</script>

<template>
  <div v-if="startupError" class="failed">
    <section class="card" role="alert" aria-labelledby="h-fail">
      <h2 id="h-fail">MercuryChat cannot start</h2>
      <p class="err">{{ startupError }}</p>
      <p class="muted">Close this window to exit.</p>
    </section>
  </div>

  <div v-else class="shell">
    <!-- The window has no system frame: this header is the title bar. Every
         non-interactive part of it drags the window; a double-click maximises. -->
    <header data-tauri-drag-region>
      <div class="brand" data-tauri-drag-region>
        <img class="mark" src="/mercurychat.svg" width="28" height="28" alt="" data-tauri-drag-region />
        <b>MercuryChat</b><span class="alpha small">early alpha</span>
      </div>
      <div v-if="activeProfile" class="station" data-tauri-drag-region>
        <b class="mono">{{ activeProfile.callsign }}</b>
        <span class="muted small">{{ where }}</span>
      </div>
      <span class="mode" role="status" aria-live="polite" data-tauri-drag-region>{{ modeWord }}</span>
      <span class="grow" data-tauri-drag-region></span>
      <span class="state" data-tauri-drag-region>
        <span :class="['pill', modemState]">{{ modemState }}</span>
        <span v-if="detail" class="detail muted small">{{ detail }}</span>
      </span>
      <span class="utc mono small muted" aria-label="UTC time" data-tauri-drag-region>{{ utc }}</span>
      <WindowControls />
    </header>

    <p v-if="banner" :class="['banner', banner.tone]" :role="banner.role">{{ banner.text }}</p>
    <p v-if="app.view?.callFailed" class="banner err" role="alert">
      Call failed: {{ app.view.callFailed }}
      <button class="quiet" @click="app.view && (app.view.callFailed = null)">Dismiss</button>
    </p>
    <p v-if="otherSession && app.page === 'operate'" class="banner info" role="status">
      Session with <b class="mono">{{ otherSession.peer }}</b> ({{ otherSession.direction === "incoming" ? "they called" : "we called" }}).
      <button class="quiet" @click="openPeer(otherSession.peer)">Open</button>
    </p>

    <div class="body">
      <nav class="rail" aria-label="Sections">
        <button
          v-for="r in RAIL"
          :key="r.page"
          :class="{ selected: app.page === r.page && !firstUse }"
          :aria-current="app.page === r.page ? 'page' : undefined"
          @click="go(r.page)"
        >
          <svg viewBox="0 0 24 24" aria-hidden="true"><path :d="r.icon" /></svg>
          {{ r.label }}
        </button>
      </nav>
      <main class="view">
        <p v-if="!snapshot" class="muted loading">Loading…</p>
        <Welcome v-else-if="firstUse" />
        <Operate v-else-if="app.page === 'operate'" />
        <Conversations v-else-if="app.page === 'conversations'" />
        <Settings v-else-if="app.page === 'settings'" />
        <Diagnostics v-else />
      </main>
    </div>
  </div>
</template>

<style scoped>
.failed { padding: calc(var(--space) * 3); }
/* A column: the banners come and go, and the body always takes what is left. */
.shell { display: flex; flex-direction: column; height: 100%; }
header {
  display: flex; align-items: center; gap: calc(var(--space) * 2); min-height: 46px; flex: none;
  padding: 0 0 0 calc(var(--space) * 2); background: var(--surface); border-bottom: 1px solid var(--border);
  user-select: none; -webkit-user-select: none;
}
header > * { flex: none; }
header > .grow { flex: 1; align-self: stretch; }
header > .state { flex: 0 1 auto; min-width: 0; }
.utc { margin-right: var(--space); }
.brand { display: flex; align-items: center; gap: var(--space); font-weight: 650; }
.mark { display: block; -webkit-user-drag: none; }
.alpha { color: var(--warning); border: 1px solid currentColor; border-radius: 999px; padding: 0 8px; }
.station { display: flex; flex-direction: column; line-height: 1.2; border-left: 1px solid var(--border); padding-left: calc(var(--space) * 2); }
.mode { font-weight: 600; }
.state { display: flex; align-items: center; gap: var(--space); }
.detail { max-width: 36ch; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.banner {
  flex: none; margin: 0; padding: 6px calc(var(--space) * 2); border-bottom: 1px solid var(--border);
  background: var(--raised); display: flex; gap: var(--space); align-items: center; flex-wrap: wrap;
}
.banner.warn { border-left: 3px solid var(--warning); }
.banner.err { border-left: 3px solid var(--danger); }
.banner.info { border-left: 3px solid var(--accent); }
.banner button { padding: 2px 10px; }
.body { flex: 1; display: grid; grid-template-columns: 84px minmax(0, 1fr); min-height: 0; }
.rail { display: flex; flex-direction: column; gap: 6px; padding: var(--space) 6px; border-right: 1px solid var(--border); background: var(--surface); }
.rail button {
  display: grid; justify-items: center; gap: 2px; padding: 8px 2px; font-size: 11px;
  border-color: transparent; background: transparent; color: var(--text-2);
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
@media (max-width: 900px) {
  .body { grid-template-columns: 64px minmax(0, 1fr); }
  .rail button { font-size: 9px; }
  .alpha, .utc { display: none; }
}
</style>
