// Shared window state: the core's mirrored snapshot, the open conversation
// and the layout. Views read it; every change to the core goes through a
// typed command (api.ts) and comes back as an event.
import { computed, reactive } from "vue";
import { call, describeError, Mirror, type View } from "./api";
import type { ChatMessage } from "./bindings/ChatMessage";
import type { Conversation } from "./bindings/Conversation";

export const MAX_BYTES = 4096; // CHT-01
export const PAGE = 50; // messages per history page

export type ViewName = "operate" | "conversations" | "settings" | "diagnostics";
export type Drawer = "peers" | "session" | null;

export const app = reactive({
  view: null as View | null,
  page: "operate" as ViewName,
  drawer: null as Drawer,
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
  if (!activeProfile.value) return "No station";
  switch (modemState.value) {
    case "unavailable":
      return "Not attached";
    case "attaching":
      return "Attaching";
    case "ready":
      return snapshot.value?.receiveOnly ? "Receive only" : "Not answering calls";
    case "listening":
      return "Answering calls";
    case "calling":
      return `Calling ${detail.value ?? ""}`.trim();
    case "negotiating":
      return `Negotiating with ${detail.value ?? ""}`.trim();
    case "connected":
      return `Connected to ${session.value?.peer ?? detail.value ?? ""}`.trim();
    case "draining":
      return detail.value === "aborting" ? "Aborting" : "Finishing queued traffic";
    case "recovering":
      return "Recovering";
    case "faulted":
      return "Modem fault";
  }
});

export const canCall = computed(() => modemState.value === "ready" || modemState.value === "listening");
export const inSession = computed(() => modemState.value === "negotiating" || modemState.value === "connected");
export const canAbort = computed(() => inSession.value || ["calling", "draining"].includes(modemState.value));

export const mirror = new Mirror((v) => (app.view = v), {
  message: onMessage,
  resynced: () => void reloadChat(),
});

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
