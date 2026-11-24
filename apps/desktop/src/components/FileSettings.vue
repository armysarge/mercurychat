<script setup lang="ts">
// Files settings for the active station (M4): the largest incoming file (from
// 1 KiB to 256 MiB, 64 KiB by default; owner, 24 Nov: no 1 MiB cap), where received files land, and the photo size a
// send starts at. The core checks the bounds; its refusal is shown as is.
import { onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { call, describeError } from "../api";
import type { PhotoSize } from "../bindings/PhotoSize";
import { activeProfile, loadFileSettings } from "../store";

const { t } = useI18n();
const SIZES: PhotoSize[] = ["small", "medium", "large", "original"];
const kib = ref(64);
const dir = ref<string | null>(null);
const photo = ref<PhotoSize>("medium");
const error = ref("");
const saved = ref(false);
// The form shows once the stored values are in, so nothing typed is overwritten.
const loaded = ref(false);

async function load() {
  loaded.value = false;
  const s = await loadFileSettings();
  if (!s) return;
  kib.value = Math.round(Number(s.maxIncoming) / 1024);
  dir.value = s.receiveDir;
  photo.value = s.photoDefault;
  loaded.value = true;
}
onMounted(load);
watch(() => activeProfile.value?.id, load);

async function choose() {
  const { open } = await import("@tauri-apps/plugin-dialog");
  const picked = await open({ directory: true, multiple: false });
  if (typeof picked === "string") dir.value = picked;
}

async function save() {
  error.value = "";
  saved.value = false;
  try {
    const settings = { maxIncoming: Math.round(Number(kib.value) * 1024), receiveDir: dir.value, photoDefault: photo.value };
    await call({ type: "setFileSettings", settings });
    saved.value = true;
  } catch (e) {
    error.value = describeError(e);
  }
}
</script>

<template>
  <section v-if="activeProfile" class="card stack" aria-labelledby="h-files">
    <h2 id="h-files">{{ t("files.settings.title") }}</h2>
    <form v-if="loaded" class="file-settings stack" novalidate @submit.prevent="save">
      <label class="stack">
        <span class="label">{{ t("files.settings.maxIncoming") }}</span>
        <input v-model.number="kib" name="max-kib" type="number" min="1" max="262144" step="1" />
        <span class="muted small">{{ t("files.settings.maxNote") }}</span>
      </label>
      <div class="stack">
        <span class="label">{{ t("files.settings.receiveDir") }}</span>
        <span class="mono small dir">{{ dir ?? t("files.settings.receiveDefault") }}</span>
        <div class="actions">
          <button type="button" class="quiet choose" @click="choose">{{ t("files.settings.choose") }}</button>
          <button v-if="dir" type="button" class="quiet default" @click="dir = null">{{ t("files.settings.useDefault") }}</button>
        </div>
      </div>
      <label class="stack">
        <span class="label">{{ t("files.settings.photoDefault") }}</span>
        <select v-model="photo" name="photo-default">
          <option v-for="s in SIZES" :key="s" :value="s">{{ t(`files.photo.${s}`) }}</option>
        </select>
      </label>
      <p v-if="error" class="err" role="alert">{{ error }}</p>
      <p v-else-if="saved" class="ok small saved" role="status">{{ t("files.settings.saved") }}</p>
      <div class="actions">
        <button type="submit">{{ t("files.settings.save") }}</button>
      </div>
    </form>
    <p class="muted small">{{ t("files.settings.note") }}</p>
  </section>
</template>

<style scoped>
.actions { display: flex; gap: 8px; flex-wrap: wrap; }
.dir { overflow-wrap: anywhere; }
input[type="number"] { max-width: 10ch; }
</style>
