<script setup lang="ts">
// The QSY schedule (off by default): changes at UTC times to frequency-list
// entries. The core moves the rig only when a change is due and the station
// is idle; one it cannot make before the next is skipped and logged.
import { ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { call, describeError } from "../api";
import type { Schedule } from "../bindings/Schedule";
import { rigUi } from "../rig";
import Switch from "./Switch.vue";

const props = defineProps<{ initial: Schedule }>();
const { t } = useI18n();
const draft = ref<Schedule>({ enabled: false, slots: [] });
watch(
  () => props.initial,
  (s) => (draft.value = { enabled: s.enabled, slots: s.slots.map((x) => ({ ...x })) }),
  { immediate: true },
);
const error = ref("");
const saved = ref(false);

function add() {
  draft.value.slots.push({ startUtc: "00:00", entryId: rigUi.entries[0]?.id ?? "" });
}
async function save() {
  error.value = "";
  saved.value = false;
  try {
    await call({ type: "setSchedule", schedule: draft.value });
    saved.value = true;
  } catch (e) {
    error.value = describeError(e);
  }
}
</script>

<template>
  <section class="card stack schedule-editor" aria-labelledby="h-schedule">
    <h2 id="h-schedule">{{ t("schedule.title") }}</h2>
    <p class="muted small">{{ t("schedule.hint") }}</p>
    <label class="switch-row">
      <span>{{ t("schedule.enabled") }}</span>
      <Switch input-class="schedule-enabled" :checked="draft.enabled" @change="draft.enabled = ($event.target as HTMLInputElement).checked" />
    </label>
    <p v-if="!rigUi.entries.length" class="muted">{{ t("schedule.needEntries") }}</p>
    <ul class="slots">
      <li v-for="(s, i) in draft.slots" :key="i">
        <label>{{ t("schedule.time") }} <input v-model="s.startUtc" class="slot-time mono" type="time" step="60" /></label>
        <label>{{ t("schedule.entry") }}
          <select v-model="s.entryId" class="slot-entry">
            <option v-for="e in rigUi.entries" :key="e.id" :value="e.id">{{ e.name }}</option>
          </select>
        </label>
        <button class="quiet small" @click="draft.slots.splice(i, 1)">{{ t("schedule.remove") }}</button>
      </li>
    </ul>
    <div class="buttons">
      <button class="quiet" :disabled="!rigUi.entries.length" @click="add">{{ t("schedule.add") }}</button>
      <button class="schedule-save" @click="save">{{ t("schedule.save") }}</button>
      <span v-if="saved" class="ok small" role="status">{{ t("schedule.saved") }}</span>
    </div>
    <p v-if="error" class="err" role="alert">{{ error }}</p>
  </section>
</template>

<style scoped>
.slots { list-style: none; margin: 0; padding: 0; display: grid; gap: 4px; }
.slots li { display: flex; gap: var(--space); align-items: center; flex-wrap: wrap; }
.buttons { display: flex; gap: var(--space); align-items: center; }
</style>
