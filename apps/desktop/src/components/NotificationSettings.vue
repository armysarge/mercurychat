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
import Switch from "./Switch.vue";

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
    <label class="switch-row master">
      <span>{{ t("notifySettings.enabled") }}</span>
      <Switch input-class="notify-enabled" :checked="prefs.enabled" @change="toggle('enabled', $event)" />
    </label>
    <fieldset :disabled="!prefs.enabled" class="kinds">
      <legend class="label">{{ t("notifySettings.kindsLegend") }}</legend>
      <div class="grid">
        <label v-for="k in KINDS" :key="k" class="switch-row">
          <span>{{ t(`notifySettings.kind.${k}`) }}</span>
          <Switch :input-class="`notify-${k}`" :checked="prefs[k]" :disabled="!prefs.enabled" @change="toggle(k, $event)" />
        </label>
      </div>
    </fieldset>
    <fieldset :disabled="!prefs.enabled" class="how">
      <legend class="label">{{ t("notifySettings.howLegend") }}</legend>
      <div class="grid">
        <label class="switch-row">
          <span>{{ t("notifySettings.sound") }}</span>
          <Switch :checked="prefs.sound" :disabled="!prefs.enabled" @change="toggle('sound', $event)" />
        </label>
        <label class="switch-row">
          <span>{{ t("notifySettings.hideText") }}</span>
          <Switch input-class="notify-hide-text" :checked="prefs.hideText" :disabled="!prefs.enabled" @change="toggle('hideText', $event)" />
        </label>
        <label class="switch-row">
          <span>{{ t("notifySettings.lab") }}</span>
          <Switch :checked="prefs.labEnabled" :disabled="!prefs.enabled" @change="toggle('labEnabled', $event)" />
        </label>
        <div class="quiet-hours">
          <label class="switch-row">
            <span>{{ t("notifySettings.quiet") }}</span>
            <Switch :checked="quietOn" :disabled="!prefs.enabled" @change="setQuiet(($event.target as HTMLInputElement).checked)" />
          </label>
          <div v-if="quietOn" class="times">
            <label>{{ t("notifySettings.quietFrom") }} <input type="time" :value="prefs.quietStart ?? ''" @change="setTime('quietStart', $event)" /></label>
            <label>{{ t("notifySettings.quietTo") }} <input type="time" :value="prefs.quietEnd ?? ''" @change="setTime('quietEnd', $event)" /></label>
          </div>
        </div>
      </div>
    </fieldset>
    <p class="muted small">{{ t("notifySettings.note") }}</p>
    <p v-if="denied" class="warn small" role="status">{{ t("notifySettings.denied") }}</p>
    <p v-if="error" class="err small" role="alert">{{ error }}</p>
  </section>
</template>

<style scoped>
fieldset { border: 0; padding: 0; margin: 0; min-width: 0; transition: opacity 160ms ease; }
fieldset:disabled { opacity: 0.55; }
legend { padding: 0; margin-bottom: 4px; }
.master { font-weight: 600; border-bottom: 1px solid var(--border); }
/* Two columns of rows on a wide window, one on a narrow one; each row ends in its switch. */
.grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(300px, 1fr)); column-gap: calc(var(--space) * 4); }
.grid > .switch-row, .quiet-hours { border-bottom: 1px solid var(--border); }
.grid .switch-row + .switch-row { border-top: 0; }
.quiet-hours .switch-row { border: 0; }
.times { display: flex; gap: calc(var(--space) * 2); padding: 0 0 8px; }
.times label { flex-direction: row; align-items: center; gap: var(--space); }
</style>
