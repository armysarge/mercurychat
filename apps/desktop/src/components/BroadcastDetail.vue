<script setup lang="ts">
// One heard broadcast in full (owner, 26 Nov). Nothing here transmits. The
// message and callsign are from the air: shown as text, and the callsign is
// only what the message claims. Focus stays in the dialog and goes back to
// the row it came from.
import { computed, nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { longUtc, snrText } from "../broadcastList";
import { currentLocale } from "../i18n";
import { app, discovery, openPeer } from "../store";

const { t } = useI18n();
const UNKNOWN = "—";
const b = computed(() => discovery.opened);
const dialog = ref<HTMLElement | null>(null);
const closer = ref<HTMLButtonElement | null>(null);
let from: HTMLElement | null = null;

watch(
  () => b.value?.id,
  async (id, old) => {
    if (id !== undefined && old === undefined) {
      from = document.activeElement as HTMLElement | null;
      await nextTick();
      closer.value?.focus();
    } else if (id === undefined && from) {
      await nextTick();
      if (from.isConnected) from.focus();
      from = null;
    }
  },
);

const snr = computed(() => {
  const r = b.value?.snr;
  return r ? t("latest.snrValue", { db: snrText(r), source: r.source }) : UNKNOWN;
});
const freq = computed(() => {
  const f = b.value?.freq;
  if (!f) return UNKNOWN;
  const mhz = `${(f.value / 1e6).toFixed(3)} MHz`;
  return b.value?.band ? `${mhz} · ${b.value.band}` : mhz;
});

function close() {
  discovery.opened = null;
}

async function converse() {
  const call = b.value?.callsign;
  if (!call) return;
  from = null; // focus moves on to the conversation
  close();
  if (app.page !== "operate" && app.page !== "conversations") app.page = "operate";
  await openPeer(call);
}

/** Tab goes round the dialog's buttons. */
function trap(e: KeyboardEvent) {
  const buttons = [...(dialog.value?.querySelectorAll("button") ?? [])];
  const first = buttons[0];
  const last = buttons[buttons.length - 1];
  const here = document.activeElement;
  const outward = e.shiftKey ? here === first || here === dialog.value : here === last;
  if (!outward) return;
  e.preventDefault();
  (e.shiftKey ? last : first)?.focus();
}
</script>

<template>
  <div v-if="b" class="overlay" @click.self="close" @keydown.esc="close" @keydown.tab="trap">
    <section ref="dialog" class="broadcast-detail card stack" role="dialog" aria-modal="true" aria-labelledby="h-broadcast-detail" tabindex="-1">
      <h2 id="h-broadcast-detail">{{ t("latest.detailTitle", { callsign: b.callsign }) }}</h2>
      <p class="message">{{ b.message }}</p>
      <dl class="facts">
        <dt>{{ t("latest.dateLabel") }}</dt>
        <dd class="when">{{ longUtc(b.atUtc, currentLocale()) }}</dd>
        <dt>{{ t("latest.snrLabel") }}</dt>
        <dd class="snr">{{ snr }}</dd>
        <dt>{{ t("latest.mode") }}</dt>
        <dd class="mono mode">{{ b.mode ?? UNKNOWN }}</dd>
        <dt>{{ t("latest.freq") }}</dt>
        <dd class="freq">{{ freq }}</dd>
      </dl>
      <p class="muted small claimed">{{ t("latest.claimed") }}</p>
      <div class="actions">
        <button type="button" class="converse" @click="converse">{{ t("latest.open") }}</button>
        <button ref="closer" type="button" class="primary close" @click="close">{{ t("latest.close") }}</button>
      </div>
    </section>
  </div>
</template>

<style scoped>
.overlay { position: fixed; inset: 0; z-index: 50; display: grid; place-items: center; padding: 16px; background: var(--shadow); }
/* Taller than the window (a long message, a large font): it scrolls inside. */
.broadcast-detail { width: min(480px, 100%); max-height: 100%; overflow-y: auto; grid-template-columns: minmax(0, 1fr); }
h2, p { margin: 0; }
h2 { overflow-wrap: anywhere; }
.message {
  padding: 10px 12px; border: 1px solid var(--border); border-radius: 10px; background: var(--raised);
  white-space: pre-wrap; overflow-wrap: anywhere; user-select: text;
}
.facts { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 6px 16px; margin: 0; }
.facts dt { color: var(--text-2); }
.facts dd { margin: 0; overflow-wrap: anywhere; }
.actions { display: flex; flex-wrap: wrap; gap: 8px; justify-content: flex-end; }
</style>
