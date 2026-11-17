// Shared window state: the core's mirrored snapshot, the open conversation
// and the layout. Views read it; every change to the core goes through a
// typed command (api.ts) and comes back as an event.
import { computed, reactive, watch } from "vue";
import { call, describeError, Mirror, type View } from "./api";
import type { ActivityEntry } from "./bindings/ActivityEntry";
import type { BroadcastKind } from "./bindings/BroadcastKind";
import type { BroadcastOutcome } from "./bindings/BroadcastOutcome";
import type { ChatMessage } from "./bindings/ChatMessage";
import type { Conversation } from "./bindings/Conversation";
import type { HeardStation } from "./bindings/HeardStation";
import type { MacroDef } from "./bindings/MacroDef";
import type { Qso } from "./bindings/Qso";
import type { QsoEdit } from "./bindings/QsoEdit";
import { applyLanguage, rememberLanguage, t } from "./i18n";

export const MAX_BYTES = 4096; // CHT-01
export const PAGE = 50; // messages per history page

export type ViewName = "operate" | "conversations" | "logbook" | "settings" | "diagnostics";
export type Drawer = "peers" | "session" | null;

export const app = reactive({
  view: null as View | null,
  page: "operate" as ViewName,
  drawer: null as Drawer,
  /** The sidebar tab: the conversation list or the heard stations. */
  sideTab: "chats" as "chats" | "heard",
});

export const chat = reactive({
  conversations: [] as Conversation[],
  /** The peer whose conversation is open ("" when none). */
  open: "",
  messages: [] as ChatMessage[],
  hasOlder: false,
  draft: "",
  unread: {} as Record<string, number>,
  error: "",
});

export const snapshot = computed(() => app.view?.snapshot ?? null);
export const modemState = computed(() => snapshot.value?.modem.type ?? "unavailable");
export const detail = computed(() => snapshot.value?.modemDetail ?? null);
export const activeProfile = computed(() => {
  const s = snapshot.value;
  return s?.profiles.find((p) => p.id === s.activeProfileId) ?? null;
});
export const session = computed(() => snapshot.value?.session ?? null);

// The language chosen in Settings is kept by the core: show it as soon as the
// snapshot says which it is, and whenever it changes.
watch(
  () => snapshot.value?.uiLanguage,
  (language) => {
    if (language) {
      applyLanguage(language);
      rememberLanguage(language);
    }
  },
  { immediate: true },
);

/** The peer a call or session in progress is with, if any. */
export const linkPeer = computed(() => {
  switch (modemState.value) {
    case "calling":
    case "negotiating":
      return detail.value;
    case "connected":
    case "draining":
      return session.value?.peer ?? null;
    default:
      return null;
  }
});

/** The header's mode, in words (docs/04 §2: never confuse app-online with reachable). */
export const modeWord = computed(() => {
  if (!activeProfile.value) return t("mode.noStation");
  const peer = detail.value ?? "";
  switch (modemState.value) {
    case "unavailable":
      return t("mode.notAttached");
    case "attaching":
      return t("mode.attaching");
    case "ready":
      return snapshot.value?.receiveOnly ? t("mode.receiveOnly") : t("mode.notAnswering");
    case "listening":
      return t("mode.answering");
    case "calling":
      return t("mode.calling", { peer }).trim();
    case "negotiating":
      return t("mode.negotiating", { peer }).trim();
    case "connected":
      return t("mode.connected", { peer: session.value?.peer ?? peer }).trim();
    case "draining":
      return detail.value === "aborting" ? t("mode.aborting") : t("mode.finishing");
    case "recovering":
      return t("mode.recovering");
    case "faulted":
      return t("mode.fault");
  }
});

export const canCall = computed(() => modemState.value === "ready" || modemState.value === "listening");
export const inSession = computed(() => modemState.value === "negotiating" || modemState.value === "connected");
export const canAbort = computed(() => inSession.value || ["calling", "draining"].includes(modemState.value));

export const mirror = new Mirror((v) => (app.view = v), {
  message: onMessage,
  heard: onHeard,
  activity: (e) => {
    discovery.activity.unshift(e);
    if (discovery.activity.length > ACTIVITY_MAX) discovery.activity.length = ACTIVITY_MAX;
  },
  broadcast: (kind, outcome) => (discovery.outcome[kind] = outcome),
  qso: upsertQso,
  resynced: () => {
    void reloadChat();
    void reloadDiscovery();
    void reloadLogbook();
  },
});

// ---- discovery: the heard list, the activity log, CQ and beacon outcomes (M3a Task 10)

/** Lines the activity drawer keeps (the core's ring holds more; it is re-read on resync). */
const ACTIVITY_MAX = 500;
/** How long a newly heard row stays highlighted (04 §2, owner request: about 3 s). */
export const HIGHLIGHT_MS = 3000;

export const discovery = reactive({
  /** The heard list, newest first; favourites with nothing heard sort last. */
  heard: [] as HeardStation[],
  /** The activity log, newest first. */
  activity: [] as ActivityEntry[],
  /** The latest outcome of each kind of broadcast this run. */
  outcome: {} as Partial<Record<BroadcastKind, BroadcastOutcome>>,
  /** Stations heard since the Heard tab was last shown. */
  unseen: 0,
  /** Rows heard just now, by callsign: the time they were heard (for the highlight). */
  fresh: {} as Record<string, number>,
  /** The latest polite announcement: "{callsign} heard: {Beacon|CQ}". */
  announce: "",
});

/** Newest heard first; never-heard favourites last, by callsign. */
function heardOrder(a: HeardStation, b: HeardStation): number {
  if (a.lastHeardUtc && b.lastHeardUtc) return b.lastHeardUtc.localeCompare(a.lastHeardUtc);
  if (a.lastHeardUtc || b.lastHeardUtc) return a.lastHeardUtc ? -1 : 1;
  return a.callsign.localeCompare(b.callsign);
}

/** A heard-list row from the core: new, heard again, or changed (SNR, star). */
function onHeard(st: HeardStation) {
  const i = discovery.heard.findIndex((h) => h.callsign === st.callsign);
  const before = i >= 0 ? discovery.heard[i] : null;
  // A new observation is a newer last-heard time; a star or a completed SNR is not.
  const heardAgain = !!st.lastHeardUtc && st.lastHeardUtc !== before?.lastHeardUtc;
  if (i >= 0) discovery.heard.splice(i, 1, st);
  else discovery.heard.push(st);
  discovery.heard.sort(heardOrder);
  if (!heardAgain || !st.source) return;
  discovery.fresh[st.callsign] = Date.now();
  setTimeout(() => {
    if (Date.now() - (discovery.fresh[st.callsign] ?? 0) >= HIGHLIGHT_MS) delete discovery.fresh[st.callsign];
  }, HIGHLIGHT_MS + 50);
  if (app.sideTab !== "heard") discovery.unseen++;
  if (st.source !== "session") discovery.announce = t("heard.announce", { callsign: st.callsign, source: t(`heard.source.${st.source}`) });
}

/** Re-read the heard list and the activity log (start-up, resync, a new station). */
export async function reloadDiscovery() {
  if (!activeProfile.value) {
    discovery.heard = [];
    discovery.activity = [];
    return;
  }
  try {
    const [h, a] = await Promise.all([call({ type: "heard" }), call({ type: "activity" })]);
    if (h.type === "heard") discovery.heard = [...h.stations].sort(heardOrder);
    if (a.type === "activity") discovery.activity = [...a.entries].reverse().slice(0, ACTIVITY_MAX);
  } catch {
    // Shown as an empty list; the next event or resync fills it.
  }
}

// Opening the Heard tab clears its count.
watch(
  () => app.sideTab,
  (tab) => {
    if (tab === "heard") discovery.unseen = 0;
  },
);

/** A discovery command's refusal, shown inline beside the control that sent it. */
export const discoveryError = reactive({ cq: "", beacon: "", star: "" });

async function discoveryAct(slot: keyof typeof discoveryError, step: () => Promise<unknown>) {
  discoveryError[slot] = "";
  try {
    await step();
  } catch (e) {
    discoveryError[slot] = describeError(e);
  }
}

export const sendCq = () => discoveryAct("cq", () => call({ type: "sendCq" }));
export const sendBeaconNow = () => discoveryAct("beacon", () => call({ type: "sendBeaconNow" }));
export const setBeacon = (on: boolean) => discoveryAct("beacon", () => call({ type: "setBeacon", on }));
export const setBeaconInterval = (minutes: number) => discoveryAct("beacon", () => call({ type: "setBeaconInterval", minutes }));
export const setBeaconContent = (includeGrid: boolean) =>
  discoveryAct("beacon", async () => {
    await call({ type: "setBeaconContent", includeGrid });
    // No event carries this setting: keep the local copy in step with the core.
    if (app.view) app.view.snapshot.beacon = { ...app.view.snapshot.beacon, includeGrid };
  });
export const setFavourite = (callsign: string, on: boolean) => discoveryAct("star", () => call({ type: "setFavourite", callsign, on }));
export const selectProfile = (id: string) => act(() => call({ type: "selectProfile", id }));

let convTimer: ReturnType<typeof setTimeout> | undefined;

function upsert(m: ChatMessage) {
  const i = chat.messages.findIndex((x) => x.id === m.id && x.direction === m.direction);
  if (i >= 0) chat.messages[i] = m;
  else {
    chat.messages.push(m);
    chat.messages.sort((a, b) => a.seq - b.seq);
  }
}

/** A message changed or arrived. Never moves focus or switches conversations. */
function onMessage(m: ChatMessage) {
  if (m.peer === chat.open) upsert(m);
  else if (m.direction === "in" && m.state === "received") chat.unread[m.peer] = (chat.unread[m.peer] ?? 0) + 1;
  clearTimeout(convTimer);
  convTimer = setTimeout(() => void loadConversations().catch(() => {}), 300);
}

export async function loadConversations() {
  const r = await call({ type: "conversations" });
  if (r.type === "conversations") chat.conversations = r.conversations;
}

/** Open `peer`'s conversation: the latest page of history and its draft. */
export async function openPeer(peer: string) {
  const p = peer.trim().toUpperCase();
  // The draft typed for the conversation being left is saved now, and the new
  // one is shown at once: no window where one peer's text sits under another.
  if (chat.open && chat.open !== p) flushDraft(chat.open);
  chat.error = "";
  chat.open = p;
  chat.draft = draftFor(p);
  chat.messages = [];
  chat.hasOlder = false;
  delete chat.unread[p];
  if (!p || !activeProfile.value) return;
  try {
    const h = await call({ type: "history", peer: p, beforeSeq: null, limit: PAGE });
    if (h.type === "history" && chat.open === p) {
      chat.messages = h.messages;
      chat.hasOlder = h.messages.length === PAGE;
    }
  } catch (e) {
    chat.error = describeError(e);
  }
}

export async function loadOlder() {
  const first = chat.messages[0];
  if (!first || !chat.open) return;
  const p = chat.open;
  try {
    const h = await call({ type: "history", peer: p, beforeSeq: first.seq, limit: PAGE });
    if (h.type === "history" && chat.open === p) {
      chat.messages = [...h.messages, ...chat.messages];
      chat.hasOlder = h.messages.length === PAGE;
    }
  } catch (e) {
    chat.error = describeError(e);
  }
}

let firstLoad = true;

/** After start-up, a resync or a new station: reload the list and the open conversation. */
export async function reloadChat() {
  if (!activeProfile.value) {
    chat.conversations = [];
    chat.messages = [];
    chat.open = "";
    return;
  }
  try {
    await loadConversations();
    let peer = chat.open;
    if (firstLoad || !peer) peer = session.value?.peer ?? linkPeer.value ?? chat.conversations[0]?.peer ?? "";
    firstLoad = false;
    await openPeer(peer);
  } catch (e) {
    chat.error = describeError(e);
  }
}

/** Run a command; its error is shown in the chat area. */
export async function act(step: () => Promise<unknown>) {
  chat.error = "";
  try {
    await step();
  } catch (e) {
    chat.error = describeError(e);
  }
}

export function send(text: string) {
  const to = chat.open;
  // The message itself arrives as MessageChanged events, which may overtake
  // the reply: never apply the reply's "queued" over a newer state.
  return act(() => call({ type: "sendChat", peer: to, text }));
}

/** Drafts this window has typed, by station and peer: newer than the list. */
const drafts = new Map<string, string>();
/** Draft saves waiting out the typing delay, by station and peer. */
const draftTimers = new Map<string, { timer: ReturnType<typeof setTimeout>; profile: string; peer: string; text: string }>();
const draftKey = (peer: string) => `${activeProfile.value?.id ?? ""}/${peer}`;

/** The draft to show for `peer`: what was typed here, else what the store holds. */
function draftFor(peer: string): string {
  return drafts.get(draftKey(peer)) ?? chat.conversations.find((c) => c.peer === peer)?.draft ?? "";
}

/** Save a pending draft for `peer` now instead of after the typing delay. */
function flushDraft(peer: string) {
  const key = draftKey(peer);
  const pending = draftTimers.get(key);
  if (!pending) return;
  clearTimeout(pending.timer);
  draftTimers.delete(key);
  storeDraft(pending);
}

/** SaveDraft goes to the active station: never send one typed for another. */
function storeDraft(d: { profile: string; peer: string; text: string }) {
  if (activeProfile.value?.id === d.profile) call({ type: "saveDraft", peer: d.peer, text: d.text }).catch(() => {});
}

/** Keep the draft for the peer it was typed for (debounced per peer); never
 *  sent. Showing a peer's own draft again saves nothing. */
export function saveDraft(text: string) {
  const forPeer = chat.open;
  if (!activeProfile.value || !forPeer) return;
  const key = draftKey(forPeer);
  if (draftFor(forPeer) === text && !draftTimers.has(key)) return;
  drafts.set(key, text);
  const conv = chat.conversations.find((c) => c.peer === forPeer);
  if (conv) conv.draft = text || null;
  clearTimeout(draftTimers.get(key)?.timer);
  const pending = { profile: activeProfile.value.id, peer: forPeer, text };
  draftTimers.set(key, {
    ...pending,
    timer: setTimeout(() => {
      draftTimers.delete(key);
      storeDraft(pending);
    }, 500),
  });
}

export const retry = (m: ChatMessage) => act(() => call({ type: "retryChat", peer: m.peer, id: m.id }));
export const setReceiveOnly = (on: boolean) => act(() => call({ type: "setReceiveOnly", on }));
export const placeCall = () => act(() => call({ type: "call", peer: chat.open }));
export const cancelCall = () => act(() => call({ type: "cancelCall" }));
export const disconnect = () => act(() => call({ type: "disconnect" }));
export const abort = () => act(() => call({ type: "abort" }));

/** Bytes waiting to go to the open peer (queued, submitting, awaiting receipt). */
export const queuedBytes = computed(() =>
  chat.messages
    .filter((m) => m.direction === "out" && ["queued", "submitting", "awaiting_receipt"].includes(m.state))
    .reduce((n, m) => n + new TextEncoder().encode(m.body).length, 0),
);

export function stopTimers() {
  clearTimeout(convTimer);
  for (const peer of [...draftTimers.values()].map((d) => d.peer)) flushDraft(peer);
}

/** The modem's state as a word in the current language (the pill and the Link panel). */
export const modemLabel = computed(() => t(`modem.${modemState.value}`));

// ---- the logbook, macros and history search (M3a Task 11)

/** Records the Logbook reads (the core keeps at most 500 newest). */
const LOGBOOK_MAX = 500;
/** CQs and beacons transmitted, oldest first, from the core's activity log (it
 *  outlives a window reload): the transcript's inline rows (docs/04 §8.1). The
 *  core pins this wording (broadcast.rs, transmitted_lines_keep_the_wording_the_transcript_reads). */
export const transmitted = computed(() =>
  discovery.activity
    .filter((e) => e.kind === "broadcast" && /^(cq|beacon) transmitted/i.test(e.text))
    .map((e) => ({ kind: (e.text.toLowerCase().startsWith("cq") ? "cq" : "beacon") as BroadcastKind, utc: e.atUtc }))
    .reverse(),
);

export const logbook = reactive({ qsos: [] as Qso[], error: "" });

function upsertQso(q: Qso) {
  const i = logbook.qsos.findIndex((x) => x.id === q.id);
  if (i >= 0) logbook.qsos.splice(i, 1, q);
  else logbook.qsos.unshift(q);
}

export async function reloadLogbook() {
  if (!activeProfile.value) {
    logbook.qsos = [];
    return;
  }
  try {
    const r = await call({ type: "qsos", limit: LOGBOOK_MAX });
    if (r.type === "qsos") logbook.qsos = r.qsos;
  } catch (e) {
    logbook.error = describeError(e);
  }
}

/** Save a draft's edits; the core answers with the record (and a qso event). */
export async function updateQso(id: string, edit: QsoEdit): Promise<Qso | null> {
  const r = await call({ type: "updateQso", id, edit });
  if (r.type !== "qso") return null;
  upsertQso(r);
  return r;
}

export async function finaliseQso(id: string): Promise<Qso | null> {
  const r = await call({ type: "finaliseQso", id });
  if (r.type !== "qso") return null;
  upsertQso(r);
  return r;
}

export async function setFinaliseOnDisconnect(on: boolean) {
  await call({ type: "setFinaliseOnDisconnect", on });
  // No event carries this setting: keep the local copy in step with the core.
  if (app.view) app.view.snapshot.finaliseOnDisconnect = on;
}

export const macros = reactive({ list: [] as MacroDef[], error: "" });

export async function loadMacros() {
  macros.error = "";
  if (!activeProfile.value) {
    macros.list = [];
    return;
  }
  try {
    const r = await call({ type: "macros" });
    if (r.type === "macros") macros.list = r.macros;
  } catch (e) {
    macros.error = describeError(e);
  }
}

/** Add (empty id) or change a macro; the error is the core's, field by field. */
export async function saveMacro(def: MacroDef) {
  await call({ type: "saveMacro", macro: def });
  await loadMacros();
}

export async function deleteMacro(id: string) {
  await call({ type: "deleteMacro", id });
  await loadMacros();
}

/** A macro's text for the draft, with the tags that had no value. Nothing is sent. */
export async function expandMacro(id: string): Promise<{ text: string; missing: string[] }> {
  const r = await call({ type: "expandMacro", id, peer: chat.open || null, allowEmail: false });
  if (r.type !== "expanded") throw new Error(r.type);
  return { text: r.text, missing: r.missing };
}

/** Messages to or from anyone whose text or callsign holds `query`, newest first. */
export async function searchHistory(query: string): Promise<ChatMessage[]> {
  const r = await call({ type: "searchHistory", query, limit: 20 });
  return r.type === "history" ? r.messages : [];
}

watch(
  () => activeProfile.value?.id,
  () => {
    void reloadLogbook();
    void loadMacros();
  },
);
