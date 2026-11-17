// Desktop notifications (M3a Task 12b): runs each core event through
// notify.ts, coalesces bursts, translates the result and hands it to the OS
// through Tauri's notification plugin. Never asks the OS for permission by
// itself: only when the operator turns a notification on (Settings). The
// sound is the OS default notification sound, never the radio's audio
// devices (02 §9).
//
// The desktop plugin reports no clicks, so a click is inferred: when the
// window next gains the focus within CLICK_MS of a notification it showed
// while unfocused, the window opens that notification's conversation or page.
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import type { AppEvent } from "./bindings/AppEvent";
import { t } from "./i18n";
import { Coalescer, DEFAULT_PREFS, shouldNotify, type Notice, type Text } from "./notify";
import { activeProfile, app, chat, openPeer, snapshot } from "./store";

const CLICK_MS = 60_000;
const coalescer = new Coalescer();
let granted: boolean | null = null;
let pending: { tag: string; at: number } | null = null;
let flushTimer: ReturnType<typeof setInterval> | undefined;

/** The OS's own default notification sound, by platform (Windows toast, macOS, freedesktop). */
function defaultSound(): string {
  const ua = navigator.userAgent;
  if (/Windows/.test(ua)) return "Default";
  if (/Mac OS X|Macintosh/.test(ua)) return "default";
  return "message-new-instant";
}

/** A Text in the window's language; a parameter that is itself a key (heard.source.*) is translated too. */
const say = (x: Text) =>
  t(x.key, Object.fromEntries(Object.entries(x.params).map(([k, v]) => [k, typeof v === "string" && v.startsWith("heard.source.") ? t(v) : v])));

async function show(n: Notice) {
  if (granted === null) granted = await isPermissionGranted().catch(() => false);
  if (!granted) return;
  const prefs = snapshot.value?.notifyPrefs ?? DEFAULT_PREFS;
  sendNotification({
    title: say(n.title),
    body: n.body ? say(n.body) : undefined,
    sound: prefs.sound ? defaultSound() : undefined,
  });
  if (!document.hasFocus()) pending = { tag: n.tag, at: Date.now() };
}

/** One core event, as the mirror applies it. */
export function onEvent(event: AppEvent) {
  const n = shouldNotify(event, {
    focused: document.hasFocus(),
    activePeer: chat.open || null,
    prefs: snapshot.value?.notifyPrefs ?? DEFAULT_PREFS,
    lab: activeProfile.value?.lab ?? false,
    now: new Date(),
  });
  const now = coalescer.push(n, Date.now());
  if (now) void show(now);
}

/** Open what the last notification was about (the inferred click). */
function onFocus() {
  const p = pending;
  pending = null;
  if (!p || Date.now() - p.at > CLICK_MS) return;
  if (p.tag.startsWith("message:")) {
    app.page = "operate";
    void openPeer(p.tag.slice("message:".length));
  } else if (p.tag === "page:heard") {
    app.page = "operate";
    app.sideTab = "heard";
  } else if (p.tag === "page:operate") {
    app.page = "operate";
  }
}

export function startNotifier() {
  window.addEventListener("focus", onFocus);
  flushTimer = setInterval(() => coalescer.flush(Date.now()).forEach((n) => void show(n)), 1000);
}

export function stopNotifier() {
  window.removeEventListener("focus", onFocus);
  clearInterval(flushTimer);
}

/** Ask the OS once, when the operator turns a notification on. True when allowed. */
export async function askPermission(): Promise<boolean> {
  granted = await isPermissionGranted().catch(() => false);
  if (granted) return true;
  granted = (await requestPermission().catch(() => "denied")) === "granted";
  return granted;
}
