<script setup lang="ts">
// An on/off switch (ui.css .switch): a real checkbox with role="switch", so
// keyboard, screen readers and a wrapping <label> work as for any checkbox.
// `inputClass` names the input for tests and styling; `warn` uses the warning
// colour when on (Receive only).
defineProps<{ checked: boolean; disabled?: boolean; inputClass?: string; warn?: boolean; label?: string }>();
const emit = defineEmits<{ change: [event: Event] }>();
</script>

<template>
  <span :class="['switch', { warn }]">
    <input type="checkbox" role="switch" :class="inputClass" :checked="checked" :disabled="disabled" :aria-label="label" @change="emit('change', $event)" />
    <span class="track" aria-hidden="true"></span>
  </span>
</template>
