<script setup lang="ts">
// Files (M4): every transfer of this station in three lists, newest first:
// incoming, outgoing, and those interrupted or failed. Each card says how it
// ended, whether the SHA-256 checked out, and the name it was saved under.
import { computed, onMounted } from "vue";
import { useI18n } from "vue-i18n";
import TransferCard from "../components/TransferCard.vue";
import { files, reloadTransfers } from "../store";
import { lists } from "../transfers";

const { t } = useI18n();
const l = computed(() => lists(files.t));
const SECTIONS = ["incoming", "outgoing", "ended"] as const;
onMounted(reloadTransfers);
</script>

<template>
  <div class="files-view">
    <h1>{{ t("files.title") }}</h1>
    <p v-if="files.error" class="err" role="alert">{{ files.error }}</p>
    <section v-for="name in SECTIONS" :key="name" :class="['list', name]" :aria-labelledby="`h-files-${name}`">
      <h2 :id="`h-files-${name}`">{{ t(`files.view.${name}`) }}</h2>
      <p v-if="!l[name].length" class="muted small">{{ t("files.view.empty") }}</p>
      <TransferCard v-for="v in l[name]" :key="v.id" :v="v" />
    </section>
  </div>
</template>

<style scoped>
.files-view { padding: calc(var(--space) * 3); overflow-y: auto; height: 100%; display: grid; gap: calc(var(--space) * 2); align-content: start; }
.list { display: grid; gap: var(--space); }
.list :deep(.transfer) { margin-left: 0; max-width: 60ch; }
</style>
