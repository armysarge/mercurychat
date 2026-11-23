// Exports (M5 §7): the operator picks where in the system's save dialog; the
// core writes the file there. Nothing is written without that choice.
import { save } from "@tauri-apps/plugin-dialog";
import { call } from "./api";
import type { ExportFormat } from "./bindings/ExportFormat";

const today = () => new Date().toISOString().slice(0, 10);
/** A callsign as a file name part: `/` (portable) becomes `-`. */
const safe = (s: string) => s.replace(/[^A-Za-z0-9-]+/g, "-");

const EXT: Record<ExportFormat, { ext: string; name: string }> = {
  adif: { ext: "adi", name: "ADIF" },
  csv: { ext: "csv", name: "CSV" },
};

/** The logbook as ADIF or CSV; `null` when the dialog was cancelled, else the record count. */
export async function exportLogbook(format: ExportFormat, finalisedOnly: boolean, callsign: string): Promise<number | null> {
  const { ext, name } = EXT[format];
  const path = await save({ defaultPath: `mercurychat-${safe(callsign)}-${today()}.${ext}`, filters: [{ name, extensions: [ext] }] });
  if (!path) return null;
  const r = await call({ type: "exportLogbook", format, finalisedOnly, path });
  return r.type === "exported" ? r.count : 0;
}

/** One conversation as text; `null` when cancelled, else the message count. */
export async function exportConversation(peer: string): Promise<number | null> {
  const path = await save({ defaultPath: `mercurychat-${safe(peer)}-${today()}.txt`, filters: [{ name: "Text", extensions: ["txt"] }] });
  if (!path) return null;
  const r = await call({ type: "exportConversation", peer, path });
  return r.type === "exported" ? r.count : 0;
}
