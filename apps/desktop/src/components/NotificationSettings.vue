<script setup lang="ts">
// Desktop notifications (M3a Task 12b): a master switch, one switch per kind,
// the OS default sound, hiding message text, quiet hours (urgent faults still
// show) and whether a Virtual Audio Lab profile notifies. App settings, kept
// by the core. The OS is asked for permission only when the operator turns a
// notification on here, never at start-up.
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { describeError } from "../api";
import type { AppError } from "../bindings/AppError";
import type { NotifyPrefs } from "../bindings/NotifyPrefs";
import { askPermission } from "../notifier";
import { DEFAULT_PREFS } from "../notify";
import { setNotifyPrefs, snapshot } from "../store";

const { t } = useI18n();
const KINDS = ["message", "incoming", "callFailed", "delivery", "heardFavourite", "heardAny", "fault", "beaconRefused"] as const;
type Kind = (typeof KINDS)[number];
const prefs = computed<NotifyPrefs>(() => snapshot.value?.notifyPrefs ?? DEFAULT_PREFS);
const error = ref("");
const denied = ref(false);
const quietOn = computed(() => !!prefs.value.quietStart && !!prefs.value.quietEnd);

async function save(change: Partial<NotifyPrefs>) {
  error.value = "";
  const next = { ...prefs.value, ...change };
  // Turning something on is the moment to ask the OS (once it has answered, it is not asked again).
  const turnedOn = Object.entries(change).some(([k, v]) => v === true && k !== "hideText" && k !== "sound");
  const saved = setNotifyPrefs(next);
  saved.catch(() => {}); // handled below, after the permission question
  if (turnedOn && next.enabled) denied.value = !(await askPermission());
  try {
    await saved;
  } catch (e) {
    const err = e as AppError;
    error.value = err && typeof err === "object" && err.code === "validation" ? err.errors.map((f) => f.message).join("; ") : describeError(e);
  }
}
const toggle = (k: Kind | "enabled" | "sound" | "hideText" | "labEnabled", e: Event) => save({ [k]: (e.target as HTMLInputElement).checked });
const setQuiet = (on: boolean) => save(on ? { quietStart: "22:00", quietEnd: "07:00" } : { quietStart: null, quietEnd: null });
const setTime = (k: "quietStart" | "quietEnd", e: Event) => {
  const v = (e.target as HTMLInputElement).value;
  if (/^\d\d:\d\d$/.test(v)) void save({ [k]: v });
};
</script>

<template>
  <section class="card stack notifications" aria-labelledby="h-notify">
    <h2 id="h-notify">{{ t("notifySettings.title") }}</h2>
    <label class="row toggle">
      <input type="checkbox" class="notify-enabled" :checked="prefs.enabled" @change="toggle('enabled', $event)" />
      <span>{{ t("notifySettings.enabled") }}</span>
    </label>
    <fieldset :disabled="!prefs.enabled" class="kinds">
      <legend class="label">{{ t("notifySettings.kindsLegend") }}</legend>
      <label v-for="k in KINDS" :key="k" class="row toggle">
        <input type="checkbox" :class="`notify-${k}`" :checked="prefs[k]" @change="toggle(k, $event)" />
        <span>{{ t(`notifySettings.kind.${k}`) }}</span>
      </label>
    </fieldset>
    <fieldset :disabled="!prefs.enabled" class="how">
      <legend class="label">{{ t("notifySettings.howLegend") }}</legend>
      <label class="row toggle"><input type="checkbox" :checked="prefs.sound" @change="toggle('sound', $event)" /><span>{{ t("notifySettings.sound") }}</span></label>
      <label class="row toggle"><input type="checkbox" class="notify-hide-text" :checked="prefs.hideText" @change="toggle('hideText', $event)" /><span>{{ t("notifySettings.hideText") }}</span></label>
      <label class="row toggle"><input type="checkbox" :checked="quietOn" @change="setQuiet(($event.target as HTMLInputElement).checked)" /><span>{{ t("notifySettings.quiet") }}</span></label>
      <div v-if="quietOn" class="row quiet">
        <label>{{ t("notifySettings.quietFrom") }} <input type="time" :value="prefs.quietStart ?? ''" @change="setTime('quietStart', $event)" /></label>
        <label>{{ t("notifySettings.quietTo") }} <input type="time" :value="prefs.quietEnd ?? ''" @change="setTime('quietEnd', $event)" /></label>
      </div>
      <label class="row toggle"><input type="checkbox" :checked="prefs.labEnabled" @change="toggle('labEnabled', $event)" /><span>{{ t("notifySettings.lab") }}</span></label>
    </fieldset>
    <p class="muted small">{{ t("notifySettings.note") }}</p>
    <p v-if="denied" class="warn small" role="status">{{ t("notifySettings.denied") }}</p>
    <p v-if="error" class="err small" role="alert">{{ error }}</p>
  </section>
</template>

<style scoped>
fieldset { border: 0; padding: 0; margin: 0; display: grid; gap: 4px; }
fieldset:disabled { opacity: 0.6; }
.kinds { grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); }
.quiet { gap: calc(var(--space) * 2); padding-left: 1.6em; }
</style>
