<script setup lang="ts">
// A small panel under a strip control (M3b). The strip clips whatever
// overflows its 60 px, so the panel is put on the page itself, below its
// button and inside the window. Escape (back to the button) or a click
// elsewhere closes it.
import { nextTick, onMounted, onUnmounted, ref, watch } from "vue";

const props = defineProps<{ open: boolean; anchor: HTMLElement | null; label: string }>();
const emit = defineEmits<{ close: [] }>();
const panel = ref<HTMLElement | null>(null);
const pos = ref({ left: 0, top: 0 });

function place() {
  const a = props.anchor?.getBoundingClientRect();
  if (!a) return;
  const w = panel.value?.offsetWidth ?? 240;
  pos.value = { left: Math.max(8, Math.min(a.left, window.innerWidth - w - 8)), top: a.bottom + 4 };
}
watch(
  () => props.open,
  async (open) => {
    if (!open) return;
    await nextTick();
    place();
    panel.value?.querySelector<HTMLElement>("button:not([disabled]), input:not([disabled]), select:not([disabled])")?.focus();
  },
);
function onKey(e: KeyboardEvent) {
  if (props.open && e.key === "Escape") {
    emit("close");
    props.anchor?.focus();
    e.stopPropagation();
  }
}
function onAway(e: Event) {
  const n = e.target as Node;
  if (props.open && !panel.value?.contains(n) && !props.anchor?.contains(n)) emit("close");
}
const onResize = () => props.open && emit("close");
onMounted(() => {
  document.addEventListener("keydown", onKey, true);
  document.addEventListener("mousedown", onAway, true);
  window.addEventListener("resize", onResize);
});
onUnmounted(() => {
  document.removeEventListener("keydown", onKey, true);
  document.removeEventListener("mousedown", onAway, true);
  window.removeEventListener("resize", onResize);
});
</script>

<template>
  <Teleport to="body">
    <div v-if="open" ref="panel" class="popover small" role="dialog" :aria-label="label" :style="{ left: `${pos.left}px`, top: `${pos.top}px` }">
      <slot />
    </div>
  </Teleport>
</template>

<style scoped>
.popover {
  position: fixed; z-index: 50; min-width: 200px; max-width: 360px; max-height: 60vh; overflow: auto;
  display: flex; flex-direction: column; gap: 6px; padding: 8px;
  background: var(--raised); color: var(--text);
  border: 1px solid var(--border-strong); border-radius: var(--radius); box-shadow: var(--shadow);
}
</style>
