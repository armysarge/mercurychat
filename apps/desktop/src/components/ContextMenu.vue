<script setup lang="ts">
// The only context menu the window has: cut, copy, paste and select all, on
// editable fields and selected text (shellGuards.ts decides when it opens).
import { nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { editMenu } from "../shellGuards";

const { t } = useI18n();
const menu = ref<HTMLElement | null>(null);

function close() {
  editMenu.open = false;
}
/** Run an edit command on the field (or selection) the menu was opened on. */
function run(command: "cut" | "copy" | "selectAll") {
  const target = editMenu.target;
  target?.focus();
  if (command === "selectAll" && (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement)) target.select();
  else document.execCommand(command);
  close();
}
async function paste() {
  const target = editMenu.target;
  close();
  try {
    const text = await navigator.clipboard.readText();
    target?.focus();
    document.execCommand("insertText", false, text);
  } catch {
    // The clipboard was not readable: Ctrl+V still pastes.
  }
}
function onKey(e: KeyboardEvent) {
  if (e.key === "Escape" && editMenu.open) {
    close();
    e.stopPropagation();
  }
}
function onAway(e: Event) {
  if (editMenu.open && !menu.value?.contains(e.target as Node)) close();
}
onMounted(() => {
  document.addEventListener("keydown", onKey, true);
  document.addEventListener("mousedown", onAway, true);
  window.addEventListener("blur", close);
  window.addEventListener("resize", close);
});
onUnmounted(() => {
  document.removeEventListener("keydown", onKey, true);
  document.removeEventListener("mousedown", onAway, true);
  window.removeEventListener("blur", close);
  window.removeEventListener("resize", close);
});
// Keep the menu inside the window.
watch(
  () => editMenu.open,
  async (open) => {
    if (!open) return;
    await nextTick();
    const el = menu.value;
    if (!el) return;
    editMenu.x = Math.max(0, Math.min(editMenu.x, window.innerWidth - el.offsetWidth - 4));
    editMenu.y = Math.max(0, Math.min(editMenu.y, window.innerHeight - el.offsetHeight - 4));
  },
);
</script>

<template>
  <div v-if="editMenu.open" ref="menu" class="ctx-menu" role="menu" :aria-label="t('menu.label')" :style="{ left: editMenu.x + 'px', top: editMenu.y + 'px' }" @mousedown.prevent>
    <button v-if="editMenu.canCut" role="menuitem" @click="run('cut')">{{ t("menu.cut") }}</button>
    <button v-if="editMenu.canCopy" role="menuitem" @click="run('copy')">{{ t("menu.copy") }}</button>
    <button v-if="editMenu.canPaste" role="menuitem" @click="paste">{{ t("menu.paste") }}</button>
    <button role="menuitem" @click="run('selectAll')">{{ t("menu.selectAll") }}</button>
  </div>
</template>

<style scoped>
.ctx-menu {
  position: fixed; z-index: 100; display: grid; min-width: 150px; padding: 4px;
  background: var(--raised); border: 1px solid var(--border); border-radius: 10px; box-shadow: 0 8px 24px var(--shadow);
}
.ctx-menu button { justify-content: flex-start; width: 100%; text-align: left; border-color: transparent; background: transparent; padding: 6px 10px; }
.ctx-menu button:hover { background: var(--surface); }
</style>
