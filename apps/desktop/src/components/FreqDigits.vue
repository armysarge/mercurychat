<script setup lang="ts">
// The dial frequency in seven-segment digits (M3b). With a rig link each
// digit is a control: the wheel or ArrowUp/ArrowDown steps that digit (a
// burst is sent as one change, 150 ms after the last step), ArrowLeft and
// ArrowRight move between digits, Enter or a typed digit opens a field
// (Escape cancels). While the core would refuse a change the digits take no
// focus and say why. Without a rig link it reads exactly as before.
import { computed, nextTick, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { formatDigits, parseFreqInput, stepDigit } from "./freqStep";

const props = defineProps<{ hz: number | null; ageMs: number | null; editable: boolean; refusal: string }>();
const emit = defineEmits<{ tune: [hz: number] }>();
const { t } = useI18n();

const PLACES = ["1 Hz", "10 Hz", "100 Hz", "1 kHz", "10 kHz", "100 kHz", "1 MHz", "10 MHz"];
/** A burst of steps not yet sent, shown at once. */
const pending = ref<number | null>(null);
let sendTimer: ReturnType<typeof setTimeout> | undefined;
let holdTimer: ReturnType<typeof setTimeout> | undefined;
const shown = computed(() => pending.value ?? props.hz);
const text = computed(() => formatDigits(shown.value));
/** Each character, with its place (0 = 1 Hz) for digits. */
const cells = computed(() => {
  const chars = [...text.value];
  let place = chars.filter((c) => /\d/.test(c)).length;
  return chars.map((c) => (/\d/.test(c) ? { c, place: --place } : { c, place: null }));
});
const age = computed(() => (props.ageMs === null ? null : (props.ageMs / 1000).toFixed(1)));
const aria = computed(() => (shown.value === null ? t("strip.freqNone") : t("strip.freqAria", { mhz: (shown.value / 1e6).toFixed(3) })));
const title = computed(() => props.refusal || (age.value !== null ? t("telemetry.ageS", { s: age.value }) : t("strip.freqNone")));

// The rig has reached the frequency asked for: show the rig's own again.
watch(
  () => props.hz,
  (hz) => {
    if (pending.value !== null && hz === pending.value && !sendTimer) pending.value = null;
  },
);

function step(place: number, delta: 1 | -1) {
  if (props.refusal) return;
  pending.value = stepDigit(pending.value ?? props.hz ?? 0, place, delta);
  clearTimeout(sendTimer);
  clearTimeout(holdTimer);
  sendTimer = setTimeout(() => {
    sendTimer = undefined;
    if (pending.value !== null) emit("tune", pending.value);
    // Show the rig's reading again if it never reaches it.
    holdTimer = setTimeout(() => (pending.value = null), 2000);
  }, 150);
}

const editing = ref(false);
const draft = ref("");
const invalid = ref(false);
const input = ref<HTMLInputElement | null>(null);
const root = ref<HTMLElement | null>(null);
let returnPlace: number | null = null;

async function startEdit(initial: string, place: number | null) {
  if (props.refusal) return;
  returnPlace = place;
  draft.value = initial;
  invalid.value = false;
  editing.value = true;
  await nextTick();
  input.value?.focus();
  if (!initial) input.value?.select();
}
async function stopEdit() {
  editing.value = false;
  await nextTick();
  if (returnPlace !== null) root.value?.querySelector<HTMLElement>(`[data-place="${returnPlace}"]`)?.focus();
}
function commit() {
  const hz = parseFreqInput(draft.value);
  if (hz === null) {
    invalid.value = true;
    return;
  }
  emit("tune", hz);
  pending.value = hz;
  clearTimeout(holdTimer);
  holdTimer = setTimeout(() => (pending.value = null), 2000);
  void stopEdit();
}
function onInputKey(e: KeyboardEvent) {
  if (e.key === "Enter") {
    e.preventDefault();
    commit();
  } else if (e.key === "Escape") {
    e.preventDefault();
    e.stopPropagation();
    void stopEdit();
  }
}

function onDigitKey(e: KeyboardEvent, place: number) {
  if (e.key === "ArrowUp" || e.key === "ArrowDown") {
    e.preventDefault();
    step(place, e.key === "ArrowUp" ? 1 : -1);
  } else if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
    e.preventDefault();
    const next = place + (e.key === "ArrowLeft" ? 1 : -1);
    root.value?.querySelector<HTMLElement>(`[data-place="${next}"]`)?.focus();
  } else if (e.key === "Enter") {
    e.preventDefault();
    void startEdit(shown.value === null ? "" : (shown.value / 1e6).toFixed(6).replace(/0+$/, "").replace(/\.$/, ""), place);
  } else if (/^\d$/.test(e.key)) {
    e.preventDefault();
    void startEdit(e.key, place);
  }
}
function onWheel(e: WheelEvent, place: number) {
  if (e.deltaY !== 0) step(place, e.deltaY < 0 ? 1 : -1);
}
onUnmounted(() => {
  clearTimeout(sendTimer);
  clearTimeout(holdTimer);
});
</script>

<template>
  <span v-if="!editable" class="freq-digits big" role="img" :aria-label="aria" :title="title">{{ text }}</span>
  <input
    v-else-if="editing"
    ref="input"
    v-model="draft"
    class="freq-input mono"
    :aria-label="t('strip.freqInput')"
    :title="t('strip.freqInput')"
    :aria-invalid="invalid"
    inputmode="decimal"
    @keydown="onInputKey"
    @blur="stopEdit"
  />
  <span v-else ref="root" :class="['freq-digits', 'big', 'tunable', { refused: !!refusal }]" role="group" :aria-label="aria" :title="title">
    <template v-for="(cell, i) in cells" :key="i">
      <span
        v-if="cell.place !== null && shown !== null"
        class="digit"
        :data-place="cell.place"
        :tabindex="refusal ? -1 : 0"
        :aria-label="t('strip.digit', { place: PLACES[cell.place] ?? '' })"
        @keydown="onDigitKey($event, cell.place)"
        @wheel.prevent="onWheel($event, cell.place)"
        @dblclick="startEdit('', cell.place)"
      >{{ cell.c }}</span>
      <span v-else class="sep" aria-hidden="true">{{ cell.c }}</span>
    </template>
  </span>
</template>

<style scoped>
.tunable { display: inline-flex; }
.digit { cursor: ns-resize; border-radius: 2px; }
.digit:hover { background: var(--canvas); }
.digit:focus-visible { outline: 2px solid var(--focus); outline-offset: 1px; }
.refused .digit { cursor: not-allowed; }
.freq-input { width: 11ch; height: 26px; font-size: calc(18px * var(--font-scale)); padding: 0 4px; }
.freq-input[aria-invalid="true"] { border-color: var(--danger); }
</style>
