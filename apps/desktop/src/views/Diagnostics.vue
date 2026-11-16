<script setup lang="ts">
// Diagnostics (docs/04 §4): build and storage, the read-only modem probe and
// the IPC round-trip measurement (moved here from the M1 window).
import { invoke } from "@tauri-apps/api/core";
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import { describeError } from "../api";
import { info, measureIpc, pingMs } from "../diag";

interface ProbeResult {
  ok: boolean;
  summary: string;
  lines: string[];
}
const { t } = useI18n();
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
    <h1>{{ t("diag.title") }}</h1>
    <section class="card" aria-labelledby="h-build">
      <h2 id="h-build">{{ t("diag.buildTitle") }}</h2>
      <dl v-if="info">
        <dt>{{ t("diag.app") }}</dt><dd class="mono">{{ info.app_version }}</dd>
        <dt>{{ t("diag.protocol") }}</dt><dd class="mono">{{ info.protocol }}</dd>
        <dt>{{ t("diag.platform") }}</dt><dd class="mono">{{ info.os }} / {{ info.arch }}</dd>
        <dt>{{ t("diag.sqlite") }}</dt><dd class="mono">{{ t("diag.schema", { n: info.db_schema }) }}</dd>
        <dt>{{ t("diag.data") }}</dt><dd class="mono path">{{ info.data_root }}</dd>
        <dt>{{ t("diag.locks") }}</dt><dd class="mono path">{{ info.locks_dir }}</dd>
        <dt>{{ t("diag.coreReady") }}</dt><dd class="mono">{{ t("diag.afterLaunch", { ms: info.startup_ms }) }}</dd>
      </dl>
      <p v-else class="muted">{{ t("diag.loading") }}</p>
    </section>

    <section class="card stack" aria-labelledby="h-probe">
      <h2 id="h-probe">{{ t("diag.probeTitle") }}</h2>
      <p class="muted small">{{ t("diag.probeBody") }}</p>
      <form class="row" @submit.prevent="runProbe">
        <label>{{ t("diag.host") }} <input v-model="host" class="mono" size="14" /></label>
        <label>{{ t("diag.controlPort") }} <input v-model.number="port" class="mono" type="number" min="1" max="65534" /></label>
        <button type="submit" :disabled="probing">{{ probing ? t("diag.probing") : t("diag.probe") }}</button>
      </form>
      <div v-if="probe" :class="['result', probe.ok ? 'ok' : 'err']" role="status">{{ probe.summary }}</div>
      <pre v-if="probe?.lines.length" class="mono log">{{ probe.lines.join("\n") }}</pre>
    </section>

    <section class="card stack" aria-labelledby="h-perf">
      <h2 id="h-perf">{{ t("diag.perfTitle") }}</h2>
      <div><button @click="measureIpc()">{{ t("diag.measure") }}</button></div>
      <p v-if="pingMs.length" class="mono">{{ t("diag.latency", { p50: pingMs[0].toFixed(2), p95: pingMs[1].toFixed(2), max: pingMs[2].toFixed(2) }) }}</p>
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
