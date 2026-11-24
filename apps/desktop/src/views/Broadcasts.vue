<script setup lang="ts">
// Broadcasts (owner, 24 Nov): what went out to everyone from this station
// (CQs, beacons, messages), and the messages other stations broadcast that
// were heard. A broadcast carries no delivery receipt (DSC-02).
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { discovery, openPeer } from "../store";

const { t } = useI18n();
type Sent = { utc: string; kind: "message" | "cq" | "beacon"; text: string };

/** The activity log is newest first. */
const sent = computed<Sent[]>(() =>
  discovery.activity.flatMap((e): Sent[] => {
    if (e.kind === "message") return [{ utc: e.atUtc, kind: "message", text: e.text.replace(/^Message sent:\s*/i, "") }];
    if (e.kind === "broadcast" && /^(cq|beacon) transmitted/i.test(e.text)) {
      return [{ utc: e.atUtc, kind: e.text.toLowerCase().startsWith("cq") ? "cq" : "beacon", text: "" }];
    }
    return [];
  }),
);
const heard = computed(() =>
  discovery.heard
    .filter((h) => !!h.status && !!h.lastHeardUtc)
    .sort((a, b) => (b.lastHeardUtc ?? "").localeCompare(a.lastHeardUtc ?? "")),
);
const hm = (utc: string) => utc.slice(11, 16);
const signed = (db: number) => (db > 0 ? "+" : "") + db.toFixed(1);
</script>

<template>
  <section class="broadcasts" :aria-label="t('broadcasts.title')">
    <h1>{{ t("broadcasts.title") }}</h1>
    <p class="muted small">{{ t("broadcasts.note") }}</p>
    <div class="cols">
      <section class="card" aria-labelledby="h-sent">
        <h2 id="h-sent" class="label">{{ t("broadcasts.sent") }}</h2>
        <p v-if="!sent.length" class="muted small empty-sent">{{ t("broadcasts.noneSent") }}</p>
        <ul v-else class="list sent-list">
          <li v-for="(s, i) in sent" :key="i" :data-kind="s.kind">
            <span class="mono small muted">{{ hm(s.utc) }} UTC</span>
            <b class="kind">{{ t(`broadcasts.kind.${s.kind}`) }}</b>
            <span v-if="s.text" class="text">{{ s.text }}</span>
          </li>
        </ul>
      </section>
      <section class="card" aria-labelledby="h-heard">
        <h2 id="h-heard" class="label">{{ t("broadcasts.heard") }}</h2>
        <p v-if="!heard.length" class="muted small empty-heard">{{ t("broadcasts.noneHeard") }}</p>
        <ul v-else class="list heard-list">
          <li v-for="h in heard" :key="h.callsign" :data-callsign="h.callsign">
            <button class="quiet who" @click="openPeer(h.callsign)"><b class="mono">{{ h.callsign }}</b></button>
            <span class="text">“{{ h.status?.value }}”</span>
            <span class="mono small muted">{{ hm(h.lastHeardUtc ?? "") }} UTC</span>
            <span v-if="h.snr" class="mono small snr" :title="t('heard.snrTitle')">{{ t("heard.snr", { db: signed(h.snr.value) }) }}</span>
          </li>
        </ul>
      </section>
    </div>
  </section>
</template>

<style scoped>
.broadcasts { display: grid; gap: calc(var(--space) * 1.5); align-content: start; padding: calc(var(--space) * 2); overflow-y: auto; height: 100%; min-height: 0; }
h1 { margin: 0; font-size: calc(20px * var(--font-scale)); font-weight: 700; }
p { margin: 0; }
.cols { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: calc(var(--space) * 2); }
.card { display: grid; gap: var(--space); align-content: start; }
.label { margin: 0; font-size: calc(11px * var(--font-scale)); letter-spacing: 0.06em; text-transform: uppercase; color: var(--text-2); }
.list { list-style: none; margin: 0; padding: 0; display: grid; gap: 6px; }
.list li { display: flex; flex-wrap: wrap; align-items: baseline; gap: 4px 10px; padding: 8px 10px; border: 1px solid var(--border); border-radius: 10px; background: var(--surface); }
.text { overflow-wrap: anywhere; }
.who { padding: 0; min-height: 0; border: 0; background: transparent; }
.snr { color: var(--accent); font-weight: 600; }
</style>
