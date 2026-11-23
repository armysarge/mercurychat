// Spectrum forwarding is one switch in the core (`set_waterfall_visible`),
// while the window can show more than one spectrum (the station strip's and
// the idle station home's, M5 Task 11b). Each shown spectrum holds a claim;
// the core forwards frames while at least one claim is held. The order in
// which spectra mount and unmount then never matters.
import { invoke } from "@tauri-apps/api/core";

const claims = new Set<symbol>();
let forwarding = false;

/** Hold or release one spectrum's claim; resolves to an error in words, or "". */
export async function claimSpectrum(who: symbol, on: boolean): Promise<string> {
  if (on) claims.add(who);
  else claims.delete(who);
  const want = claims.size > 0;
  if (want === forwarding) return "";
  forwarding = want;
  try {
    await invoke("set_waterfall_visible", { visible: want });
    return "";
  } catch (e) {
    forwarding = !want;
    return String(e);
  }
}
