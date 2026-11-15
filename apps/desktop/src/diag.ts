// Start-up facts from the shell and the IPC round-trip measurement (M0/Pi
// responsiveness evidence), shared by the shell and the Diagnostics view.
import { invoke } from "@tauri-apps/api/core";
import { ref } from "vue";

export interface AppInfo {
  app_version: string;
  protocol: string;
  os: string;
  arch: string;
  data_root: string;
  db_path: string;
  db_schema: number;
  locks_dir: string;
  startup_ms: number;
  startup_error: string | null;
}

export const info = ref<AppInfo | null>(null);
export const pingMs = ref<number[]>([]);

export async function measureIpc(n = 50) {
  const samples: number[] = [];
  for (let i = 0; i < n; i++) {
    const t = performance.now();
    await invoke("ping");
    samples.push(performance.now() - t);
  }
  samples.sort((a, b) => a - b);
  const q = (p: number) => samples[Math.min(n - 1, Math.floor(p * n))];
  pingMs.value = [q(0.5), q(0.95), samples[n - 1]];
  await invoke("report_ipc", { p50: pingMs.value[0], p95: pingMs.value[1], max: pingMs.value[2], samples: n });
}
