// Settings → Rig (M3b.1 design §5): which boxes show for a configuration and
// which PTT choices fit a Frequency control. Pure, so node can test it
// (scripts/rigForm.test.mjs).
import type { FreqControl } from "./bindings/FreqControl";
import type { PttMethod } from "./bindings/PttMethod";
import type { RigConfig } from "./bindings/RigConfig";

export type Box = "cat" | "external" | "flrig" | "serialPtt" | "cm108";

/** The Frequency control dropdown, in order. */
export const FREQ_CONTROLS = ["managedRigctld", "externalRigctld", "flrig", "none"] as const satisfies readonly FreqControl[];
/** The PTT dropdown, in order. */
export const PTT_METHODS = ["cat", "serialLine", "cm108", "noPtt", "leaveMercury"] as const satisfies readonly PttMethod[];
export const LINKS = ["serial", "tcp"] as const;
export const PARITIES = ["none", "even", "odd"] as const;
export const LINE_STATES = ["unset", "on", "off"] as const;
export const PTT_LINES = ["rts", "dtr"] as const;
export const READ_EVERY = [1, 2, 5, 10] as const;
export const BAUDS = [1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200] as const;
export const DATA_MODES = ["PKTUSB", "PKTLSB", "USB", "LSB", "FM", "PKTFM"] as const;

/** The boxes Settings → Rig shows for `c`. */
export function visibleBoxes(c: RigConfig): Set<Box> {
  const out = new Set<Box>();
  if (c.freqControl === "managedRigctld") out.add("cat");
  if (c.freqControl === "externalRigctld") out.add("external");
  if (c.freqControl === "flrig") out.add("flrig");
  if (c.ptt.method === "serialLine") out.add("serialPtt");
  if (c.ptt.method === "cm108") out.add("cm108");
  return out;
}

/** CAT PTT needs a rigctld: not offered with FLRig or no rig link. */
export function pttChoices(f: FreqControl): PttMethod[] {
  return PTT_METHODS.filter((m) => m !== "cat" || f === "managedRigctld" || f === "externalRigctld");
}

/** The core's defaults (RigConfig::default). */
export function defaultRigConfig(): RigConfig {
  return {
    freqControl: "none",
    external: { host: "127.0.0.1", port: 4532 },
    flrig: { host: "127.0.0.1", port: 12345 },
    cat: {
      model: null,
      link: "serial",
      serialPort: "",
      tcpAddr: "",
      baud: 9600,
      parity: "none",
      dataBits: 8,
      stopBits: 1,
      waitMs: 0,
      dtr: "unset",
      rts: "unset",
    },
    ptt: { method: "leaveMercury", line: "rts", invert: false, pttPort: "", cm108Gpio: 3 },
    restoreOnStartup: false,
    offsetHz: 0,
    readEveryS: 2,
    dataMode: "PKTUSB",
    hamlibDir: "",
  };
}

/** Owner, 24 Nov: frequency control starts on the built-in rigctld. A station
 *  with nothing set up yet (control "none" and no rig model) is offered it in
 *  the form; it is stored only when the operator saves, so nothing starts by
 *  itself. A saved choice is kept. */
export function withBuiltInDefault(c: RigConfig): RigConfig {
  if (c.freqControl !== "none" || c.cat.model != null) return c;
  return { ...c, freqControl: "managedRigctld" };
}
