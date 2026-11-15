<script setup lang="ts">
// Diagnostics (docs/04 §4): build and storage, the read-only modem probe and
// the IPC round-trip measurement (moved here from the M1 window).
import { invoke } from "@tauri-apps/api/core";
import { ref } from "vue";
import { describeError } from "../api";
import { info, measureIpc, pingMs } from "../diag";

interface ProbeResult {
  ok: boolean;
  summary: string;
  lines: string[];
}
const host = ref("127.0.0.1");
const port = ref(8300);
const probing = ref(false);
const probe = ref<ProbeResult | null>(null);

async function runProbe() {
  probing.value = true;
  probe.value = null;
  try {
    probe.value = await invoke<ProbeResult>("modem_probe", { host: host.value, port: Number(port.value) });
  } catch (e) {
    probe.value = { ok: false, summary: describeError(e), lines: [] };
  } finally {
    probing.value = false;
  }
}
</script>

<template>
  <div class="diag">
    <h1>Diagnostics</h1>
    <section class="card" aria-labelledby="h-build">
      <h2 id="h-build">Build and storage</h2>
      <dl v-if="info">
        <dt>App</dt><dd class="mono">{{ info.app_version }}</dd>
        <dt>Protocol</dt><dd class="mono">{{ info.protocol }}</dd>
        <dt>Platform</dt><dd class="mono">{{ info.os }} / {{ info.arch }}</dd>
        <dt>SQLite</dt><dd class="mono">schema v{{ info.db_schema }}</dd>
        <dt>Data</dt><dd class="mono path">{{ info.data_root }}</dd>
        <dt>Locks</dt><dd class="mono path">{{ info.locks_dir }}</dd>
        <dt>Core ready</dt><dd class="mono">{{ info.startup_ms }} ms after launch</dd>
      </dl>
      <p v-else class="muted">Loading…</p>
    </section>

    <section class="card stack" aria-labelledby="h-probe">
      <h2 id="h-probe">Modem probe (read-only)</h2>
      <p class="muted small">Attaches control + data, asks VERSION and BUFFER, reads 3 s of status lines, detaches. Changes no modem setting. Only this computer or a modem named in a station profile.</p>
      <form class="row" @submit.prevent="runProbe">
        <label>Host <input v-model="host" class="mono" size="14" /></label>
        <label>Control port <input v-model.number="port" class="mono" type="number" min="1" max="65534" /></label>
        <button type="submit" :disabled="probing">{{ probing ? "Probing…" : "Probe" }}</button>
      </form>
      <div v-if="probe" :class="['result', probe.ok ? 'ok' : 'err']" role="status">{{ probe.summary }}</div>
      <pre v-if="probe?.lines.length" class="mono log">{{ probe.lines.join("\n") }}</pre>
    </section>

    <section class="card stack" aria-labelledby="h-perf">
      <h2 id="h-perf">Responsiveness</h2>
      <div><button @click="measureIpc()">Measure IPC round trip (50×)</button></div>
      <p v-if="pingMs.length" class="mono">p50 {{ pingMs[0].toFixed(2) }} ms · p95 {{ pingMs[1].toFixed(2) }} ms · max {{ pingMs[2].toFixed(2) }} ms</p>
    </section>
  </div>
</template>

<style scoped>
.diag { padding: calc(var(--space) * 3); display: grid; gap: calc(var(--space) * 2); overflow-y: auto; height: 100%; align-content: start; max-width: 960px; }
h1 { font-size: 20px; }
dl { display: grid; grid-template-columns: max-content 1fr; gap: 4px 12px; margin: var(--space) 0 0; }
dt { color: var(--text-2); }
dd { margin: 0; }
.path { word-break: break-all; font-size: 12px; }
.result { font-weight: 600; }
.log { background: var(--raised); border-radius: 8px; padding: var(--space); max-height: 220px; overflow: auto; font-size: 12px; margin: 0; }
</style>
