<script setup lang="ts">
// The one sidebar, on every page but first-use setup: Chats (the conversation
// list, the default) and Heard. App.vue keeps this single instance mounted
// outside the page views, so the tab, the search text, the scroll position and
// the selection all survive going from page to page. Below 960 px it is a
// drawer (04 §2).
import { useI18n } from "vue-i18n";
import ConversationList from "./ConversationList.vue";
import HeardList from "./HeardList.vue";
import { app, discovery } from "../store";

const { t } = useI18n();
const TABS = [
  { id: "chats", label: "sidebar.chats" },
  { id: "heard", label: "sidebar.heard" },
] as const;

/** A conversation was picked: show it (from a page that cannot) and close the drawer. */
function opened() {
  if (app.page !== "operate" && app.page !== "conversations") app.page = "operate";
  app.drawer = null;
}
</script>

<template>
  <aside class="sidebar" :aria-label="t('sidebar.label')">
    <div class="tabs" role="tablist">
      <button
        v-for="tab in TABS"
        :id="`side-tab-${tab.id}`"
        :key="tab.id"
        role="tab"
        :aria-selected="app.sideTab === tab.id"
        :aria-controls="`side-pane-${tab.id}`"
        :aria-describedby="tab.id === 'heard' && discovery.unseen ? 'side-heard-new' : undefined"
        :class="{ on: app.sideTab === tab.id }"
        @click="app.sideTab = tab.id"
      >
        {{ t(tab.label) }}
      </button>
      <!-- Stations heard since the Heard tab was last open, beside that tab (its own name stays unchanged). -->
      <span v-if="discovery.unseen" id="side-heard-new" class="unseen" :title="t('sidebar.heardNew', discovery.unseen)">
        <span aria-hidden="true">{{ discovery.unseen }}</span><span class="sr-only">{{ t("sidebar.heardNew", discovery.unseen) }}</span>
      </span>
      <span class="grow"></span>
      <button class="close quiet" :aria-label="t('sidebar.close')" @click="app.drawer = null">×</button>
    </div>
    <div v-show="app.sideTab === 'chats'" id="side-pane-chats" class="pane" role="tabpanel" aria-labelledby="side-tab-chats">
      <ConversationList @opened="opened" />
    </div>
    <div v-show="app.sideTab === 'heard'" id="side-pane-heard" class="pane" role="tabpanel" aria-labelledby="side-tab-heard">
      <HeardList @opened="opened" />
    </div>
  </aside>
</template>

<style scoped>
.sidebar { display: flex; flex-direction: column; min-height: 0; min-width: 0; background: var(--canvas); border-right: 1px solid var(--border); }
.tabs { display: flex; gap: 4px; align-items: center; padding: var(--space) var(--space) 0; border-bottom: 1px solid var(--border); }
.tabs button[role="tab"] {
  border: 0; border-bottom: 2px solid transparent; border-radius: 8px 8px 0 0; background: transparent; color: var(--text-2);
  padding: 6px 14px; font-weight: 600;
}
.tabs button[role="tab"].on { color: var(--accent); border-bottom-color: var(--accent); }
.close { display: none; padding: 2px 10px; font-size: calc(18px * var(--font-scale)); line-height: 1; }
.pane { flex: 1; min-height: 0; overflow: hidden; }
.unseen {
  display: inline-block; min-width: 18px; margin-left: 4px; padding: 0 5px; border-radius: 999px;
  background: var(--accent); color: var(--accent-text); font-size: calc(11px * var(--font-scale)); line-height: 18px; text-align: center;
}
@media (max-width: 960px) {
  .sidebar {
    position: fixed; top: 0; left: 0; bottom: 0; width: min(300px, 92vw); z-index: 30; border-right: 1px solid var(--border);
    transform: translateX(-100%); visibility: hidden; box-shadow: 8px 0 24px var(--shadow);
    transition: transform 120ms ease-out, visibility 120ms;
  }
  .sidebar.open { transform: none; visibility: visible; }
  .close { display: inline-block; }
}
</style>
