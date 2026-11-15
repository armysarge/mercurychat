// The UI's only route to the core (docs/02 §2–§3): typed commands through
// `app_call`, events on `mc://event`, and `events_since` to fill gaps.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AppCommand } from "./bindings/AppCommand";
import type { AppError } from "./bindings/AppError";
import type { AppSnapshot } from "./bindings/AppSnapshot";
import type { CommandOk } from "./bindings/CommandOk";
import type { EventEnvelope } from "./bindings/EventEnvelope";
import type { Since } from "./bindings/Since";

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

/** Operator-readable text for anything a command rejects with. */
export function describeError(e: unknown): string {
  if (e && typeof e === "object" && "code" in e) {
    const err = e as AppError;
    switch (err.code) {
      case "validation":
        return err.errors.map((f) => `${f.field}: ${f.message}`).join("; ");
      case "notFound":
        return `No profile with id ${err.id}.`;
      default:
        return err.message;
    }
  }
  return String(e);
}

export interface View {
  snapshot: AppSnapshot;
  modemDetail: string | null;
  lastFault: string | null;
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

  constructor(private readonly changed: (v: View) => void) {}

  async start(): Promise<void> {
    // Listen first, then snapshot: anything the snapshot already holds is skipped by seq.
    this.unlisten = await onEvent((env) => this.enqueue(() => this.receive(env)));
    await this.enqueue(() => this.resync());
  }

  stop(): void {
    this.unlisten?.();
    this.unlisten = null;
  }

  /** Replace the local copy with a fresh snapshot. */
  resync(): Promise<void> {
    return snapshot().then((s) => {
      this.view = { snapshot: s, modemDetail: null, lastFault: this.view?.lastFault ?? null };
      this.publish();
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
    switch (ev.type) {
      case "modemState":
        v.snapshot.modem = ev.state;
        v.modemDetail = ev.detail;
        break;
      case "activeProfileChanged":
        v.snapshot.activeProfileId = ev.id;
        break;
      case "fault":
        v.lastFault = ev.message;
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
