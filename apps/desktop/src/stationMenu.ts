// The station menu (M5 Task 11c, owner): what can be done with one station,
// opened from its row in Heard or Chats (right-click, the menu key, or the
// row's "…") and from the conversation header's "…". One menu for the whole
// window, mounted once (App.vue), like the edit menu.
import { reactive } from "vue";
import { describeError } from "./api";
import { exportConversation } from "./exports";
import { app, chat, closePeer, openPeer, placeCall, setFavourite } from "./store";

/** Where the menu was opened: a list row, or the open conversation's header. */
export type MenuFrom = "heard" | "chats" | "header";

export const stationMenu = reactive({
  open: false,
  x: 0,
  y: 0,
  callsign: "",
  from: "heard" as MenuFrom,
  /** Focus goes back here when the menu closes. */
  opener: null as HTMLElement | null,
});

/** The station a QSY suggestion is being written to (the Heard list shows the form), or "". */
export const qsyTo = reactive({ callsign: "" });

/** The last export's result, shown briefly by the menu (StationMenu.vue). */
export const menuNote = reactive({ text: "", failed: false });

/** Open the menu at a right-click (or the menu key) on a station. */
export function openAt(e: MouseEvent, callsign: string, from: MenuFrom) {
  e.preventDefault();
  e.stopPropagation();
  const opener = e.currentTarget instanceof HTMLElement ? e.currentTarget : null;
  // The menu key fires at the focused element with no pointer position.
  const r = opener?.getBoundingClientRect();
  const keyboard = e.clientX === 0 && e.clientY === 0 && r;
  Object.assign(stationMenu, {
    open: true,
    x: keyboard ? r.left + 16 : e.clientX,
    y: keyboard ? r.top + r.height / 2 : e.clientY,
    callsign,
    from,
    opener,
  });
}

/** Open the menu under a "…" button. */
export function openFrom(button: HTMLElement, callsign: string, from: MenuFrom) {
  if (stationMenu.open && stationMenu.opener === button) return closeMenu();
  const r = button.getBoundingClientRect();
  Object.assign(stationMenu, { open: true, x: r.left, y: r.bottom + 4, callsign, from, opener: button });
}

export function closeMenu(refocus = true) {
  if (!stationMenu.open) return;
  stationMenu.open = false;
  if (refocus) stationMenu.opener?.focus();
}

/** Show `peer`'s conversation on the Operate page (and close a drawer). */
export async function openChat(peer: string) {
  await openPeer(peer);
  if (app.page !== "operate" && app.page !== "conversations") app.page = "operate";
  app.drawer = null;
}

export async function callStation(peer: string) {
  await openChat(peer);
  await placeCall();
}

/** The QSY form for `peer`, in the Heard list (opened as a drawer when narrow). */
export function suggestQsy(peer: string) {
  qsyTo.callsign = peer;
  app.sideTab = "heard";
  if (window.matchMedia("(max-width: 960px)").matches) app.drawer = "peers";
}

export const star = (peer: string, on: boolean) => setFavourite(peer, on);

export async function exportChat(peer: string, done: (n: number, peer: string) => string) {
  menuNote.text = "";
  menuNote.failed = false;
  try {
    const n = await exportConversation(peer);
    if (n !== null) menuNote.text = done(n, peer);
  } catch (e) {
    menuNote.failed = true;
    menuNote.text = describeError(e);
  }
}

export const closeChat = () => closePeer();

/** `peer` has a conversation that can be exported. */
export const hasHistory = (peer: string) => chat.open === peer || chat.conversations.some((c) => c.peer === peer && c.messages > 0);
