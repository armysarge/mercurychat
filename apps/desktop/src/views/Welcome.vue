<script setup lang="ts">
// First use (docs/04 §3): callsign and modem. Nothing transmits from here;
// listening is a separate, explicit choice once the modem is attached.
import ProfileForms from "../components/ProfileForms.vue";
import { app } from "../store";
</script>

<template>
  <div class="welcome">
    <div class="intro stack">
      <h1>Set up your station</h1>
      <p class="muted">Choose how MercuryChat reaches a modem. You can add more stations later in Settings.</p>
    </div>
    <ProfileForms @done="app.page = 'operate'" />
  </div>
</template>

<style scoped>
.welcome { padding: calc(var(--space) * 4); display: grid; gap: calc(var(--space) * 3); max-width: 960px; overflow-y: auto; height: 100%; align-content: start; }
h1 { font-size: 22px; }
</style>
