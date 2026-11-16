<script setup lang="ts">
// Settings: Station (docs/04 §4). Profiles, and new ones from the same forms
// as first use.
import { ref } from "vue";
import { call, describeError } from "../api";
import type { StationProfile } from "../bindings/StationProfile";
import ProfileForms from "../components/ProfileForms.vue";
import { snapshot } from "../store";

const error = ref("");
const working = ref(false);

async function act(step: () => Promise<unknown>) {
  working.value = true;
  error.value = "";
  try {
    await step();
  } catch (e) {
    error.value = describeError(e);
  } finally {
    working.value = false;
  }
}
const select = (id: string | null) => act(() => call({ type: "selectProfile", id }));
/** The profile whose Delete is waiting for a yes (docs/04 §5: destructive
 *  actions are explicit). */
const confirming = ref<string | null>(null);
const remove = (id: string) => {
  confirming.value = null;
  return act(() => call({ type: "deleteProfile", id }));
};
const where = (p: StationProfile) => (p.modem.kind === "simulator" ? "simulator" : `${p.modem.host}:${p.modem.controlPort}`);
</script>

<template>
  <div class="settings">
    <h1>Settings · Station</h1>
    <section class="card stack" aria-labelledby="h-profiles">
      <h2 id="h-profiles">Station profiles</h2>
      <ul v-if="snapshot?.profiles.length" class="profiles">
        <li v-for="p in snapshot.profiles" :key="p.id" :class="{ active: p.id === snapshot.activeProfileId }">
          <span class="mono call">{{ p.callsign }}</span>
          <span class="muted small">{{ where(p) }}</span>
          <span v-if="p.id === snapshot.activeProfileId" class="ok small">in use</span>
          <span class="grow"></span>
          <template v-if="confirming === p.id">
            <p class="confirm small" role="alert">
              Delete {{ p.callsign }}?
              <template v-if="p.id === snapshot.activeProfileId">It is in use: any session ends at once. </template>
              Its message history stays on this computer but is no longer shown.
            </p>
            <button class="danger" :disabled="working" @click="remove(p.id)">Delete {{ p.callsign }}</button>
            <button :disabled="working" @click="confirming = null">Cancel</button>
          </template>
          <template v-else>
            <button v-if="p.id === snapshot.activeProfileId" :disabled="working" @click="select(null)">Stop using</button>
            <button v-else :disabled="working" @click="select(p.id)">Use</button>
            <button class="quiet" :disabled="working" :aria-label="'Delete ' + p.callsign" @click="confirming = p.id">Delete</button>
          </template>
        </li>
      </ul>
      <p v-else class="muted">No station profile yet.</p>
      <p v-if="error" class="err" role="alert">{{ error }}</p>
    </section>
    <h2>Add a station</h2>
    <ProfileForms />
  </div>
</template>

<style scoped>
.settings { padding: calc(var(--space) * 3); display: grid; gap: calc(var(--space) * 2); overflow-y: auto; height: 100%; align-content: start; }
h1 { font-size: 20px; }
.profiles { list-style: none; margin: 0; padding: 0; display: grid; gap: 6px; }
.profiles li { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space); padding: 6px 8px; border-radius: 8px; background: var(--raised); }
.profiles li.active { outline: 1px solid var(--success); }
.call { font-weight: 650; }
.confirm { flex-basis: 100%; color: var(--warning); }
</style>
