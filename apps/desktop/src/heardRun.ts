// The Heard list per app run (owner, 24 Nov): it starts empty at every start
// and fills as stations are heard. Nothing is deleted: what was heard before
// stays on record, so the rest of the app can still say when a callsign was
// last heard, and favourites keep their own filter whether heard or not.

/** When this window (and the app with it) started, by this computer's clock. */
export const APP_START = Date.now();

type Heard = { callsign: string; lastHeardUtc: string | null; favourite: boolean };

/** The stations the Heard list shows under `filter`: favourites under their
 *  own filter; otherwise only those heard since `since` (ms since the epoch). */
export function heardThisRun<T extends Heard>(list: T[], since: number, filter: string): T[] {
  if (filter === "favourite") return list.filter((h) => h.favourite);
  return list.filter((h) => !!h.lastHeardUtc && Date.parse(h.lastHeardUtc) >= since);
}
