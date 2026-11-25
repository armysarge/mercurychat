<script lang="ts">
import { ref } from "vue";

/** One unsent message, shared by the header menu and the station home, so closing the menu keeps it. */
const text = ref("");
</script>

<script setup lang="ts">
// A short message to all stations (a one-off beacon whose status line is the
// message, up to 40 bytes), only while the modem is idle. Used by the header's
// Broadcast menu and by the station home's Broadcast card.
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { activeProfile, discoveryError, modemState, sendBroadcastMessage } from "../store";

const emit = defineEmits<{ sent: [] }>();
const { t } = useI18n();
const MAX = 40;
const idle = computed(() => modemState.value === "ready" || modemState.value === "listening");
const usable = computed(() => !!activeProfile.value && idle.value);
const bytes = computed(() => new TextEncoder().encode(text.value.trim()).length);
const canSend = computed(() => usable.value && bytes.value > 0 && bytes.value <= MAX);

async function send() {
  if (!canSend.value) return;
  await sendBroadcastMessage(text.value);
  if (!discoveryError.message) {
    text.value = "";
    emit("sent");
  }
}
</script>

<template>
  <form class="broadcast-message" @submit.prevent="send">
    <label>
      {{ t("broadcast.message") }}
      <input v-model="text" class="broadcast-text" maxlength="80" autocomplete="off" :placeholder="t('broadcast.placeholder')" :disabled="!usable" />
    </label>
    <div class="send-row">
      <span :class="['small', bytes > MAX ? 'err' : 'muted']">{{ t("broadcast.bytes", { n: bytes, max: MAX }) }}</span>
      <button type="submit" class="primary broadcast-send" :disabled="!canSend">{{ t("broadcast.send") }}</button>
    </div>
    <p v-if="discoveryError.message" class="err small" role="alert">{{ discoveryError.message }}</p>
  </form>
</template>

<style scoped>
.broadcast-message { display: grid; gap: 6px; }
.broadcast-message label { display: grid; gap: 4px; font-size: calc(12px * var(--font-scale)); color: var(--text-2); }
.send-row { display: flex; justify-content: space-between; align-items: center; gap: 8px; }
p { margin: 0; }
</style>
