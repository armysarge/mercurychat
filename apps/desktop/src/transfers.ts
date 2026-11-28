// File transfers as the UI keeps them (M4): one row per transfer id, from
// the Transfers list and then the Transfer events. Kept pure for node --test.
import type { TransferState } from "./bindings/TransferState";
import type { TransferView } from "./bindings/TransferView";

export interface Transfers {
  byId: Record<string, TransferView>;
  /** Ids, oldest first. */
  order: string[];
}

export function emptyTransfers(): Transfers {
  return { byId: {}, order: [] };
}

const FINAL: readonly TransferState[] = ["complete", "rejected", "cancelled", "failed", "interrupted"];

/** A final state never changes again (G6: it never turns back into "sending"). */
export function isFinal(s: TransferState): boolean {
  return FINAL.includes(s);
}

/** Replace the store with the core's list (newest first, as `Transfers` answers). */
export function loadTransfers(t: Transfers, newestFirst: TransferView[]): void {
  t.byId = {};
  t.order = [];
  for (const v of [...newestFirst].reverse()) {
    t.byId[v.id] = v;
    t.order.push(v.id);
  }
}

/**
 * One event's view of a transfer. False when it changes nothing shown: the
 * transfer already ended, so the update is stale. Progress never goes
 * backwards (a throttled event may arrive after a later one).
 */
export function applyTransfer(t: Transfers, v: TransferView): boolean {
  const old = t.byId[v.id];
  if (!old) {
    t.byId[v.id] = v;
    t.order.push(v.id);
    return true;
  }
  if (isFinal(old.state)) return false;
  const bytesDone = isFinal(v.state) ? v.bytesDone : Math.max(old.bytesDone, v.bytesDone);
  t.byId[v.id] = { ...v, bytesDone };
  return true;
}

/** The Files view's three lists, newest first: incoming and outgoing (each
 *  active or done), then those that were interrupted or failed. */
export function lists(t: Transfers): { incoming: TransferView[]; outgoing: TransferView[]; ended: TransferView[] } {
  const all = [...t.order].reverse().map((id) => t.byId[id]);
  const broke = (v: TransferView) => v.state === "interrupted" || v.state === "failed";
  return {
    incoming: all.filter((v) => !broke(v) && v.direction === "in"),
    outgoing: all.filter((v) => !broke(v) && v.direction === "out"),
    ended: all.filter(broke),
  };
}

/** Airtime in words: seconds under a minute, minutes under two hours, then hours (rounded). */
export function airtime(s: number): { key: string; n: number } {
  if (s < 60) return { key: "files.airtime.seconds", n: s };
  if (s < 7200) return { key: "files.airtime.minutes", n: Math.round(s / 60) };
  return { key: "files.airtime.hours", n: Math.round(s / 3600) };
}

/** "812 B", "4.7 KB", "1.0 MB" (1 KB = 1024 B). */
export function formatSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

/** A name the photo-size choice applies to (the core decides by content). */
export function isPhotoName(name: string): boolean {
  return /\.(jpe?g|png|webp|gif)$/i.test(name);
}

/** The core's reason codes (FILE_REJECT, cancels, link ends) and their words. */
const REASONS: Record<string, string> = {
  declined: "declined",
  timeout: "timeout",
  "too large": "tooLarge",
  "no folder": "noFolder",
  "no space": "noSpace",
  busy: "busy",
  "write failed": "writeFailed",
  "accept failed": "writeFailed",
  user: "user",
  peer: "peer",
  link: "link",
  restart: "restart",
  "not supported by this station": "unsupported",
};

/** The i18n key for a reason code, or null to show the core's own words. */
export function reasonKey(reason: string): string | null {
  const k = REASONS[reason];
  return k ? `files.reason.${k}` : null;
}

/** An incoming offer's airtime at the session's nominal rate (the core's
 *  rates: BW500 19 B/s, else 115 B/s), with 10 % framing, rounded up. */
export function offerAirtimeS(wireSize: number, bandwidth: string): number {
  const rate = /(^|BW)500$/i.test(bandwidth) ? 19 : 115;
  return Math.ceil((wireSize * 1.1) / rate);
}
