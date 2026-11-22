// Quick messages (M3c B8): the composer's macro menu lists the macros marked
// "quick" first, each in its own order. Picking one inserts it into the draft
// at the cursor, as any macro does; only Enter sends. Kept pure for node --test.
import type { MacroDef } from "./bindings/MacroDef";

/** What a click on a menu item does: insert, never send. */
export const PICK_ACTION = "insert" as const;

/** The quick macros, then the others; the list itself is left as it was. */
export function quickFirst<M extends Pick<MacroDef, "quick">>(list: readonly M[]): { quick: M[]; others: M[] } {
  return { quick: list.filter((m) => m.quick), others: list.filter((m) => !m.quick) };
}
