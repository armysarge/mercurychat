// The Logbook export's checks before saving (owner's Log4OM import, 24 Nov):
// how many QSOs have neither band nor frequency (loggers refuse them, so the
// operator is warned), and whether a typed ADIF mode is one the core takes.
// Pure: unit-tested by scripts/logExport.test.mjs.

type ExportQso = { state: string; band: string | null; freq: { value: number } | null };

/** QSOs an ADIF export would write that have neither band nor frequency. */
export function withoutBand(qsos: readonly ExportQso[], finalisedOnly: boolean): number {
  return qsos.filter((q) => (!finalisedOnly || q.state === "final") && !q.band && !q.freq).length;
}

/** The core's rule (export::adif_mode): 1 to 20 of A–Z, 0–9 and '-', after trimming and upper-casing. */
export function adifModeOk(raw: string): boolean {
  return /^[A-Z0-9-]{1,20}$/.test(raw.trim().toUpperCase());
}
