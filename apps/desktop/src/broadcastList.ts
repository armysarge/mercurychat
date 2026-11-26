// The latest general broadcasts (owner, 26 Nov): keeping the list, and
// wording its dates and S/N. Pure, so it is tested without the app.

import type { HeardBroadcast } from "./bindings/HeardBroadcast";
import type { Reading } from "./bindings/Reading";

/** The list shows this many, as the core sends them. */
export const LATEST_MAX = 10;
const UNKNOWN = "—";

/**
 * `list` with `b` in it: one row per id, newest first, at most [`LATEST_MAX`].
 * A copy without an S/N never takes one away: the S/N arrives a moment after
 * the broadcast, and a re-read of the list can answer in between.
 */
export function upsertLatest(list: HeardBroadcast[], b: HeardBroadcast): HeardBroadcast[] {
  const had = list.find((x) => x.id === b.id);
  const next = had?.snr && !b.snr ? { ...b, snr: had.snr, mode: had.mode } : b;
  return [...list.filter((x) => x.id !== b.id), next]
    .sort((x, y) => y.atUtc.localeCompare(x.atUtc) || y.id - x.id)
    .slice(0, LATEST_MAX);
}

/** The parts of a UTC time as this app words them, or null if it is not one. */
function parts(iso: string, locale: string) {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return null;
  const month = new Intl.DateTimeFormat(locale, { month: "short", timeZone: "UTC" }).format(d);
  return { day: `${d.getUTCDate()} ${month}`, year: d.getUTCFullYear(), time: d.toISOString().slice(11, 19) };
}

/** For a row: `26 Nov 14:32`. */
export function shortUtc(iso: string, locale: string): string {
  const p = parts(iso, locale);
  return p ? `${p.day} ${p.time.slice(0, 5)}` : UNKNOWN;
}

/** For the details: `26 Nov 2026, 14:32:08 UTC`. */
export function longUtc(iso: string, locale: string): string {
  const p = parts(iso, locale);
  return p ? `${p.day} ${p.year}, ${p.time} UTC` : UNKNOWN;
}

/** `+7.5`, `−2.0` (a true minus), or a dash when there is none. */
export function snrText(r: Reading<number> | null): string {
  if (!r) return UNKNOWN;
  return (r.value < 0 ? "−" : "+") + Math.abs(r.value).toFixed(1);
}
