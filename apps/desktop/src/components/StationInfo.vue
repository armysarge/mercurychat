<script setup lang="ts">
// Station information for the station in use (CHT-06): the operator's name,
// locator, QTH, rig, power, antenna, email and a few words about the
// station. Macros insert these into drafts; nothing here is ever sent by
// itself. The core checks every field (at most 128 characters, no control
// characters) and its errors are shown beside the field they name.
import { computed, reactive, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { call, describeError } from "../api";
import type { AppError } from "../bindings/AppError";
import type { ShareField } from "../bindings/ShareField";
import { SHARE_FIELDS, setShareSettings, shareSettings } from "../extras";
import { activeProfile } from "../store";

const { t } = useI18n();
const FIELDS = ["name", "grid", "qth", "rig", "power", "antenna", "email", "about"] as const;
type Field = (typeof FIELDS)[number];
const form = reactive<Record<Field, string>>({ name: "", grid: "", qth: "", rig: "", power: "", antenna: "", email: "", about: "" });
const errors = ref<Record<string, string>>({});
const status = ref("");
const formError = ref("");
const busy = ref(false);

/** Byte limits (M3c B10): what an answer to a peer may carry. The name has
 *  its own 64-character limit for the profile; an answer carries 32 bytes. */
const LIMITS: Partial<Record<Field, number>> = { name: 32, qth: 48, rig: 48, power: 16, antenna: 48, about: 256, email: 254 };
const bytes = (s: string) => new TextEncoder().encode(s.trim()).length;
/** A stored value over its limit is kept, but left out of answers (never cut). */
const over = computed(() => FIELDS.filter((f) => (LIMITS[f] ?? Infinity) < bytes(form[f])));

// Settings → Station → Share: what an automatic answer to a station-info request carries (B6).
const shared = ref<ShareField[]>([]);
const shareStatus = ref("");
const shareError = ref("");
async function loadShare() {
  shareStatus.value = "";
  try {
    shared.value = (await shareSettings()) ?? [];
  } catch (e) {
    shareError.value = describeError(e);
  }
}
function toggleShare(f: ShareField, on: boolean) {
  shared.value = on ? [...shared.value.filter((x) => x !== f), f] : shared.value.filter((x) => x !== f);
  shareStatus.value = "";
}
async function saveShare() {
  shareError.value = "";
  try {
    shared.value = (await setShareSettings(SHARE_FIELDS.filter((f) => shared.value.includes(f)))) ?? shared.value;
    shareStatus.value = t("stationInfo.share.saved");
  } catch (e) {
    shareError.value = describeError(e);
  }
}
const shareLabel = (f: ShareField) => t(`stationInfo.${f === "locator" ? "grid" : f}`);

watch(
  () => activeProfile.value?.id,
  () => {
    const p = activeProfile.value;
    for (const f of FIELDS) form[f] = (p?.[f] as string | null | undefined) ?? "";
    errors.value = {};
    status.value = "";
    void loadShare();
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
        <input v-model="form[f]" :name="f" :class="{ mono: f === 'grid' }" :maxlength="f === 'name' ? 64 : 256" :type="f === 'email' ? 'email' : 'text'" autocomplete="off" spellcheck="false" />
        <span v-if="LIMITS[f]" :class="['small', over.includes(f) ? 'warn over-limit' : 'muted']" :data-field="f">{{ t(over.includes(f) ? "stationInfo.overLimit" : "stationInfo.limit", { n: LIMITS[f] }) }}</span>
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
  <section v-if="activeProfile" class="card stack share" aria-labelledby="h-share">
    <h2 id="h-share">{{ t("stationInfo.share.title") }}</h2>
    <p class="muted small">{{ t("stationInfo.share.intro") }}</p>
    <div class="ticks">
      <label v-for="f in SHARE_FIELDS" :key="f">
        <span class="switch"><input type="checkbox" role="switch" :name="`share-${f}`" :checked="shared.includes(f)" @change="toggleShare(f, ($event.target as HTMLInputElement).checked)" /><span class="track" aria-hidden="true"></span></span>
        {{ shareLabel(f) }}
      </label>
    </div>
    <p class="muted small">{{ t("stationInfo.share.emailNever") }}</p>
    <p v-if="shareError" class="err" role="alert">{{ shareError }}</p>
    <div class="row">
      <button class="share-save" @click="saveShare">{{ t("stationInfo.share.save") }}</button>
      <span class="small ok share-status" role="status">{{ shareStatus }}</span>
    </div>
  </section>
</template>

<style scoped>
.fields { display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: var(--space) calc(var(--space) * 2); }
.fields input { width: 100%; }
.wide { grid-column: 1 / -1; }
.ticks { display: flex; flex-wrap: wrap; gap: 6px 16px; }
</style>
