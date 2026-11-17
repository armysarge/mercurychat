<script setup lang="ts">
// Station information for the station in use (CHT-06): the operator's name,
// locator, QTH, rig, power, antenna, email and a few words about the
// station. Macros insert these into drafts; nothing here is ever sent by
// itself. The core checks every field (at most 128 characters, no control
// characters) and its errors are shown beside the field they name.
import { reactive, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { call, describeError } from "../api";
import type { AppError } from "../bindings/AppError";
import { activeProfile } from "../store";

const { t } = useI18n();
const FIELDS = ["name", "grid", "qth", "rig", "power", "antenna", "email", "about"] as const;
type Field = (typeof FIELDS)[number];
const form = reactive<Record<Field, string>>({ name: "", grid: "", qth: "", rig: "", power: "", antenna: "", email: "", about: "" });
const errors = ref<Record<string, string>>({});
const status = ref("");
const formError = ref("");
const busy = ref(false);

watch(
  () => activeProfile.value?.id,
  () => {
    const p = activeProfile.value;
    for (const f of FIELDS) form[f] = (p?.[f] as string | null | undefined) ?? "";
    errors.value = {};
    status.value = "";
  },
  { immediate: true },
);

async function save() {
  const p = activeProfile.value;
  if (!p) return;
  busy.value = true;
  errors.value = {};
  formError.value = "";
  status.value = "";
  const blank = (v: string) => (v.trim() ? v.trim() : null);
  try {
    await call({ type: "saveProfile", profile: { ...p, ...Object.fromEntries(FIELDS.map((f) => [f, blank(form[f])])) } });
    status.value = t("stationInfo.saved");
  } catch (e) {
    const err = e as AppError;
    if (err && typeof err === "object" && err.code === "validation") {
      for (const f of err.errors) errors.value[f.field] = f.message;
    } else {
      formError.value = describeError(e);
    }
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <form v-if="activeProfile" class="card stack station-info" aria-labelledby="h-station-info" @submit.prevent="save" @input="status = ''">
    <h2 id="h-station-info">{{ t("stationInfo.title", { callsign: activeProfile.callsign }) }}</h2>
    <p class="muted small">{{ t("stationInfo.intro") }}</p>
    <div class="fields">
      <label v-for="f in FIELDS" :key="f" :class="{ wide: f === 'about' }">
        {{ t(`stationInfo.${f}`) }}
        <input v-model="form[f]" :name="f" :class="{ mono: f === 'grid' }" :maxlength="f === 'name' ? 64 : 128" :type="f === 'email' ? 'email' : 'text'" autocomplete="off" spellcheck="false" />
        <span v-if="errors[f]" class="field-error">{{ errors[f] }}</span>
      </label>
    </div>
    <p class="muted small">{{ t("stationInfo.emailNote") }}</p>
    <p v-if="formError" class="err" role="alert">{{ formError }}</p>
    <div class="row">
      <button type="submit" class="primary" :disabled="busy">{{ t("stationInfo.save") }}</button>
      <span class="saved small ok" role="status">{{ status }}</span>
    </div>
  </form>
</template>

<style scoped>
.fields { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: var(--space) calc(var(--space) * 2); }
.fields input { width: 100%; }
.wide { grid-column: 1 / -1; }
</style>
