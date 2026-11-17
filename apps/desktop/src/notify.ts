// The one place that decides whether a core event becomes a desktop
// notification (M3a Task 12b). Pure: no Vue, no i18n, no plugin calls, so it
// is unit tested with plain node (scripts/notify.test.mjs). Every string in a
// Notice is an i18n key with its parameters; the caller translates and sends.
// Later milestones add their kinds here (file offers in M4, QSY in M3c).
import type { AppEvent } from "./bindings/AppEvent";
import type { NotifyPrefs } from "./bindings/NotifyPrefs";

export type NoticeKind =
  | "message" | "incoming" | "callFailed" | "delivery" | "heardFavourite" | "heardAny" | "fault" | "beaconRefused";

export type Text = { key: string; params: Record<string, string | number> };

export type Notice = {
  kind: NoticeKind;
  title: Text;
  /** `null` when there is nothing more to say, or the operator hid message text. */
  body: Text | null;
  /** What a click on it should open (a conversation's callsign, or a page). */
  tag: string;
  /** Shown in quiet hours too. */
  urgent: boolean;
};

export type NotifyContext = {
  /** The window has the focus. */
  focused: boolean;
  /** The conversation open in the window. */
  activePeer: string | null;
  prefs: NotifyPrefs;
  /** The station in use is a Virtual Audio Lab profile. */
  lab: boolean;
  /** Local time, for quiet hours. */
  now: Date;
};

/** The core's defaults (crates/mc-core/src/app/notify.rs), for a window that has no snapshot yet. */
export const DEFAULT_PREFS: NotifyPrefs = {
  enabled: true, message: true, incoming: true, callFailed: true, delivery: true, heardFavourite: true,
  heardAny: false, fault: true, beaconRefused: false, sound: true, hideText: false,
  quietStart: null, quietEnd: null, labEnabled: false,
};

/** Characters of message text a notification shows at most. */
export const TEXT_MAX = 80;
/** Repeats of one kind within this long become one summary notification. */
export const COALESCE_MS = 10_000;

/** A message body as one line of plain text: no control characters, at most TEXT_MAX characters. */
export function plainText(body: string): string {
  // eslint-disable-next-line no-control-regex
  const flat = body.replace(/[\u0000-\u001f\u007f-\u009f]+/g, " ").replace(/\s+/g, " ").trim();
  const chars = [...flat];
  return chars.length <= TEXT_MAX ? flat : chars.slice(0, TEXT_MAX - 1).join("").trimEnd() + "…";
}

const minutes = (hhmm: string) => Number(hhmm.slice(0, 2)) * 60 + Number(hhmm.slice(3, 5));

/** Whether `now` (local time) is inside the quiet hours; a window may run past midnight. */
export function inQuietHours(prefs: NotifyPrefs, now: Date): boolean {
  if (!prefs.quietStart || !prefs.quietEnd) return false;
  const start = minutes(prefs.quietStart), end = minutes(prefs.quietEnd);
  const at = now.getHours() * 60 + now.getMinutes();
  return start <= end ? at >= start && at < end : at >= start || at < end;
}

const text = (key: string, params: Record<string, string | number> = {}): Text => ({ key, params });

/** What `event` should show, before quiet hours and switches are applied. */
function candidate(event: AppEvent, ctx: NotifyContext): Notice | null {
  switch (event.type) {
    case "messageChanged": {
      const m = event.message;
      if (m.direction === "in" && m.state === "received") {
        if (ctx.focused && ctx.activePeer === m.peer) return null;
        return {
          kind: "message",
          title: text("notify.message.title", { callsign: m.peer }),
          body: ctx.prefs.hideText ? null : text("notify.message.body", { text: plainText(m.body) }),
          tag: `message:${m.peer}`,
          urgent: false,
        };
      }
      if (m.direction === "out" && (m.state === "uncertain" || m.state === "failed")) {
        return {
          kind: "delivery",
          title: text(`notify.delivery.${m.state}`, { callsign: m.peer }),
          body: null,
          tag: `message:${m.peer}`,
          urgent: false,
        };
      }
      return null;
    }
    case "session":
      if (!event.session || event.session.direction !== "incoming") return null;
      return { kind: "incoming", title: text("notify.incoming.title", { callsign: event.session.peer }), body: null, tag: `message:${event.session.peer}`, urgent: false };
    case "callFailed":
      return { kind: "callFailed", title: text("notify.callFailed.title", { callsign: event.peer }), body: text("notify.reason", { reason: event.reason }), tag: `message:${event.peer}`, urgent: false };
    case "heard": {
      const s = event.station;
      if (!s.source || s.source === "session") return null;
      const title = text("notify.heard.title", { callsign: s.callsign, source: `heard.source.${s.source}` });
      return { kind: s.favourite ? "heardFavourite" : "heardAny", title, body: null, tag: "page:heard", urgent: false };
    }
    case "modemState": {
      const st = event.state.type;
      if (st !== "faulted" && st !== "recovering") return null;
      return {
        kind: "fault",
        title: text(`notify.fault.${st}`),
        body: event.detail ? text("notify.reason", { reason: event.detail }) : null,
        tag: "page:operate",
        urgent: true,
      };
    }
    case "broadcast":
      if (event.kind !== "beacon" || event.outcome.type !== "refused") return null;
      return { kind: "beaconRefused", title: text("notify.beaconRefused.title"), body: text("notify.reason", { reason: event.outcome.reason }), tag: "page:operate", urgent: false };
    default:
      return null;
  }
}

/** The notification `event` should raise now, or null. */
export function shouldNotify(event: AppEvent, ctx: NotifyContext): Notice | null {
  const p = ctx.prefs;
  if (!p.enabled || (ctx.lab && !p.labEnabled)) return null;
  const n = candidate(event, ctx);
  if (!n || !p[n.kind]) return null;
  if (!n.urgent && inQuietHours(p, ctx.now)) return null;
  return n;
}

/** The summary key for `n` more of a kind within one window. */
const MORE: Record<NoticeKind, string> = {
  message: "notify.message.more",
  incoming: "notify.incoming.more",
  callFailed: "notify.callFailed.more",
  delivery: "notify.delivery.more",
  heardFavourite: "notify.heard.more",
  heardAny: "notify.heard.more",
  fault: "notify.fault.more",
  beaconRefused: "notify.beaconRefused.more",
};

/**
 * Bursts become one notification: the first of a kind is shown at once; more
 * of that kind within COALESCE_MS are held and, when the window ends, shown
 * as one "N more" summary. The two heard kinds share a window.
 */
export class Coalescer {
  private windows = new Map<string, { until: number; held: number; last: Notice }>();

  private static group(kind: NoticeKind): string {
    return kind === "heardAny" || kind === "heardFavourite" ? "heard" : kind;
  }

  /** The notice to show now, or null when it is held for the summary. */
  push(n: Notice | null, now: number): Notice | null {
    if (!n) return null;
    const g = Coalescer.group(n.kind);
    const w = this.windows.get(g);
    if (w && now < w.until) {
      w.held++;
      w.last = n;
      return null;
    }
    this.windows.set(g, { until: now + COALESCE_MS, held: 0, last: n });
    return n;
  }

  /** Summaries for windows that have ended with notices held. */
  flush(now: number): Notice[] {
    const out: Notice[] = [];
    for (const [g, w] of this.windows) {
      if (now < w.until) continue;
      this.windows.delete(g);
      if (w.held > 0) {
        out.push({ ...w.last, title: text(MORE[w.last.kind], { n: w.held }), body: null });
      }
    }
    return out;
  }
}
