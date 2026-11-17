<script setup lang="ts">
// Periodic beacons (DSC-03; docs/04 §8.1): off by default, an on/off switch
// for this run, the interval, whether the locator is included, the countdown
// to the next one, and why it is held when it is. The stored preference is
// shown but never applied by itself: after a restart or a recovery the
// operator turns beacons on again. The channel is always shown, and Unknown
// says so in words.
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { activeProfile, discoveryError, modemState, setBeacon, setBeaconContent, setBeaconInterval, snapshot } from "../store";
import { countdown } from "./countdown";
import Switch from "./Switch.vue";

const { t } = useI18n();
const props = defineProps<{ now: number }>();
const beacon = computed(() => snapshot.value?.beacon ?? null);
const idle = computed(() => modemState.value === "ready" || modemState.value === "listening");
const INTERVALS = [5, 10, 15, 20, 30, 45, 60];
const next = computed(() => (beacon.value?.active && beacon.value.nextDueUtc ? countdown(beacon.value.nextDueUtc, props.now) : null));
const channelKey = computed(() => `beacon.channel.${beacon.value?.channel.type ?? "unknown"}`);
const hasGrid = computed(() => !!activeProfile.value?.grid);

function onToggle(e: Event) {
  const box = e.target as HTMLInputElement;
  const wanted = box.checked;
  box.checked = beacon.value?.active ?? false; // follows the core, never ahead of it
  void setBeacon(wanted);
}
</script>

<template>
  <section class="card stack beacon" aria-labelledby="h-beacon">
    <span id="h-beacon" class="label">{{ t("beacon.title") }}</span>
    <template v-if="beacon">
      <label class="switch-row">
        <span>{{ t("beacon.periodic") }}</span>
        <Switch input-class="beacon-toggle" :checked="beacon.active" :disabled="!activeProfile || (!beacon.active && !idle)" @change="onToggle" />
      </label>
      <p v-if="beacon.preferred && !beacon.active" class="warn small prompt">{{ t("beacon.prompt") }}</p>
      <div class="row">
        <label class="inline">
          {{ t("beacon.interval") }}
          <select class="beacon-interval" :value="beacon.intervalMin" :disabled="!activeProfile" @change="setBeaconInterval(Number(($event.target as HTMLSelectElement).value))">
            <option v-for="m in INTERVALS" :key="m" :value="m">{{ t("beacon.minutes", { n: m }) }}</option>
          </select>
        </label>
        <span class="muted small">{{ t("beacon.jitter") }}</span>
      </div>
      <label class="switch-row">
        <span>{{ t("beacon.includeGrid") }}</span>
        <Switch input-class="beacon-grid" :checked="beacon.includeGrid" :disabled="!activeProfile" @change="setBeaconContent(($event.target as HTMLInputElement).checked)" />
      </label>
      <p v-if="beacon.includeGrid && !hasGrid" class="muted small">{{ t("beacon.noGrid") }}</p>
      <div class="stat">
        <span class="muted">{{ t("beacon.next") }}</span>
        <span v-if="next" class="mono countdown">{{ next }}</span>
        <span v-else-if="beacon.active && beacon.suppressed" class="warn small held">{{ t("beacon.suppressed", { reason: beacon.suppressed }) }}</span>
        <span v-else class="muted small">{{ t("beacon.off") }}</span>
      </div>
      <div class="stat"><span class="muted">{{ t("beacon.channelLabel") }}</span><span :class="['channel', beacon.channel.type]">{{ t(channelKey) }}</span></div>
      <p v-if="discoveryError.beacon" class="err small" role="alert">{{ discoveryError.beacon }}</p>
    </template>
  </section>
</template>

<style scoped>
.inline { flex-direction: row; align-items: center; gap: 6px; }
.stat { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 8px; border-top: 1px solid var(--border); padding-top: 6px; font-size: calc(13px * var(--font-scale)); }
.channel.busy { color: var(--warning); }
.channel.clear { color: var(--success); }
.channel.unknown { color: var(--text-2); }
</style>
