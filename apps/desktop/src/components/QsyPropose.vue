<script setup lang="ts">
// Propose a QSY to the session's peer (M3c B3): 1 to 5 frequency-list
// entries (entries on another band are the "band skip") and an optional
// note. The core checks this station's rig before anything is sent.
// Also used before a connection ("Suggest QSY", B5) with at most 3 entries.
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { describeTuning, proposeQsy, suggestQsy } from "../extras";
import { loadFrequencies, rigUi } from "../rig";

const props = defineProps<{ to?: string }>();
const emit = defineEmits<{ done: [sent: boolean] }>();
const { t } = useI18n();
onMounted(() => void loadFrequencies());

/** A suggestion (kind 3) carries 3 at most; a session proposal 5. */
const max = computed(() => (props.to ? 3 : 5));
const chosen = ref<string[]>([]);
const note = ref("");
const busy = ref(false);

function toggle(id: string, on: boolean) {
  chosen.value = on ? [...chosen.value, id].slice(0, max.value) : chosen.value.filter((x) => x !== id);
}

async function send() {
  busy.value = true;
  const ok = props.to ? await suggestQsy(props.to, chosen.value) : await proposeQsy(chosen.value, note.value.trim() || null);
  busy.value = false;
  if (ok) emit("done", true);
}
</script>

<template>
  <form class="stack qsy-propose" @submit.prevent="send">
    <p class="muted small">{{ t(props.to ? "qsy.suggestIntro" : "qsy.proposeIntro", { n: max, peer: props.to ?? "" }) }}</p>
    <ul class="entries">
      <li v-for="e in rigUi.entries" :key="e.id">
        <label>
          <input
            type="checkbox"
            name="qsy-entry"
            :value="e.id"
            :checked="chosen.includes(e.id)"
            :disabled="!chosen.includes(e.id) && chosen.length >= max"
            @change="toggle(e.id, ($event.target as HTMLInputElement).checked)"
          />
          <span>{{ e.name }}</span>
          <span class="mono small muted">{{ describeTuning(e) }}</span>
        </label>
      </li>
    </ul>
    <p v-if="!rigUi.entries.length" class="muted small">{{ t("qsy.noEntries") }}</p>
    <label v-if="!props.to">{{ t("qsy.note") }} <input v-model="note" name="qsy-note" maxlength="64" autocomplete="off" /></label>
    <div class="row">
      <button type="submit" class="primary qsy-send" :disabled="busy || !chosen.length">{{ t(props.to ? "qsy.suggestSend" : "qsy.proposeSend", chosen.length) }}</button>
      <button type="button" @click="emit('done', false)">{{ t("settings.cancel") }}</button>
    </div>
  </form>
</template>

<style scoped>
.entries { list-style: none; margin: 0; padding: 0; display: grid; gap: 2px; max-height: 220px; overflow-y: auto; }
.entries label { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
input[name="qsy-note"] { width: 100%; }
</style>
