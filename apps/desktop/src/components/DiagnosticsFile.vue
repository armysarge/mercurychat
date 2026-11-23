<script setup lang="ts">
// The diagnostics file (M5 §7), under Settings → About: what it would hold is
// listed first, then saved as a zip where the operator chooses. It holds the
// app's logs, the last crash report, Mercury's and rigctld's log tails, the
// versions and the station's settings without its email: never a message, a
// file or the database (the core decides; this only shows and asks).
import { save } from "@tauri-apps/plugin-dialog";
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import { call, describeError } from "../api";
import type { DiagEntryView } from "../bindings/DiagEntryView";

const { t } = useI18n();
const entries = ref<DiagEntryView[] | null>(null);
const busy = ref(false);
const note = ref("");
const error = ref("");

const size = (n: number) => (n < 1000 ? t("diagnostics.file.bytes", { n }) : t("diagnostics.file.kb", { n: (n / 1000).toFixed(1) }));

async function preview() {
  busy.value = true;
  note.value = "";
  error.value = "";
  try {
    const r = await call({ type: "diagnosticsPreview" });
    if (r.type === "diagnostics") entries.value = r.entries;
  } catch (e) {
    error.value = describeError(e);
  } finally {
    busy.value = false;
  }
}

async function saveIt() {
  error.value = "";
  const path = await save({
    defaultPath: `mercurychat-diagnostics-${new Date().toISOString().slice(0, 10)}.zip`,
    filters: [{ name: "Zip", extensions: ["zip"] }],
  });
  if (!path) return;
  busy.value = true;
  try {
    const r = await call({ type: "saveDiagnostics", path });
    if (r.type === "exported") {
      note.value = t("diagnostics.file.saved", { path: r.path });
      entries.value = null;
    }
  } catch (e) {
    error.value = describeError(e);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <section class="card stack diagnostics-file" aria-labelledby="h-diag-file">
    <h2 id="h-diag-file">{{ t("diagnostics.file.title") }}</h2>
    <p class="muted small">{{ t("diagnostics.file.body") }}</p>
    <div v-if="!entries"><button class="diag-preview" :disabled="busy" @click="preview">{{ t("diagnostics.file.prepare") }}</button></div>
    <template v-else>
      <p class="small">{{ t("diagnostics.file.holds") }}</p>
      <ul class="diag-entries small">
        <li v-for="e in entries" :key="e.name" :data-name="e.name"><span class="mono">{{ e.name }}</span> <span class="muted">{{ size(e.bytes) }}</span></li>
      </ul>
      <p class="muted small">{{ t("diagnostics.file.never") }}</p>
      <div class="row">
        <button class="primary diag-save" :disabled="busy" @click="saveIt">{{ t("diagnostics.file.save") }}</button>
        <button class="diag-cancel" :disabled="busy" @click="entries = null">{{ t("settings.cancel") }}</button>
      </div>
    </template>
    <p v-if="note" class="ok small" role="status">{{ note }}</p>
    <p v-if="error" class="err small" role="alert">{{ error }}</p>
  </section>
</template>

<style scoped>
.diag-entries { list-style: none; margin: 0; padding: 0; display: grid; gap: 2px; overflow-wrap: anywhere; }
.row { display: flex; flex-wrap: wrap; gap: var(--space); }
</style>
