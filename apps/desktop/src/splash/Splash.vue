<script setup lang="ts">
// 360x180, frameless, centred: the logo, an indeterminate bar and one status
// line while the station data opens and the modem starts. A start-up failure
// shows its message here with a Close button instead of a blank window.
import { invoke } from "@tauri-apps/api/core";
import { onMounted, onUnmounted, ref } from "vue";
import { useI18n } from "vue-i18n";

type Status = { stage: "opening" } | { stage: "modem" } | { stage: "failed"; message: string };

const { t } = useI18n();
const status = ref<Status>({ stage: "opening" });
let timer: ReturnType<typeof setInterval> | undefined;

async function poll() {
  try {
    status.value = await invoke<Status>("splash_status");
  } catch {
    // The shell is not answering yet: keep the last line.
  }
}
onMounted(() => {
  void poll();
  timer = setInterval(poll, 200);
});
onUnmounted(() => clearInterval(timer));
const quit = () => invoke("quit_app");
</script>

<template>
  <main class="splash">
    <img v-if="status.stage !== 'failed'" class="mark" src="/mercurychat.svg" width="56" height="56" alt="" />
    <b v-if="status.stage !== 'failed'" class="name">MercuryChat</b>
    <template v-if="status.stage === 'failed'">
      <section class="failure" role="alert">
        <p class="err title">{{ t("app.startFailed") }}</p>
        <p class="message small">{{ status.message }}</p>
      </section>
      <button class="primary" @click="quit">{{ t("splash.close") }}</button>
    </template>
    <template v-else>
      <p class="status small muted" role="status">{{ t(status.stage === "opening" ? "splash.opening" : "splash.modem") }}</p>
      <div class="bar" role="progressbar" :aria-label="t('splash.progressAria')"><i></i></div>
    </template>
  </main>
</template>

<style scoped>
.splash {
  height: 100%; box-sizing: border-box; display: flex; flex-direction: column; align-items: center; justify-content: center;
  gap: var(--space); padding: calc(var(--space) * 2); background: var(--canvas); color: var(--text);
  border: 1px solid var(--border); text-align: center;
}
.mark { display: block; }
.name { font-size: 16px; font-weight: 650; }
.status { min-height: 1.3em; }
.bar { width: 200px; height: 4px; border-radius: 999px; background: var(--raised); overflow: hidden; }
.bar i { display: block; height: 100%; width: 40%; border-radius: 999px; background: var(--accent); animation: slide 1.2s ease-in-out infinite; }
@keyframes slide {
  from { transform: translateX(-100%); }
  to { transform: translateX(250%); }
}
/* No motion: a still bar, part full, so it is not mistaken for a finished one. */
@media (prefers-reduced-motion: reduce) {
  .bar i { animation: none; margin-inline: auto; }
}
.failure { display: grid; gap: 4px; max-height: 84px; overflow-y: auto; width: 100%; }
.title { font-weight: 600; }
.message { overflow-wrap: anywhere; user-select: text; }
</style>
