<script setup lang="ts">
// Settings → Audio (M3b.1 design §6): Mercury's sound system, input, output
// and input channel, as Mercury lists them on its UI link (it needs -G).
// Apply sends them to Mercury, which restarts its audio and saves the choice
// only if that works; its verdict arrives in the activity log.
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { call, describeError } from "../api";
import type { AudioSettingsView } from "../bindings/AudioSettingsView";
import { activeProfile } from "../store";
import { builtinSystems } from "../builtin";

const { t } = useI18n();
const CHANNELS = ["left", "right", "stereo"] as const;

const view = ref<AudioSettingsView | null>(null);
/** The systems offered: for a built-in Mercury, this platform's two, preferred
 *  first (M5 §5.5), of those Mercury has; otherwise all Mercury lists. */
const systems = computed(() => {
  const listed = view.value?.systems ?? [];
  if (activeProfile.value?.modem.kind !== "builtIn") return listed;
  const ours = builtinSystems().filter((s) => listed.includes(s));
  return ours.length ? ours : listed;
});
const system = ref("");
const input = ref("");
const output = ref("");
const channel = ref("left");
const touched = ref(false);
const error = ref("");
const sent = ref(false);

async function refresh() {
  try {
    const r = await call({ type: "audioSettings" });
    if (r.type !== "audio") return;
    view.value = r;
    // The form follows Mercury until the operator changes something.
    if (!touched.value) {
      // A built-in Mercury's first start runs on "null": offer the preferred system.
      system.value = systems.value.includes(r.system) || !systems.value.length ? r.system : systems.value[0];
      input.value = r.input;
      output.value = r.output;
      channel.value = r.channel || "left";
    }
  } catch (e) {
    error.value = describeError(e);
  }
}
let timer: ReturnType<typeof setInterval> | null = null;
onMounted(() => {
  void refresh();
  timer = setInterval(refresh, 2000);
});
onUnmounted(() => {
  if (timer) clearInterval(timer);
});
watch(() => activeProfile.value?.id, () => {
  touched.value = false;
  void refresh();
});

async function apply() {
  error.value = "";
  sent.value = false;
  try {
    await call({ type: "setAudio", system: system.value, input: input.value, output: output.value, channel: channel.value });
    sent.value = true;
    touched.value = false;
  } catch (e) {
    error.value = describeError(e);
  }
}
</script>

<template>
  <section class="card stack audio-settings span-all" aria-labelledby="h-audio">
    <h2 id="h-audio">{{ t("audioSettings.title") }}</h2>
    <p v-if="!activeProfile" class="muted">{{ t("audioSettings.noStation") }}</p>
    <p v-else-if="!view?.available" class="muted audio-unavailable">{{ t("waterfall.howLost") }}</p>
    <template v-else>
      <div class="grid">
        <label v-if="systems.length > 1">
          {{ t("audioSettings.system") }}
          <select v-model="system" class="audio-system" @change="touched = true">
            <option v-for="s in systems" :key="s" :value="s">{{ s }}</option>
          </select>
        </label>
        <label>
          {{ t("audioSettings.input") }}
          <select v-model="input" class="audio-input" @change="touched = true">
            <option v-for="d in view.inputs" :key="d.id" :value="d.id">{{ d.name }}</option>
          </select>
        </label>
        <label>
          {{ t("audioSettings.output") }}
          <select v-model="output" class="audio-output" @change="touched = true">
            <option v-for="d in view.outputs" :key="d.id" :value="d.id">{{ d.name }}</option>
          </select>
        </label>
        <label>
          {{ t("audioSettings.channel") }}
          <select v-model="channel" class="audio-channel" @change="touched = true">
            <option v-for="c in CHANNELS" :key="c" :value="c">{{ t(`audioSettings.channelOpt.${c}`) }}</option>
          </select>
        </label>
      </div>
      <div class="buttons">
        <button class="audio-apply" @click="apply">{{ t("audioSettings.apply") }}</button>
        <span v-if="sent" class="small muted" role="status">{{ t("audioSettings.sent") }}</span>
      </div>
      <p v-if="view.audioOk === true" class="ok small">{{ t("audioSettings.ok") }}</p>
      <p v-else-if="view.audioOk === false" class="err small">{{ t("audioSettings.bad", { error: view.audioError ?? "?" }) }}</p>
      <p class="muted small">{{ t("audioSettings.hint") }}</p>
    </template>
    <p v-if="error" class="err" role="alert">{{ error }}</p>
  </section>
</template>

<style scoped>
.grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 16em), 1fr)); gap: 6px 12px; }
.grid label { display: grid; gap: 2px; }
.buttons { display: flex; gap: var(--space); align-items: center; }
</style>
