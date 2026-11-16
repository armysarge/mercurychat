<script setup lang="ts">
// Conversations (docs/04 §4): the chosen peer's history (the callsign list is
// the app's sidebar), readable whether or not a link is up. Ctrl+F searches it.
import { onMounted, onUnmounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import Transcript from "../components/Transcript.vue";
import { chat } from "../store";

const { t } = useI18n();
const search = ref("");
const box = ref<HTMLInputElement | null>(null);

function onKey(e: KeyboardEvent) {
  if (e.ctrlKey && (e.key === "f" || e.key === "F")) {
    e.preventDefault();
    box.value?.focus();
  }
}
onMounted(() => document.addEventListener("keydown", onKey));
onUnmounted(() => document.removeEventListener("keydown", onKey));
</script>

<template>
  <div class="conversations">
    <section class="history" :aria-label="t('conversations.history')">
      <div class="row head">
        <h1 class="mono">{{ chat.open || t("conversations.history") }}</h1>
        <span class="grow"></span>
        <input ref="box" v-model="search" type="search" class="history-search" :aria-label="t('conversations.historySearchAria')" :placeholder="t('conversations.historySearchPlaceholder')" />
      </div>
      <Transcript class="fill" :filter="search" />
    </section>
  </div>
</template>

<style scoped>
.conversations { display: grid; grid-template-columns: minmax(0, 1fr); height: 100%; min-height: 0; }
.history { display: flex; flex-direction: column; min-height: 0; }
.head { padding: calc(var(--space) * 1.5) calc(var(--space) * 2); border-bottom: 1px solid var(--border); }
.head h1 { font-size: 18px; }
.fill { flex: 1; }
</style>
