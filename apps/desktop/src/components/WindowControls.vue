<script setup lang="ts">
// MercuryChat draws its own title bar (the window has no system frame):
// minimise, maximise/restore and close, at the right end of the header.
import { getCurrentWindow } from "@tauri-apps/api/window";
import { onMounted, onUnmounted, ref } from "vue";
import { useI18n } from "vue-i18n";

const { t } = useI18n();
const maximized = ref(false);
let unlisten: (() => void) | undefined;

function win() {
  try {
    return getCurrentWindow();
  } catch {
    return null; // not inside Tauri (a plain browser preview)
  }
}
async function sync() {
  maximized.value = (await win()?.isMaximized()) ?? false;
}

onMounted(async () => {
  const w = win();
  if (!w) return;
  await sync();
  unlisten = await w.onResized(() => void sync());
});
onUnmounted(() => unlisten?.());
</script>

<template>
  <div class="winctl">
    <button class="min" :title="t('window.minimise')" :aria-label="t('window.minimise')" @click="win()?.minimize()">
      <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M1 5.5h8" /></svg>
    </button>
    <button class="max" :title="t(maximized ? 'window.restore' : 'window.maximise')" :aria-label="t(maximized ? 'window.restore' : 'window.maximise')" @click="win()?.toggleMaximize()">
      <svg v-if="maximized" viewBox="0 0 10 10" aria-hidden="true"><path d="M3 1.5h5.5V7M1.5 3h5.5v5.5H1.5z" /></svg>
      <svg v-else viewBox="0 0 10 10" aria-hidden="true"><path d="M1.5 1.5h7v7h-7z" /></svg>
    </button>
    <button class="close" :title="t('window.close')" :aria-label="t('window.close')" @click="win()?.close()">
      <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M1.5 1.5l7 7M8.5 1.5l-7 7" /></svg>
    </button>
  </div>
</template>

<style scoped>
.winctl { display: flex; align-self: stretch; app-region: no-drag; }
.winctl button {
  width: 46px; padding: 0; border: 0; border-radius: 0; background: transparent; color: var(--text-2);
  display: grid; place-items: center;
}
.winctl button:hover { background: var(--raised); color: var(--text); }
.winctl button.close:hover { background: #c42b1c; color: #fff; }
.winctl button:focus-visible { outline-offset: -2px; }
svg { width: 10px; height: 10px; fill: none; stroke: currentColor; stroke-width: 1; shape-rendering: crispEdges; }
.close svg { shape-rendering: geometricPrecision; }
@media (max-width: 900px) {
  .winctl button { width: 40px; }
}
</style>
