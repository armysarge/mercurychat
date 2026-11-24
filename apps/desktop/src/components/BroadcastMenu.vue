<script setup lang="ts">
// The header's quick access to what goes out to everyone (owner, 24 Nov): Call
// CQ, Beacon now, and a short message to all stations (a one-off beacon whose
// status line is the message, up to 40 bytes). Only while the modem is idle.
import { computed, nextTick, onMounted, onUnmounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { activeProfile, discoveryError, modemState, sendBeaconNow, sendBroadcastMessage, sendCq } from "../store";

const { t } = useI18n();
const MAX = 40;
const open = ref(false);
const text = ref("");
const root = ref<HTMLElement | null>(null);
const idle = computed(() => modemState.value === "ready" || modemState.value === "listening");
const usable = computed(() => !!activeProfile.value && idle.value);
const bytes = computed(() => new TextEncoder().encode(text.value.trim()).length);
const canSend = computed(() => usable.value && bytes.value > 0 && bytes.value <= MAX);
const error = computed(() => discoveryError.cq || discoveryError.beacon || discoveryError.message);

/** Open or close the panel: focus goes into it, and back to the button on close; old errors go. */
async function toggle() {
  open.value = !open.value;
  if (open.value) {
    discoveryError.cq = discoveryError.beacon = discoveryError.message = "";
    await nextTick();
    root.value?.querySelector<HTMLElement>(".broadcast-text:not([disabled]), .broadcast-cq:not([disabled])")?.focus();
  }
}
async function run(step: () => Promise<unknown>) {
  await step();
  if (!error.value) open.value = false;
}
async function send() {
  if (!canSend.value) return;
  await sendBroadcastMessage(text.value);
  if (!discoveryError.message) {
    text.value = "";
    open.value = false;
  }
}
function away(e: Event) {
  if (open.value && root.value && !root.value.contains(e.target as Node)) open.value = false;
}
function onKey(e: KeyboardEvent) {
  if (open.value && e.key === "Escape") {
    open.value = false;
    e.stopPropagation();
    root.value?.querySelector<HTMLElement>(".broadcast-toggle")?.focus();
  }
}
onMounted(() => document.addEventListener("pointerdown", away));
onUnmounted(() => document.removeEventListener("pointerdown", away));
</script>

<template>
  <div v-if="activeProfile" ref="root" class="broadcast-menu" @keydown="onKey">
    <button type="button" class="broadcast-toggle quiet" aria-haspopup="dialog" :aria-expanded="open" :title="t('broadcast.title')" @click="toggle">
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M12 12h.01M8.5 8.5a5 5 0 0 0 0 7M15.5 8.5a5 5 0 0 1 0 7M5.6 5.6a9 9 0 0 0 0 12.8M18.4 5.6a9 9 0 0 1 0 12.8" /></svg>
      <span class="label">{{ t("broadcast.button") }}</span>
    </button>
    <div v-if="open" class="broadcast-panel" role="dialog" :aria-label="t('broadcast.title')">
      <div class="buttons">
        <button class="broadcast-cq" :disabled="!usable" :title="t('strip.idleOnly')" @click="run(sendCq)">{{ t("conversations.footCq") }}</button>
        <button class="broadcast-beacon" :disabled="!usable" :title="t('strip.idleOnly')" @click="run(sendBeaconNow)">{{ t("conversations.footBeacon") }}</button>
      </div>
      <form class="message" @submit.prevent="send">
        <label>
          {{ t("broadcast.message") }}
          <input v-model="text" class="broadcast-text" maxlength="80" autocomplete="off" :placeholder="t('broadcast.placeholder')" :disabled="!usable" />
        </label>
        <div class="row">
          <span :class="['small', bytes > MAX ? 'err' : 'muted']">{{ t("broadcast.bytes", { n: bytes, max: MAX }) }}</span>
          <button type="submit" class="primary broadcast-send" :disabled="!canSend">{{ t("broadcast.send") }}</button>
        </div>
      </form>
      <p v-if="!usable" class="muted small">{{ t("strip.idleOnly") }}</p>
      <p v-if="error" class="err small" role="alert">{{ error }}</p>
      <p class="muted small">{{ t("broadcast.hint") }}</p>
    </div>
  </div>
</template>

<style scoped>
.broadcast-menu { position: relative; }
.broadcast-toggle { display: inline-flex; align-items: center; gap: 6px; }
.broadcast-toggle svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 1.8; stroke-linecap: round; }
.broadcast-panel {
  position: absolute; z-index: 30; top: calc(100% + 8px); right: 0; width: min(340px, 92vw);
  display: grid; gap: var(--space); padding: calc(var(--space) * 1.5);
  background: var(--surface); border: 1px solid var(--border-strong); border-radius: 14px;
}
.buttons { display: grid; grid-template-columns: 1fr 1fr; gap: 6px; }
.message { display: grid; gap: 6px; }
.message label { display: grid; gap: 4px; font-size: calc(12px * var(--font-scale)); color: var(--text-2); }
.row { display: flex; justify-content: space-between; align-items: center; gap: 8px; }
p { margin: 0; }
@media (max-width: 1100px) { .broadcast-toggle .label { display: none; } }
</style>
