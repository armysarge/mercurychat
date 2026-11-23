<script setup lang="ts">
// A station's round initials (the design canvas, M5 Task 11b): the first and
// last character of the callsign (SIM1 → S1), in one of the badge colours
// picked from the callsign, so a station keeps its colour everywhere. A dot
// says a link is up with it. Decoration only: the callsign is always written
// beside it, so it is hidden from screen readers.
import { computed } from "vue";

const props = defineProps<{ callsign: string; live?: boolean; size?: number }>();
const TONES = ["session", "cq", "beacon"] as const;

const initials = computed(() => {
  const base = (props.callsign.split("/").find((p) => p.length > 2) ?? props.callsign).replace(/[^A-Z0-9]/gi, "").toUpperCase();
  return base.length <= 2 ? base : base[0] + base[base.length - 1];
});
const tone = computed(() => {
  let h = 0;
  for (const c of props.callsign.toUpperCase()) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return TONES[h % TONES.length];
});
</script>

<template>
  <span :class="['avatar', tone]" :style="size ? { '--av': `${size}px` } : undefined" aria-hidden="true">
    {{ initials }}<span v-if="live" class="dot"></span>
  </span>
</template>

<style scoped>
.avatar {
  --av: 34px; --tone: var(--badge-session);
  position: relative; flex: none; display: inline-grid; place-items: center;
  width: var(--av); height: var(--av); border-radius: 50%;
  font: 600 calc(var(--av) * 0.38) / 1 var(--font-mono); letter-spacing: 0.02em;
  color: var(--tone); background: color-mix(in srgb, var(--tone) 16%, var(--surface));
  border: 1px solid color-mix(in srgb, var(--tone) 35%, transparent);
}
.avatar.cq { --tone: var(--badge-cq); }
.avatar.beacon { --tone: var(--badge-beacon); }
.dot {
  position: absolute; right: -1px; bottom: -1px; width: 10px; height: 10px; border-radius: 50%;
  background: var(--success); border: 2px solid var(--surface);
}
</style>
