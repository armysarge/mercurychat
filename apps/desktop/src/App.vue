<script setup lang="ts">
// M1 foundation: station profiles, the simulator modem and its state, all
// through typed commands (api.ts), plus the thin M2 chat test panel. Not yet
// the operator interface (docs/04 section 2).
import { invoke } from "@tauri-apps/api/core";
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { call, describeError, Mirror, type View } from "./api";
import type { ChatMessage } from "./bindings/ChatMessage";
import type { Conversation } from "./bindings/Conversation";
import type { StationProfile } from "./bindings/StationProfile";

interface AppInfo {
  app_version: string;
  protocol: string;
  os: string;
  arch: string;
  data_root: string;
  db_path: string;
  db_schema: number;
  locks_dir: string;
  startup_ms: number;
  startup_error: string | null;
}
interface ProbeResult {
  ok: boolean;
  summary: string;
  lines: string[];
}

const info = ref<AppInfo | null>(null);
const startupError = ref("");
const view = ref<View | null>(null);
const actionError = ref("");
const working = ref(false);
const callsign = ref("");
const host = ref("127.0.0.1");
const port = ref(8300);
const probing = ref(false);
const probe = ref<ProbeResult | null>(null);
const pingMs = ref<number[]>([]);
const now = ref(new Date());
const mirror = new Mirror((v) => (view.value = v), { message: onMessage, resynced: () => void reloadChat() });
let clock: ReturnType<typeof setInterval> | undefined;

const profiles = computed(() => view.value?.snapshot.profiles ?? []);
const activeId = computed(() => view.value?.snapshot.activeProfileId ?? null);
const modemState = computed(() => view.value?.snapshot.modem.type ?? "unavailable");

// ---- M2 chat test panel ----
const MAX_BYTES = 4096; // CHT-01
const peer = ref("SIM1"); // the call box, and the conversation shown
const conversations = ref<Conversation[]>([]);
const messages = ref<ChatMessage[]>([]);
const composer = ref("");
const chatError = ref("");
let firstLoad = true;
let draftTimer: ReturnType<typeof setTimeout> | undefined;
let convTimer: ReturnType<typeof setTimeout> | undefined;

const openPeer = computed(() => peer.value.trim().toUpperCase());
const textBytes = computed(() => new TextEncoder().encode(composer.value).length);
const listening = computed(() => modemState.value === "listening");
const canCall = computed(() => modemState.value === "ready" || listening.value);
const inSession = computed(() => modemState.value === "negotiating" || modemState.value === "connected");
const canAbort = computed(() => inSession.value || modemState.value === "calling" || modemState.value === "draining");
const recovering = computed(() => modemState.value === "recovering" || modemState.value === "faulted");

const STATE_WORDS: Record<string, string> = {
  queued: "queued",
  submitting: "submitting",
  awaiting_receipt: "awaiting receipt",
  delivered: "delivered",
  failed: "failed",
  uncertain: "uncertain",
  received: "received",
};
const stateWord = (s: string) => STATE_WORDS[s] ?? s;
const msgTime = (m: ChatMessage) => (m.receivedUtc ?? m.createdUtc).slice(11, 19) + "Z";

function onMessage(m: ChatMessage) {
  if (m.peer === openPeer.value) {
    const i = messages.value.findIndex((x) => x.id === m.id && x.direction === m.direction);
    if (i >= 0) messages.value[i] = m;
    else {
      messages.value.push(m);
      messages.value.sort((a, b) => a.seq - b.seq);
    }
  }
  clearTimeout(convTimer);
  convTimer = setTimeout(() => void loadConversations().catch(() => {}), 300);
}

async function loadConversations() {
  const r = await call({ type: "conversations" });
  if (r.type === "conversations") conversations.value = r.conversations;
}

/** The open conversation's history and draft. */
async function loadPeer() {
  chatError.value = "";
  messages.value = [];
  if (!activeId.value || !openPeer.value) return;
  try {
    const h = await call({ type: "history", peer: openPeer.value, beforeSeq: null, limit: 200 });
    if (h.type === "history") messages.value = h.messages;
    composer.value = conversations.value.find((c) => c.peer === openPeer.value)?.draft ?? "";
  } catch (e) {
    chatError.value = describeError(e);
  }
}

async function reloadChat() {
  if (!activeId.value) {
    conversations.value = [];
    messages.value = [];
    return;
  }
  try {
    await loadConversations();
    if (firstLoad && conversations.value.length) peer.value = conversations.value[0].peer;
    firstLoad = false;
    await loadPeer();
  } catch (e) {
    chatError.value = describeError(e);
  }
}
watch(activeId, () => void reloadChat());

// The draft is kept (debounced) for the peer it was typed for; never sent.
watch(composer, (text) => {
  const forPeer = openPeer.value;
  clearTimeout(draftTimer);
  draftTimer = setTimeout(() => {
    if (activeId.value && forPeer) call({ type: "saveDraft", peer: forPeer, text }).catch(() => {});
  }, 500);
});

function openConversation(p: string) {
  peer.value = p;
  void loadPeer();
}

async function chat(step: () => Promise<unknown>) {
  chatError.value = "";
  try {
    await step();
  } catch (e) {
    chatError.value = describeError(e);
  }
}

function send() {
  const text = composer.value;
  if (!text.trim() || textBytes.value > MAX_BYTES) return;
  const to = openPeer.value;
  return chat(async () => {
    const r = await call({ type: "sendChat", peer: to, text });
    if (r.type === "message") onMessage(r);
    if (composer.value === text) composer.value = "";
  });
}

const setListen = (on: boolean) => chat(() => call({ type: "setListen", on }));
const placeCall = () => chat(() => call({ type: "call", peer: openPeer.value }).then(() => loadPeer()));
const cancelCall = () => chat(() => call({ type: "cancelCall" }));
const disconnect = () => chat(() => call({ type: "disconnect" }));
const abort = () => chat(() => call({ type: "abort" }));
const retry = (m: ChatMessage) => chat(() => call({ type: "retryChat", peer: m.peer, id: m.id }));

onMounted(async () => {
  clock = setInterval(() => (now.value = new Date()), 1000);
  try {
    info.value = await invoke<AppInfo>("app_info");
    if (info.value.startup_error) {
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
  clearInterval(clock);
  clearTimeout(draftTimer);
  clearTimeout(convTimer);
});

async function act(step: () => Promise<unknown>) {
  working.value = true;
  actionError.value = "";
  try {
    await step();
  } catch (e) {
    actionError.value = describeError(e);
  } finally {
    working.value = false;
  }
}

/** Save a simulator profile for the callsign (or reuse one), then select it. */
function useSimulator() {
  return act(async () => {
    const wanted = callsign.value.trim().toUpperCase();
    let p = profiles.value.find((x) => x.modem.kind === "simulator" && x.callsign === wanted);
    if (!p) {
      const draft: StationProfile = {
        id: "",
        callsign: callsign.value,
        name: null,
        grid: null,
        modem: { kind: "simulator" },
        lab: true,
        listenPreferred: false,
      };
      const saved = await call({ type: "saveProfile", profile: draft });
      if (saved.type !== "profile") throw new Error(`unexpected reply ${saved.type}`);
      p = saved;
    }
    await call({ type: "selectProfile", id: p.id });
  });
}

const select = (id: string | null) => act(() => call({ type: "selectProfile", id }));
const remove = (id: string) => act(() => call({ type: "deleteProfile", id }));

function where(p: StationProfile): string {
  return p.modem.kind === "simulator" ? "simulator" : `${p.modem.host}:${p.modem.controlPort}`;
}

async function runProbe() {
  probing.value = true;
  probe.value = null;
  try {
    probe.value = await invoke<ProbeResult>("modem_probe", { host: host.value, port: Number(port.value) });
  } catch (e) {
    probe.value = { ok: false, summary: describeError(e), lines: [] };
  } finally {
    probing.value = false;
  }
}

// IPC round-trip latency sample (responsiveness measurement for M0/Pi).
async function measureIpc(n = 50) {
  const samples: number[] = [];
  for (let i = 0; i < n; i++) {
    const t = performance.now();
    await invoke("ping");
    samples.push(performance.now() - t);
  }
  samples.sort((a, b) => a - b);
  const q = (p: number) => samples[Math.min(n - 1, Math.floor(p * n))];
  pingMs.value = [q(0.5), q(0.95), samples[n - 1]];
  await invoke("report_ipc", { p50: pingMs.value[0], p95: pingMs.value[1], max: pingMs.value[2], samples: n });
}

const utc = () => now.value.toISOString().slice(11, 19) + "Z";
</script>

<template>
  <div class="shell">
    <header class="top">
      <div class="brand"><span class="mark" aria-hidden="true">M</span> MercuryChat</div>
      <div class="badge warn" role="status">M1 foundation — simulator only, no RF, PTT not controlled</div>
      <div class="utc mono" aria-label="UTC time">{{ utc() }}</div>
    </header>

    <main v-if="startupError" class="grid">
      <section class="card" role="alert" aria-labelledby="h-fail">
        <h2 id="h-fail">MercuryChat cannot start</h2>
        <p class="err">{{ startupError }}</p>
        <p class="muted">Close this window to exit.</p>
      </section>
    </main>

    <main v-else class="grid">
      <section class="card" aria-labelledby="h-station">
        <h2 id="h-station">Station</h2>
        <ul v-if="profiles.length" class="profiles">
          <li v-for="p in profiles" :key="p.id" :class="{ active: p.id === activeId }">
            <span class="mono call">{{ p.callsign }}</span>
            <span class="muted kind">{{ where(p) }}</span>
            <span v-if="p.id === activeId" class="tag ok">in use</span>
            <button v-if="p.id === activeId" :disabled="working" @click="select(null)">Stop using</button>
            <button v-else :disabled="working" @click="select(p.id)">Use</button>
            <button class="quiet" :disabled="working" :aria-label="'Delete ' + p.callsign" @click="remove(p.id)">Delete</button>
          </li>
        </ul>
        <p v-else-if="view" class="muted">No station profile yet.</p>
        <p v-else class="muted">Loading…</p>
        <form class="row" @submit.prevent="useSimulator">
          <label>Callsign <input v-model="callsign" class="mono" maxlength="15" autocomplete="off" /></label>
          <button type="submit" :disabled="working">Use simulator</button>
        </form>
        <p v-if="actionError" class="err" role="alert">{{ actionError }}</p>
      </section>

      <section class="card" aria-labelledby="h-modemstate">
        <h2 id="h-modemstate">Modem</h2>
        <p class="state" role="status" aria-live="polite">
          <span :class="['mono', 'pill', modemState]">{{ modemState }}</span>
          <span v-if="view?.snapshot.modemDetail" class="muted detail">{{ view.snapshot.modemDetail }}</span>
        </p>
        <p v-if="view?.lastFault" class="err">{{ view.lastFault }}</p>
      </section>

      <section v-if="activeId" class="card chat" aria-labelledby="h-chat">
        <h2 id="h-chat">Chat (M2 test panel)</h2>
        <p v-if="recovering" class="banner" role="alert">
          <template v-if="modemState === 'recovering'">Recovering the modem. Listening is off and nothing is resent automatically.</template>
          <template v-else>The modem has a fault: {{ view?.snapshot.modemDetail }}. Select the station again to retry.</template>
        </p>
        <div class="row">
          <label class="check">
            <input
              class="listen"
              type="checkbox"
              :checked="listening"
              :disabled="!canCall"
              @change="setListen(($event.target as HTMLInputElement).checked)"
            />
            Listen
          </label>
          <span v-if="listening" class="tag ok">Incoming calls answered automatically</span>
        </div>
        <form class="row" @submit.prevent="placeCall">
          <label>Callsign <input v-model="peer" class="mono peer" maxlength="15" autocomplete="off" @change="loadPeer()" /></label>
          <button type="submit" :disabled="!canCall">Call</button>
          <button type="button" class="quiet" :disabled="modemState !== 'calling'" @click="cancelCall">Cancel</button>
          <button type="button" class="quiet" :disabled="!inSession" @click="disconnect">Disconnect</button>
          <button type="button" class="danger" :disabled="!canAbort" @click="abort">Abort (delivery uncertain)</button>
        </form>
        <p v-if="view?.snapshot.session" class="muted">
          Session with <span class="mono">{{ view.snapshot.session.peer }}</span>
          ({{ view.snapshot.session.direction }}, {{ view.snapshot.session.peerApp ?? "unknown app" }})
        </p>
        <p v-if="view?.callFailed" class="err" role="alert">Call failed: {{ view.callFailed }}</p>
        <div class="chatgrid">
          <ul class="convs" aria-label="Conversations">
            <li v-for="c in conversations" :key="c.peer" :class="{ active: c.peer === openPeer }">
              <button class="quiet mono" @click="openConversation(c.peer)">{{ c.peer }}</button>
              <span class="muted">{{ c.messages }}<template v-if="c.uncertain"> · {{ c.uncertain }} uncertain</template><template v-if="c.draft"> · draft</template></span>
            </li>
          </ul>
          <div>
            <ol class="transcript" aria-label="Transcript" aria-live="polite">
              <li
                v-for="m in messages"
                :key="m.direction + m.id"
                :data-id="m.id"
                :data-state="m.state"
                :data-direction="m.direction"
                :class="m.direction"
              >
                <div class="meta mono">
                  {{ msgTime(m) }} · {{ m.direction === "out" ? "to" : "from" }} {{ m.peer }} · {{ stateWord(m.state) }}
                  <button v-if="m.state === 'uncertain' || m.state === 'failed'" class="quiet" @click="retry(m)">Retry</button>
                </div>
                <div class="body">{{ m.body }}</div>
              </li>
            </ol>
            <form class="compose" @submit.prevent="send">
              <textarea
                v-model="composer"
                class="composer"
                rows="3"
                :aria-label="'Message to ' + openPeer"
                @keydown.enter.ctrl.prevent="send"
              ></textarea>
              <div class="row">
                <span :class="['mono', textBytes > MAX_BYTES ? 'err' : 'muted']">{{ textBytes }} / {{ MAX_BYTES }} bytes</span>
                <button type="submit" :disabled="!composer.trim() || textBytes > MAX_BYTES">Send (Ctrl+Enter)</button>
              </div>
            </form>
          </div>
        </div>
        <p v-if="chatError" class="err" role="alert">{{ chatError }}</p>
      </section>

      <section class="card" aria-labelledby="h-build">
        <h2 id="h-build">Build and storage</h2>
        <dl v-if="info">
          <dt>App</dt><dd class="mono">{{ info.app_version }}</dd>
          <dt>Protocol</dt><dd class="mono">{{ info.protocol }}</dd>
          <dt>Platform</dt><dd class="mono">{{ info.os }} / {{ info.arch }}</dd>
          <dt>SQLite</dt><dd class="mono">schema v{{ info.db_schema }}</dd>
          <dt>Data</dt><dd class="mono path">{{ info.data_root }}</dd>
          <dt>Locks</dt><dd class="mono path">{{ info.locks_dir }}</dd>
          <dt>Core ready</dt><dd class="mono">{{ info.startup_ms }} ms after launch</dd>
        </dl>
        <p v-else class="muted">Loading…</p>
      </section>

      <section class="card" aria-labelledby="h-modem">
        <h2 id="h-modem">Modem probe (read-only)</h2>
        <p class="muted">Attaches control + data, asks VERSION and BUFFER, reads 3 s of status lines, detaches. Changes no modem setting.</p>
        <form class="row" @submit.prevent="runProbe">
          <label>Host <input v-model="host" class="mono" size="14" /></label>
          <label>Control port <input v-model.number="port" class="mono" type="number" min="1" max="65534" /></label>
          <button type="submit" :disabled="probing">{{ probing ? "Probing…" : "Probe" }}</button>
        </form>
        <div v-if="probe" :class="['result', probe.ok ? 'ok' : 'err']" role="status">{{ probe.summary }}</div>
        <pre v-if="probe?.lines.length" class="mono log">{{ probe.lines.join("\n") }}</pre>
      </section>

      <section class="card" aria-labelledby="h-perf">
        <h2 id="h-perf">Responsiveness</h2>
        <button @click="measureIpc()">Measure IPC round trip (50×)</button>
        <p v-if="pingMs.length" class="mono">p50 {{ pingMs[0].toFixed(2) }} ms · p95 {{ pingMs[1].toFixed(2) }} ms · max {{ pingMs[2].toFixed(2) }} ms</p>
      </section>
    </main>
  </div>
</template>

<style scoped>
.shell { display: flex; flex-direction: column; min-height: 100vh; }
.top {
  display: flex; align-items: center; gap: calc(var(--space) * 2);
  padding: var(--space) calc(var(--space) * 2);
  background: var(--surface); border-bottom: 1px solid var(--border);
}
.brand { font-weight: 650; display: flex; align-items: center; gap: var(--space); }
.mark {
  display: inline-grid; place-items: center; width: 28px; height: 28px;
  border-radius: 8px; background: var(--accent); color: var(--canvas); font-weight: 800;
}
.badge { font-size: 12px; padding: 2px 10px; border-radius: 999px; border: 1px solid currentColor; }
.warn { color: var(--warning); }
.utc { margin-left: auto; color: var(--text-2); }
.grid {
  display: grid; gap: calc(var(--space) * 2); padding: calc(var(--space) * 2);
  grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
}
.card {
  background: var(--surface); border: 1px solid var(--border);
  border-radius: var(--radius); padding: calc(var(--space) * 2);
}
h2 { font-size: 15px; margin: 0 0 var(--space); }
dl { display: grid; grid-template-columns: max-content 1fr; gap: 4px 12px; margin: 0; }
dt { color: var(--text-2); }
dd { margin: 0; }
.path { word-break: break-all; font-size: 12px; }
.muted { color: var(--text-2); margin: 0 0 var(--space); }
.row { display: flex; flex-wrap: wrap; gap: var(--space); align-items: end; }
label { display: flex; flex-direction: column; gap: 2px; color: var(--text-2); font-size: 12px; }
input {
  background: var(--raised); color: var(--text); border: 1px solid var(--border);
  border-radius: 8px; padding: 6px 8px; width: 9em;
}
button {
  background: var(--accent); color: var(--canvas); border: 0; border-radius: 8px;
  padding: 7px 14px; font-weight: 600; cursor: pointer;
}
button:disabled { opacity: 0.6; cursor: default; }
button.quiet { background: transparent; color: var(--text-2); border: 1px solid var(--border); }
.result { margin-top: var(--space); font-weight: 600; }
.ok { color: var(--success); }
.err { color: var(--danger); }
.log {
  background: var(--raised); border-radius: 8px; padding: var(--space);
  max-height: 220px; overflow: auto; font-size: 12px; margin: var(--space) 0 0;
}
.profiles { list-style: none; margin: 0 0 var(--space); padding: 0; display: grid; gap: 6px; }
.profiles li {
  display: flex; flex-wrap: wrap; align-items: center; gap: var(--space);
  padding: 6px 8px; border-radius: 8px; background: var(--raised);
}
.profiles li.active { outline: 1px solid var(--success); }
.call { font-weight: 650; }
.kind, .detail { margin: 0; font-size: 12px; }
.tag { font-size: 12px; }
.state { display: flex; align-items: center; gap: var(--space); margin: 0; }
.pill { padding: 2px 10px; border-radius: 999px; border: 1px solid currentColor; color: var(--text-2); }
.pill.ready, .pill.listening, .pill.connected { color: var(--success); }
.pill.attaching, .pill.calling, .pill.negotiating, .pill.draining, .pill.recovering { color: var(--warning); }
.pill.faulted { color: var(--danger); }
.chat { grid-column: 1 / -1; }
.banner { background: var(--raised); border-left: 3px solid var(--warning); padding: 6px 10px; border-radius: 6px; }
.check { flex-direction: row; align-items: center; gap: 6px; color: var(--text); font-size: 14px; }
.check input { width: auto; }
button.danger { background: var(--danger); }
.chatgrid { display: grid; grid-template-columns: minmax(140px, 1fr) 3fr; gap: calc(var(--space) * 2); margin-top: var(--space); }
.convs { list-style: none; margin: 0; padding: 0; display: grid; gap: 4px; align-content: start; }
.convs li { display: flex; align-items: center; gap: 6px; }
.convs li.active button { outline: 1px solid var(--success); }
.transcript {
  list-style: none; margin: 0 0 var(--space); padding: 0; display: grid; gap: 6px;
  max-height: 360px; overflow: auto;
}
.transcript li { background: var(--raised); border-radius: 8px; padding: 6px 8px; }
.transcript li.out { margin-left: 15%; }
.transcript li.in { margin-right: 15%; }
.transcript .meta { font-size: 12px; color: var(--text-2); display: flex; align-items: center; gap: 6px; }
.transcript .body { white-space: pre-wrap; overflow-wrap: anywhere; }
.compose { display: grid; gap: 6px; }
.composer {
  width: 100%; box-sizing: border-box; background: var(--raised); color: var(--text);
  border: 1px solid var(--border); border-radius: 8px; padding: 6px 8px; font: inherit; resize: vertical;
}
@media (max-width: 820px) {
  .chatgrid { grid-template-columns: 1fr; }
  .badge { display: none; }
}
</style>
