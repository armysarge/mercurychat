<script setup lang="ts">
// First use (docs/04 §3): callsign and modem. Nothing transmits from here, but
// once a station is attached it answers incoming calls by itself, which means
// transmitting (D18): setup says so and offers to start in Receive only.
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import ProfileForms from "../components/ProfileForms.vue";
import { app } from "../store";

const { t } = useI18n();
const startReceiveOnly = ref(false);
</script>

<template>
  <div class="welcome">
    <div class="intro stack">
      <h1>{{ t("welcome.title") }}</h1>
      <p class="muted">{{ t("welcome.intro") }}</p>
      <p class="muted">{{ t("welcome.answersAuto") }}</p>
      <label class="start-rx">
        <input v-model="startReceiveOnly" type="checkbox" name="startReceiveOnly" />
        <span>{{ t("welcome.startReceiveOnly") }}</span>
      </label>
      <p class="muted small">{{ t("welcome.startReceiveOnlyNote") }}</p>
    </div>
    <ProfileForms :receive-only="startReceiveOnly" @done="app.page = 'operate'" />
  </div>
</template>

<style scoped>
.welcome { padding: calc(var(--space) * 4); display: grid; gap: calc(var(--space) * 3); max-width: 960px; overflow-y: auto; height: 100%; align-content: start; }
h1 { font-size: calc(22px * var(--font-scale)); }
.start-rx { flex-direction: row; align-items: center; gap: 8px; color: var(--text); font-size: calc(14px * var(--font-scale)); }
</style>
