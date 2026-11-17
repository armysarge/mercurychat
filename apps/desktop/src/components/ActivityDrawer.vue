<script setup lang="ts">
// The activity log (docs/04 §2): what the modem and the broadcast plane did,
// newest first, with monospace UTC times. Collapsible; open or closed is
// remembered. Every line is the core's own words.
import { ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { discovery } from "../store";

const { t } = useI18n();
const KEY = "mc.activityOpen";
const stored = (() => {
  try {
    return localStorage.getItem(KEY);
  } catch {
    return null;
  }
})();
const open = ref(stored === null ? true : stored === "1");
watch(open, (v) => {
  try {
    localStorage.setItem(KEY, v ? "1" : "0");
  } catch {
    // Storage may be unavailable: the choice then lasts for this run only.
  }
});
const time = (utc: string) => utc.slice(11, 19);
</script>

<template>
  <section class="card activity" aria-labelledby="h-activity">
    <button id="h-activity" class="head quiet" :aria-expanded="open" aria-controls="activity-lines" @click="open = !open">
      <span class="label">{{ t("activity.title") }}</span>
      <span class="muted small">{{ t("activity.count", discovery.activity.length) }}</span>
      <span class="grow"></span>
      <span aria-hidden="true">{{ open ? "▾" : "▸" }}</span>
    </button>
    <ol v-show="open" id="activity-lines" class="lines" :aria-label="t('activity.listAria')">
      <li v-for="(e, i) in discovery.activity" :key="`${e.atUtc}-${i}`" :class="['line', e.kind]" :data-kind="e.kind">
        <time class="mono muted" :datetime="e.atUtc">{{ time(e.atUtc) }}</time>
        <span class="kind mono small">{{ e.kind }}</span>
        <span class="text">{{ e.text }}</span>
      </li>
      <li v-if="!discovery.activity.length" class="muted small">{{ t("activity.empty") }}</li>
    </ol>
  </section>
</template>

<style scoped>
.activity { display: grid; gap: var(--space); padding: var(--space) calc(var(--space) * 1.5); }
.head { display: flex; align-items: center; gap: var(--space); padding: 2px 0; border: 0; text-align: left; width: 100%; }
.lines { list-style: none; margin: 0; padding: 0; display: grid; gap: 2px; max-height: 240px; overflow-y: auto; font-size: calc(12px * var(--font-scale)); }
.line { display: grid; grid-template-columns: auto auto minmax(0, 1fr); gap: 6px; align-items: baseline; }
.kind { color: var(--text-2); }
.text { overflow-wrap: anywhere; }
.line.malformed .kind, .line.unsupported .kind { color: var(--warning); }
.line.heard .kind { color: var(--accent); }
</style>
