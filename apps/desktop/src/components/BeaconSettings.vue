<script setup lang="ts">
// Settings → Beacon (M3c B7): what the beacon carries besides the callsign
// and locator. Any field ticked makes it an extended beacon (MCB/1 kind 2);
// none is the plain one. Name and QTH come from Station information. The
// bytes left are the core's count for the current listen mode, live as the
// fields change; a selection that does not fit is not saved.
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { describeError } from "../api";
import { beaconContent, previewBeacon, saveBeaconFields } from "../extras";
import { activeProfile } from "../store";

const { t } = useI18n();
const name = ref(false);
const qth = ref(false);
const status = ref("");
const left = ref<number | null>(null);
const saved = ref("");
const error = ref("");

async function load() {
  error.value = "";
  saved.value = "";
  try {
    const c = await beaconContent();
    if (!c) return;
    name.value = c.name;
    qth.value = c.qth;
    status.value = c.status ?? "";
    left.value = c.bytesLeft;
  } catch (e) {
    error.value = describeError(e);
  }
}
onMounted(() => void load());
watch(() => activeProfile.value?.id, () => void load());

const statusOrNull = () => status.value.trim() || null;
/** The status line's own limit (40 bytes), counted apart from the whole beacon's room (owner, 24 Nov). */
const statusBytes = computed(() => new TextEncoder().encode(status.value.trim()).length);
const leftText = computed(() => {
  const n = left.value;
  if (n === null) return "";
  return n < 0 ? t("beaconSettings.over", { n: -n }) : t("beaconSettings.left", { n });
});
let seq = 0;
watch([name, qth, status], async () => {
  saved.value = "";
  const mine = ++seq;
  try {
    const n = await previewBeacon(name.value, qth.value, statusOrNull());
    if (mine === seq) left.value = n;
  } catch (e) {
    if (mine === seq) error.value = describeError(e);
  }
});

async function save() {
  error.value = "";
  saved.value = "";
  if (left.value !== null && left.value < 0) {
    error.value = t("beaconSettings.tooBig", { n: -left.value });
    return;
  }
  try {
    const c = await saveBeaconFields(name.value, qth.value, statusOrNull());
    if (c) left.value = c.bytesLeft;
    saved.value = t("beaconSettings.saved");
  } catch (e) {
    error.value = describeError(e);
  }
}
</script>

<template>
  <section v-if="activeProfile" class="card stack beacon-settings" aria-labelledby="h-beacon-settings">
    <h2 id="h-beacon-settings">{{ t("beaconSettings.title") }}</h2>
    <p class="muted small">{{ t("beaconSettings.intro") }}</p>
    <label class="check"><span class="switch"><input v-model="name" type="checkbox" role="switch" name="beacon-name" /><span class="track" aria-hidden="true"></span></span> {{ t("beaconSettings.name", { value: activeProfile.name ?? "" }) }}</label>
    <label class="check"><span class="switch"><input v-model="qth" type="checkbox" role="switch" name="beacon-qth" /><span class="track" aria-hidden="true"></span></span> {{ t("beaconSettings.qth", { value: activeProfile.qth ?? "" }) }}</label>
    <label>{{ t("beaconSettings.status") }} <input v-model="status" name="beacon-status" maxlength="40" autocomplete="off" /></label>
    <p :class="['small', 'status-bytes', statusBytes > 40 ? 'err' : 'muted']">{{ t("beaconSettings.statusBytes", { n: statusBytes, max: 40 }) }}</p>
    <p v-if="left !== null" :class="['small', 'beacon-left', left < 0 ? 'err' : 'muted']" role="status" :data-left="left">{{ leftText }}</p>
    <p class="muted small">{{ t("beaconSettings.longLeftOut") }}</p>
    <p v-if="error" class="err" role="alert">{{ error }}</p>
    <div class="row">
      <button class="primary beacon-save" :disabled="left !== null && left < 0" @click="save">{{ t("beaconSettings.save") }}</button>
      <span class="small ok" role="status">{{ saved }}</span>
    </div>
  </section>
</template>

<style scoped>
input[name="beacon-status"] { width: 100%; }
</style>
