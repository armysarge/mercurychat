<script setup lang="ts">
// The send dialog (M4 C2): the picked file is prepared (a photo shrunk to the
// chosen size, everything compressed when that is smaller) before anything is
// sent, and the operator sees what it costs: its size, its size on the air
// and the airtime, labelled an estimate until a rate is measured. An original
// photo's location data is said out loud. Cancel discards the prepared file.
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { call, describeError } from "../api";
import type { PhotoSize } from "../bindings/PhotoSize";
import type { PreparedView } from "../bindings/PreparedView";
import { files, loadFileSettings } from "../store";
import { airtime, formatSize, isPhotoName } from "../transfers";

const { t } = useI18n();
const SIZES: PhotoSize[] = ["small", "medium", "large", "original"];
const prep = ref<PreparedView | null>(null);
const photo = ref<PhotoSize>("medium");
const busy = ref(false);
const error = ref("");
const name = computed(() => files.sending?.split(/[\\/]/).pop() ?? "");
const isPhoto = computed(() => isPhotoName(name.value));
let generation = 0;

function discard(p: PreparedView | null) {
  if (p) call({ type: "discardPrepared", prepId: p.prepId }).catch(() => {});
}

async function prepare() {
  const path = files.sending;
  if (!path) return;
  const mine = ++generation;
  discard(prep.value);
  prep.value = null;
  busy.value = true;
  error.value = "";
  try {
    const r = await call({ type: "prepareFile", path, photo: photo.value });
    if (r.type !== "prepared") return;
    const { type: _type, ...view } = r;
    // A newer choice (or a closed dialog) replaced this one: let it go.
    if (mine !== generation || files.sending !== path) return discard(view);
    prep.value = view;
  } catch (e) {
    if (mine === generation) error.value = describeError(e);
  } finally {
    if (mine === generation) busy.value = false;
  }
}

watch(
  () => files.sending,
  async (path) => {
    if (!path) return;
    photo.value = (await loadFileSettings())?.photoDefault ?? "medium";
    await prepare();
  },
);

function close() {
  generation++;
  discard(prep.value);
  prep.value = null;
  busy.value = false;
  error.value = "";
  files.sending = null;
}

async function send() {
  const p = prep.value;
  if (!p) return;
  busy.value = true;
  try {
    await call({ type: "sendFile", prepId: p.prepId });
    prep.value = null; // sent: the core owns it now
    close();
  } catch (e) {
    error.value = describeError(e);
    busy.value = false;
  }
}

const air = computed(() => {
  const p = prep.value;
  if (!p) return "";
  const a = airtime(p.airtimeS);
  return t(a.key, { n: a.n });
});
</script>

<template>
  <div v-if="files.sending" class="overlay" @keydown.esc="close">
    <form class="send-file card stack" role="dialog" aria-modal="true" aria-labelledby="h-send-file" @submit.prevent="send">
      <h2 id="h-send-file">{{ t("files.dialog.title") }}</h2>
      <div class="mono name">{{ name }}</div>
      <label v-if="isPhoto" class="stack">
        <span class="label">{{ t("files.dialog.photoSize") }}</span>
        <select v-model="photo" name="photo" :disabled="busy" @change="prepare">
          <option v-for="s in SIZES" :key="s" :value="s">{{ t(`files.photo.${s}`) }}</option>
        </select>
      </label>
      <p v-if="busy && !prep" class="muted small" role="status">{{ t("files.dialog.preparing") }}</p>
      <dl v-if="prep" class="facts">
        <dt>{{ t("files.dialog.size") }}</dt>
        <dd class="size">{{ formatSize(Number(prep.size)) }}<span v-if="prep.shrunk" class="muted"> · {{ t("files.dialog.shrunk") }}</span></dd>
        <dt>{{ t("files.dialog.onAir") }}</dt>
        <dd class="wire">{{ formatSize(Number(prep.wireSize)) }}<span v-if="prep.compressed" class="muted"> · {{ t("files.dialog.compressed") }}</span></dd>
        <dt>{{ t("files.dialog.airtime") }}</dt>
        <dd class="airtime">{{ air }}<span v-if="!prep.measured" class="muted"> · {{ t("files.dialog.estimate") }}</span></dd>
      </dl>
      <p v-if="prep?.keepsMetadata" class="warn small metadata">{{ t("files.dialog.metadata") }}</p>
      <p v-if="error" class="err" role="alert">{{ error }}</p>
      <div class="actions">
        <button type="submit" class="send" :disabled="!prep || busy">{{ t("files.dialog.send") }}</button>
        <button type="button" class="quiet cancel" @click="close">{{ t("files.dialog.cancel") }}</button>
      </div>
    </form>
  </div>
</template>

<style scoped>
.overlay { position: fixed; inset: 0; z-index: 50; display: grid; place-items: center; background: var(--shadow); }
.send-file { width: min(440px, 92vw); }
.name { overflow-wrap: anywhere; }
.facts { display: grid; grid-template-columns: auto 1fr; gap: 4px 12px; margin: 0; }
.facts dt { color: var(--text-2); }
.facts dd { margin: 0; }
.actions { display: flex; gap: 8px; justify-content: flex-end; }
</style>
