// The fake built-in bundle for window checks on a debug build (MC_BUNDLE_DIR
// is read only by debug builds): the fake Mercury and rigctld, their
// manifest and a BUILD.txt, beside the check's data dir.
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";

export const MERCURY = "1.9.25";
export const HAMLIB = "4.7.2";

export function fakeBundle(exe, dataDir) {
  const bin = dirname(exe);
  const dir = `${dataDir}-bundle`;
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  let manifest = "";
  for (const [from, to] of [["fake-mercury.exe", "mercury.exe"], ["fake-rigctld.exe", "rigctld.exe"]]) {
    if (!existsSync(join(bin, from))) throw new Error(`${from} is not built: cargo build -p mercury-adapter --bins`);
    copyFileSync(join(bin, from), join(dir, to));
    manifest += `${createHash("sha256").update(readFileSync(join(dir, to))).digest("hex")}  ${to}\n`;
  }
  writeFileSync(join(dir, "MANIFEST.sha256"), manifest);
  writeFileSync(join(dir, "BUILD.txt"), `target=windows-x64\nhamlib_version=${HAMLIB}\nmercury_version=v${MERCURY}\n`);
  return dir;
}
