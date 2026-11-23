// The UI's only route to the core (docs/02 §2–§3): typed commands through
// `app_call`, events on `mc://event`, and `events_since` to fill gaps.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ActivityEntry } from "./bindings/ActivityEntry";
import type { AppCommand } from "./bindings/AppCommand";
import type { AppEvent } from "./bindings/AppEvent";
import type { BroadcastKind } from "./bindings/BroadcastKind";
import type { BroadcastOutcome } from "./bindings/BroadcastOutcome";
import type { HeardStation } from "./bindings/HeardStation";
import type { Qso } from "./bindings/Qso";
import type { AppError } from "./bindings/AppError";
import type { AppSnapshot } from "./bindings/AppSnapshot";
import type { ChatMessage } from "./bindings/ChatMessage";
import type { CommandOk } from "./bindings/CommandOk";
import type { EventEnvelope } from "./bindings/EventEnvelope";
import type { Since } from "./bindings/Since";
import type { TransferView } from "./bindings/TransferView";
import { t, te } from "./i18n";

export const EVENT_CHANNEL = "mc://event";

/** Run one command. Rejects with an `AppError`. */
export function call(cmd: AppCommand): Promise<CommandOk> {
  return invoke<CommandOk>("app_call", { cmd });
}

export function eventsSince(seq: number): Promise<Since> {
  return invoke<Since>("events_since", { seq });
}

export function onEvent(handler: (env: EventEnvelope) => void): Promise<UnlistenFn> {
  return listen<EventEnvelope>(EVENT_CHANNEL, (e) => handler(e.payload));
}

export async function snapshot(): Promise<AppSnapshot> {
  const r = await call({ type: "snapshot" });
  if (r.type !== "snapshot") throw new Error(`expected a snapshot, got ${r.type}`);
  const { type: _type, ...snap } = r;
  return snap;
}

/**
 * Operator-readable text for anything a command rejects with: the translation
 * of the error's stable `key` when the UI has one, else the core's English
 * message. Field errors (validation) carry the core's English text only.
 */
export function describeError(e: unknown): string {
  if (e && typeof e === "object" && "code" in e) {
    const err = e as AppError;
    if (err.code === "validation" && err.errors.length) return err.errors.map((f) => `${f.field}: ${f.message}`).join("; ");
    if (te(err.key)) {
      return err.code === "notFound" ? t(err.key, { id: err.id }) : t(err.key, { message: "message" in err ? err.message : "" });
    }
    return "message" in err ? err.message : String(err.code);
  }
  return String(e);
}

export interface View {
  snapshot: AppSnapshot;
  lastFault: string | null;
  /** The last call that did not lead to a session, as "PEER: reason". */
  callFailed: string | null;
}

export interface MirrorHooks {
  /** A message was queued, changed state, or arrived. */
  message?: (m: ChatMessage) => void;
  /** A heard-list row, new or changed (heard again, its SNR completed, starred). */
  heard?: (station: HeardStation) => void;
  /** A line for the activity log. */
  activity?: (entry: ActivityEntry) => void;
  /** What became of a CQ or a beacon. */
  broadcast?: (kind: BroadcastKind, outcome: BroadcastOutcome) => void;
  /** A QSO record was created, edited or finalised. */
  qso?: (qso: Qso) => void;
  /** A file transfer changed state or made progress (M4). */
  transfer?: (t: TransferView) => void;
  /** Every event, after it is applied (desktop notifications). */
  event?: (event: AppEvent) => void;
  /** The local copy was rebuilt from a snapshot: message events may be missing. */
  resynced?: () => void;
}

/**
 * A local copy of the core's state, kept current from the event stream.
 * Events are applied one at a time in order. A gap in `seq` is filled from
 * `events_since`; a new generation or `snapshotNeeded` means a fresh snapshot.
 */
export class Mirror {
  private view: View | null = null;
  private queue: Promise<void> = Promise.resolve();
  private unlisten: UnlistenFn | null = null;

  constructor(
    private readonly changed: (v: View) => void,
    private readonly hooks: MirrorHooks = {},
  ) {}

  async start(): Promise<void> {
    // Listen first, then snapshot: anything the snapshot already holds is skipped by seq.
    this.unlisten = await onEvent((env) => this.enqueue(() => this.receive(env)));
    await this.enqueue(() => this.resync());
  }

  /** The core has stored a new interface language: show it (no event announces it). */
  setUiLanguage(language: string): void {
    if (!this.view) return;
    this.view.snapshot.uiLanguage = language;
    this.publish();
  }

  /** A setting the core keeps without announcing it by an event: show it in
   *  the local copy, so the next event does not put the old value back. */
  patch(change: Partial<AppSnapshot>): void {
    if (!this.view) return;
    Object.assign(this.view.snapshot, change);
    this.publish();
  }

  stop(): void {
    this.unlisten?.();
    this.unlisten = null;
  }

  /** Replace the local copy with a fresh snapshot. */
  resync(): Promise<void> {
    return snapshot().then((s) => {
      this.view = { snapshot: s, lastFault: this.view?.lastFault ?? null, callFailed: this.view?.callFailed ?? null };
      this.publish();
      this.hooks.resynced?.();
    });
  }

  private enqueue(step: () => Promise<void>): Promise<void> {
    this.queue = this.queue.then(step).catch((e) => console.error("event mirror:", describeError(e)));
    return this.queue;
  }

  private publish(): void {
    if (this.view) this.changed({ ...this.view, snapshot: { ...this.view.snapshot } });
  }

  private async receive(env: EventEnvelope): Promise<void> {
    const v = this.view;
    if (!v || env.seq <= v.snapshot.seq) return; // not started yet, or already held
    if (env.generation !== v.snapshot.generation) return this.resync();
    let batch = [env];
    if (env.seq !== v.snapshot.seq + 1) {
      const since = await eventsSince(v.snapshot.seq);
      if (since.type === "snapshotNeeded" || since.events.some((e) => e.generation !== v.snapshot.generation)) {
        return this.resync();
      }
      batch = since.events;
    }
    for (const e of batch) {
      if (e.seq > this.view!.snapshot.seq) await this.apply(e);
    }
    this.publish();
  }

  private async apply(env: EventEnvelope): Promise<void> {
    const v = this.view!;
    const ev = env.event;
    this.hooks.event?.(ev);
    switch (ev.type) {
      case "modemState":
        v.snapshot.modem = ev.state;
        v.snapshot.modemDetail = ev.detail;
        break;
      case "builtIn":
        v.snapshot.builtinDetail = ev.detail;
        break;
      case "activeProfileChanged":
        v.snapshot.activeProfileId = ev.id;
        break;
      case "receiveOnly":
        v.snapshot.receiveOnly = ev.on;
        v.snapshot.receiveOnlyPending = ev.pending;
        break;
      case "fault":
        v.lastFault = ev.message;
        break;
      case "session":
        v.snapshot.session = ev.session;
        if (ev.session) v.callFailed = null;
        break;
      case "callFailed":
        v.callFailed = `${ev.peer}: ${ev.reason}`;
        break;
      case "messageChanged":
        this.hooks.message?.(ev.message);
        break;
      case "beacon":
        v.snapshot.beacon = ev.status;
        break;
      case "telemetry":
        v.snapshot.telemetry = ev.telemetry;
        break;
      case "rig":
        v.snapshot.rig = ev.rig;
        break;
      case "tune":
        v.snapshot.tune = ev.tune;
        break;
      case "scheduleNext":
        v.snapshot.scheduleNext = ev.next;
        break;
      case "qsy":
        v.snapshot.qsy = ev.qsy;
        break;
      case "qso":
        // The card follows the latest session's record; an edit of an older one (Logbook) leaves it.
        if (!v.snapshot.currentQso || v.snapshot.currentQso.id === ev.qso.id || ev.qso.startUtc >= v.snapshot.currentQso.startUtc) {
          v.snapshot.currentQso = ev.qso;
        }
        this.hooks.qso?.(ev.qso);
        break;
      case "heard":
      case "heardUpdated":
        this.hooks.heard?.(ev.station);
        break;
      case "activity":
        this.hooks.activity?.(ev.entry);
        break;
      case "broadcast":
        this.hooks.broadcast?.(ev.kind, ev.outcome);
        break;
      case "transfer":
        this.hooks.transfer?.(ev.transfer);
        break;
      case "profilesChanged": {
        const fresh = await snapshot();
        v.snapshot = fresh;
        return; // the snapshot is at least as new as this event
      }
    }
    v.snapshot.seq = env.seq;
  }
}
