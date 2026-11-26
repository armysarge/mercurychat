<script setup lang="ts">
// The latest general broadcasts heard (owner, 26 Nov): who, when, how strong
// and the start of the message. A row opens the details (BroadcastDetail.vue).
// CQs and beacons are not here: they are in the Heard list. The text is from
// the air: it is only ever shown as text.
import { useI18n } from "vue-i18n";
import { shortUtc, snrText } from "../broadcastList";
import type { HeardBroadcast } from "../bindings/HeardBroadcast";
import { currentLocale } from "../i18n";
import { discovery } from "../store";

defineProps<{ title: string }>();
const { t } = useI18n();
const rowLabel = (b: HeardBroadcast) =>
  t("latest.rowAria", { callsign: b.callsign, date: shortUtc(b.atUtc, currentLocale()), snr: snrText(b.snr), message: b.message });
</script>

<template>
  <section class="latest-broadcasts" :aria-label="title">
    <h2 class="label">{{ title }}</h2>
    <p v-if="!discovery.broadcasts.length" class="muted small empty">{{ t("latest.empty") }}</p>
    <div v-else class="table">
      <div class="head small muted" aria-hidden="true">
        <span>{{ t("latest.callsign") }}</span>
        <span>{{ t("latest.date") }}</span>
        <span class="snr">{{ t("latest.snr") }}</span>
        <span>{{ t("latest.message") }}</span>
      </div>
      <ul>
        <li v-for="b in discovery.broadcasts" :key="b.id">
          <button type="button" class="row-button" aria-haspopup="dialog" :aria-label="rowLabel(b)" @click="discovery.opened = b">
            <b class="mono call">{{ b.callsign }}</b>
            <span class="mono small when">{{ shortUtc(b.atUtc, currentLocale()) }}</span>
            <span class="mono small snr">{{ snrText(b.snr) }}</span>
            <span class="message">{{ b.message }}</span>
          </button>
        </li>
      </ul>
    </div>
  </section>
</template>

<style scoped>
/* A container of its own: the layout follows the card's width, wherever it is. */
.latest-broadcasts { display: grid; grid-template-columns: minmax(0, 1fr); gap: var(--space); align-content: start; min-width: 0; container-type: inline-size; }
h2, p { margin: 0; }
ul { list-style: none; margin: 0; padding: 0; display: grid; grid-template-columns: minmax(0, 1fr); gap: 4px; }
.table { display: grid; grid-template-columns: minmax(0, 1fr); gap: 4px; }
/* Widths in em, so a larger font widens the columns with it. */
.head, .row-button {
  display: grid; align-items: baseline; gap: 0 12px; padding: 0 10px;
  grid-template-columns: 11em 9.5em 4.5em minmax(0, 1fr);
}
.head { font-size: calc(12px * var(--font-scale)); }
.head span { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.row-button { width: 100%; min-height: 36px; padding-block: 6px; justify-content: stretch; text-align: left; background: var(--surface); border-color: var(--border); }
.row-button > * { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.row-button .when, .row-button .snr { font-size: calc(12px * var(--font-scale)); }
.head span, .row-button > .when { color: var(--text-2); }
.snr { text-align: right; }
.row-button .snr { color: var(--accent); font-weight: 600; }
/* Narrow: the callsign and S/N, then the date, then the message. The callsign
   shares its line only with the S/N, so it is not cut to make room. */
@container (max-width: 560px) {
  .head { display: none; }
  .row-button { grid-template-columns: minmax(0, 1fr) auto; grid-template-areas: "call snr" "when when" "message message"; row-gap: 2px; }
  .row-button .call { grid-area: call; }
  .row-button .when { grid-area: when; }
  .row-button .snr { grid-area: snr; }
  .row-button .message { grid-area: message; }
}
</style>
