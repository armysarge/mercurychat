<script setup lang="ts">
// The station menu (stationMenu.ts): one station's actions, from its row in
// Heard or Chats, or from the conversation header's "…". A menu by the ARIA
// pattern: focus on the first item, arrows move, Escape closes and gives focus
// back to where it was opened, Tab closes and moves on.
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import {
  callStation, closeChat, closeMenu, exportChat, hasHistory, menuNote, openChat, star, stationMenu, suggestQsy,
} from "../stationMenu";
import { activeProfile, app, canCall, discovery } from "../store";

const { t } = useI18n();
const menu = ref<HTMLElement | null>(null);

type Item = { action: string; label: string; disabled?: boolean; run: () => unknown };

const items = computed<Item[]>(() => {
  const c = stationMenu.callsign;
  const heard = discovery.heard.find((h) => h.callsign === c);
  const list: Item[] = [];
  if (stationMenu.from !== "header") {
    list.push({ action: "open", label: t("stationMenu.open"), run: () => openChat(c) });
    list.push({ action: "call", label: t("stationMenu.call", { callsign: c }), disabled: !canCall.value, run: () => callStation(c) });
  }
  list.push({ action: "suggest-qsy", label: t("qsy.suggestAria", { callsign: c }), disabled: !activeProfile.value, run: () => suggestQsy(c) });
  if (heard) {
    list.push({
      action: "star",
      label: t(heard.favourite ? "heard.unstar" : "heard.star", { callsign: c }),
      disabled: !activeProfile.value,
      run: () => star(c, !heard.favourite),
    });
  }
  if (hasHistory(c)) {
    list.push({ action: "export", label: t("stationMenu.export"), run: () => exportChat(c, (n, peer) => t("operate.exported", { n, peer }, n)) });
  }
  if (stationMenu.from === "header" && hasHistory(c)) {
    list.push({ action: "search", label: t("stationMenu.search"), run: () => (app.page = "conversations") });
  }
  if (stationMenu.from === "header") list.push({ action: "close", label: t("stationMenu.close"), run: () => closeChat() });
  return list;
});

const buttons = () => [...(menu.value?.querySelectorAll<HTMLButtonElement>("[role=menuitem]:not(:disabled)") ?? [])];

function pick(item: Item) {
  // Close first, without refocusing: the action may move focus (a new page, a form).
  closeMenu(false);
  void item.run();
}

function onKey(e: KeyboardEvent) {
  const all = buttons();
  const at = all.indexOf(document.activeElement as HTMLButtonElement);
  const go = (i: number) => all[(i + all.length) % all.length]?.focus();
  if (e.key === "Escape") closeMenu();
  else if (e.key === "Tab") closeMenu(false);
  else if (e.key === "ArrowDown") go(at + 1);
  else if (e.key === "ArrowUp") go(at - 1);
  else if (e.key === "Home") go(0);
  else if (e.key === "End") go(all.length - 1);
  else return;
  if (e.key !== "Tab") {
    e.preventDefault();
    e.stopPropagation();
  }
}

function onAway(e: Event) {
  if (stationMenu.open && !menu.value?.contains(e.target as Node) && e.target !== stationMenu.opener) closeMenu(false);
}
const shut = () => closeMenu(false);
onMounted(() => {
  document.addEventListener("mousedown", onAway, true);
  window.addEventListener("blur", shut);
  window.addEventListener("resize", shut);
});
onUnmounted(() => {
  document.removeEventListener("mousedown", onAway, true);
  window.removeEventListener("blur", shut);
  window.removeEventListener("resize", shut);
});

// Inside the window, then focus on the first item.
watch(
  () => [stationMenu.open, stationMenu.callsign, stationMenu.x, stationMenu.y],
  async () => {
    if (!stationMenu.open) return;
    await nextTick();
    const el = menu.value;
    if (!el) return;
    stationMenu.x = Math.max(4, Math.min(stationMenu.x, window.innerWidth - el.offsetWidth - 4));
    stationMenu.y = Math.max(4, Math.min(stationMenu.y, window.innerHeight - el.offsetHeight - 4));
    buttons()[0]?.focus();
  },
);

// The export's result stays a few seconds.
let noteTimer: ReturnType<typeof setTimeout> | undefined;
watch(
  () => menuNote.text,
  (text) => {
    clearTimeout(noteTimer);
    if (text) noteTimer = setTimeout(() => (menuNote.text = ""), 8000);
  },
);
</script>

<template>
  <div
    v-if="stationMenu.open"
    ref="menu"
    class="station-menu"
    role="menu"
    :aria-label="t('stationMenu.label', { callsign: stationMenu.callsign })"
    :style="{ left: stationMenu.x + 'px', top: stationMenu.y + 'px' }"
    @keydown="onKey"
  >
    <p class="who mono small" aria-hidden="true">{{ stationMenu.callsign }}</p>
    <button v-for="i in items" :key="i.action" role="menuitem" :data-action="i.action" :disabled="i.disabled" @click="pick(i)">{{ i.label }}</button>
  </div>
  <p v-if="menuNote.text" :class="['menu-note', 'small', menuNote.failed ? 'err' : 'ok']" :role="menuNote.failed ? 'alert' : 'status'">
    {{ menuNote.text }}
    <button class="quiet" :aria-label="t('stationMenu.dismiss')" @click="menuNote.text = ''">×</button>
  </p>
</template>

<style scoped>
.station-menu {
  position: fixed; z-index: 100; display: grid; min-width: 200px; max-width: min(320px, calc(100vw - 8px)); padding: 4px;
  background: var(--raised); border: 1px solid var(--border); border-radius: 12px; box-shadow: 0 8px 24px var(--shadow);
}
.who { margin: 0; padding: 4px 10px 6px; color: var(--text-2); border-bottom: 1px solid var(--border); margin-bottom: 4px; }
.station-menu button { justify-content: flex-start; width: 100%; text-align: left; border-color: transparent; background: transparent; padding: 7px 10px; border-radius: 8px; }
.station-menu button:hover:not(:disabled), .station-menu button:focus-visible { background: var(--surface); }
.menu-note {
  position: fixed; z-index: 90; right: 16px; bottom: 16px; max-width: min(420px, calc(100vw - 32px)); margin: 0;
  display: flex; gap: 8px; align-items: center; padding: 8px 8px 8px 12px;
  background: var(--raised); border: 1px solid var(--border); border-radius: 10px; box-shadow: 0 8px 24px var(--shadow);
}
.menu-note button { min-height: 0; padding: 2px 8px; }
</style>
