// The station strip's tuning arithmetic (M3b Task 10), kept apart from the
// component so node can test it: step one digit, read what the operator
// typed, and show Hz as "7.105.000".

/** The highest frequency the strip tunes to (6 m tops out at 54 MHz). */
export const FREQ_MAX_HZ = 54_000_000;
/** What an entry may hold at most (matches the core's 60 MHz bound). */
const PARSE_MAX_HZ = 60_000_000;

/** `hz` with digit `digitIndex` (0 = 1 Hz … 7 = 10 MHz) moved by one, clamped to 0..54 MHz. */
export function stepDigit(hz: number, digitIndex: number, delta: 1 | -1): number {
  const next = Math.round(hz) + delta * 10 ** digitIndex;
  return Math.min(FREQ_MAX_HZ, Math.max(0, next));
}

/**
 * A typed frequency in Hz, or null. Under 100 is MHz, under 100 000 kHz,
 * anything larger Hz. A separator used more than once groups thousands; with
 * both `,` and `.` the last is the decimal point; a lone one is a decimal
 * point either way ("7,105" is 7.105 MHz).
 */
export function parseFreqInput(text: string): number | null {
  let s = text.trim();
  if (!s || !/^[0-9.,]+$/.test(s) || /[.,]{2}/.test(s)) return null;
  const dots = (s.match(/\./g) ?? []).length;
  const commas = (s.match(/,/g) ?? []).length;
  if (dots && commas) {
    const decimal = s.lastIndexOf(".") > s.lastIndexOf(",") ? "." : ",";
    const group = decimal === "." ? "," : ".";
    if ((decimal === "." ? dots : commas) > 1) return null;
    s = s.split(group).join("").replace(",", ".");
  } else if (dots > 1 || commas > 1) {
    s = s.replace(/[.,]/g, "");
  } else {
    s = s.replace(",", ".");
  }
  const v = Number(s);
  if (!Number.isFinite(v) || v <= 0) return null;
  const hz = Math.round(v < 100 ? v * 1e6 : v < 100_000 ? v * 1e3 : v);
  return hz > 0 && hz <= PARSE_MAX_HZ ? hz : null;
}

/** "7.105.000" from Hz; dashes for none. */
export function formatDigits(hz: number | null | undefined): string {
  if (hz === undefined || hz === null) return "--.---.---";
  const n = Math.round(hz);
  const khz = String(Math.floor((n % 1_000_000) / 1000)).padStart(3, "0");
  return `${Math.floor(n / 1_000_000)}.${khz}.${String(n % 1000).padStart(3, "0")}`;
}

/** The amateur bands, as the core names them (crates/mc-core/src/band.rs): [low Hz, high Hz, name]. */
export const BANDS: readonly (readonly [number, number, string])[] = [
  [1_800_000, 2_000_000, "160m"],
  [3_500_000, 4_000_000, "80m"],
  [5_250_000, 5_450_000, "60m"],
  [7_000_000, 7_300_000, "40m"],
  [10_100_000, 10_150_000, "30m"],
  [14_000_000, 14_350_000, "20m"],
  [18_068_000, 18_168_000, "17m"],
  [21_000_000, 21_450_000, "15m"],
  [24_890_000, 24_990_000, "12m"],
  [28_000_000, 29_700_000, "10m"],
  [50_000_000, 54_000_000, "6m"],
];

/** The band a dial frequency falls in, edges included; null outside every band. */
export function bandFor(hz: number | null | undefined): string | null {
  if (hz === null || hz === undefined) return null;
  return BANDS.find(([lo, hi]) => hz >= lo && hz <= hi)?.[2] ?? null;
}
