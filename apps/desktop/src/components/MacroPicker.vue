<script setup lang="ts">
// The composer's macro menu (CHT-06): each macro of the station, expanded by
// the core into text for the draft, and a separate "Insert email" item. The
// email address goes in only by that explicit item: it fills a macro's <EMAIL>
// tags in the draft, or goes in at the cursor. Nothing here sends anything.
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { describeError } from "../api";
import { quickFirst } from "../quick";
import { activeProfile, chat, expandMacro, loadMacros, macros } from "../store";

const { t } = useI18n();
const emit = defineEmits<{ insert: [text: string, missing: string[], asEmail: boolean] }>();
const open = ref(false);
const error = ref("");
const root = ref<HTMLElement | null>(null);
const email = computed(() => activeProfile.value?.email?.trim() ?? "");
// Quick messages on top (M3c B8); a click inserts, Enter in the composer sends.
const sections = computed(() => quickFirst(macros.list));

async function toggle() {
  open.value = !open.value;
  error.value = "";
  if (open.value && !macros.list.length) await loadMacros();
}
async function pick(id: string) {
  open.value = false;
  try {
    const r = await expandMacro(id);
    emit("insert", r.text, r.missing, false);
  } catch (e) {
    error.value = describeError(e);
  }
}
function insertEmail() {
  open.value = false;
  if (email.value) emit("insert", email.value, [], true);
}
function away(e: Event) {
  if (open.value && root.value && !root.value.contains(e.target as Node)) open.value = false;
}
function onKey(e: KeyboardEvent) {
  if (open.value && e.key === "Escape") {
    open.value = false;
    e.stopPropagation();
  }
}
onMounted(() => document.addEventListener("pointerdown", away));
onUnmounted(() => document.removeEventListener("pointerdown", away));
</script>

<template>
  <div ref="root" class="macros" @keydown="onKey">
    <button type="button" class="macro-toggle quiet" aria-haspopup="menu" :aria-expanded="open" :disabled="!chat.open || !activeProfile" @click="toggle">
      <svg class="macro-icon" viewBox="0 0 24 24" aria-hidden="true"><path d="M13 3 5 14h6l-1 7 8-11h-6l1-7Z" /></svg>
      {{ t("macros.button") }}
    </button>
    <div v-if="open" class="macro-menu" role="menu" :aria-label="t('macros.menuAria')">
      <template v-if="sections.quick.length">
        <p class="muted small macro-heading" role="presentation">{{ t("macros.quickTitle") }}</p>
        <button v-for="m in sections.quick" :key="m.id" type="button" role="menuitem" class="macro-quick" :title="m.body" @click="pick(m.id)">{{ m.name }}</button>
        <hr v-if="sections.others.length" />
      </template>
      <button v-for="m in sections.others" :key="m.id" type="button" role="menuitem" @click="pick(m.id)">{{ m.name }}</button>
      <p v-if="!macros.list.length" class="muted small">{{ t("macros.none") }}</p>
      <hr />
      <button type="button" role="menuitem" class="macro-email" :disabled="!email" :title="email ? email : t('macros.noEmail')" @click="insertEmail">{{ t("macros.insertEmail") }}</button>
    </div>
    <p v-if="error" class="err small" role="alert">{{ error }}</p>
  </div>
</template>

<style scoped>
.macros { position: relative; }
.macro-toggle { display: inline-flex; align-items: center; gap: 6px; min-height: 0; padding: 8px 10px; border-color: transparent; }
.macro-icon { width: 18px; height: 18px; fill: none; stroke: currentColor; stroke-width: 1.8; stroke-linejoin: round; }
@media (max-width: 1099px) { .macro-toggle { font-size: 0; gap: 0; } }
.macro-menu {
  position: absolute; bottom: calc(100% + 4px); left: 0; z-index: 25; min-width: 200px; max-height: 280px; overflow-y: auto;
  display: grid; padding: 4px; border: 1px solid var(--border-strong); border-radius: 9px; background: var(--raised); box-shadow: 0 6px 18px var(--shadow);
}
.macro-menu button { justify-content: flex-start; width: 100%; text-align: left; border-color: transparent; background: transparent; padding: 6px 10px; }
.macro-menu button:hover:not(:disabled), .macro-menu button:focus-visible { background: var(--surface); }
.macro-menu hr { border: 0; border-top: 1px solid var(--border); margin: 4px 0; width: 100%; }
.macro-menu p { margin: 4px 10px; }
.macro-menu .macro-heading { margin: 4px 10px 2px; font-weight: 600; }
</style>
