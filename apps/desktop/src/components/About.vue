<script setup lang="ts">
// About (owner request, 16 Nov): the app, its version and build, who founded
// it, its licence, and the projects it is built on. The links open in the
// system browser through the shell's open_link, by name; "Check for updates"
// only opens the releases page. The window itself never fetches anything
// (OPS-02: no automatic internet lookup).
import { invoke } from "@tauri-apps/api/core";
import { onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { describeError } from "../api";

const { t } = useI18n();
const version = ref("");
const build = ref("");
const error = ref("");
onMounted(async () => {
  try {
    const info = await invoke<{ app_version: string; build: string }>("app_info");
    version.value = info.app_version;
    build.value = info.build;
  } catch (e) {
    error.value = describeError(e);
  }
});

const LINKS = ["project", "changelog", "issues", "releases"] as const;
async function open(link: (typeof LINKS)[number]) {
  error.value = "";
  try {
    await invoke("open_link", { link });
  } catch (e) {
    error.value = t("about.openFailed", { reason: describeError(e) });
  }
}
const CREDITS = ["mercury", "hamlib", "dseg"] as const;
</script>

<template>
  <section class="card stack about" aria-labelledby="h-about">
    <h2 id="h-about">{{ t("about.title") }}</h2>
    <p class="name">
      <b>MercuryChat</b>
      <span class="mono about-version">{{ version }}</span>
      <span class="muted small mono">{{ build }}</span>
    </p>
    <p>{{ t("about.founded") }}</p>
    <p class="small">{{ t("about.licence") }}</p>
    <div class="row links">
      <button v-for="l in LINKS" :key="l" :data-link="l" @click="open(l)">{{ t(`about.link.${l}`) }}</button>
    </div>
    <p class="muted small">{{ t("about.updatesNote") }}</p>
    <h3>{{ t("about.credits") }}</h3>
    <ul class="credits small">
      <li v-for="c in CREDITS" :key="c">{{ t(`about.credit.${c}`) }}</li>
    </ul>
    <p v-if="error" class="err small" role="alert">{{ error }}</p>
  </section>
</template>

<style scoped>
.name { display: flex; flex-wrap: wrap; align-items: baseline; gap: var(--space); margin: 0; }
.links { flex-wrap: wrap; }
h3 { font-size: calc(14px * var(--font-scale)); margin: var(--space) 0 0; }
.credits { margin: 0; padding-left: 1.2em; display: grid; gap: 4px; }
</style>
