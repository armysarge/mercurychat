<script setup lang="ts">
// The open conversation's messages, oldest first. Text only ({{ }}, never
// v-html). Every state is said in words with an icon, not by colour alone
// (docs/04 §3 Chat); there are no read ticks. System events are muted,
// non-bubble rows between the messages (docs/04 §8.1): a QSO summary when a
// session with this peer ended (frequency with its source, duration, bytes
// exchanged), and each CQ or beacon transmitted (from the activity log).
import { computed, nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { ChatMessage } from "../bindings/ChatMessage";
import type { Qso } from "../bindings/Qso";
import { chat, loadOlder, logbook, retry, snapshot, transmitted } from "../store";

const props = defineProps<{ filter?: string }>();
const { t } = useI18n();

// Each state's words are the keys transcript.status.<state>.
const STATUS: Record<string, { icon: string; tone: string }> = {
  queued: { icon: "◷", tone: "muted" },
  submitting: { icon: "↑", tone: "muted" },
  awaiting_receipt: { icon: "…", tone: "muted" },
  delivered: { icon: "✓", tone: "ok" },
  failed: { icon: "✕", tone: "err" },
  uncertain: { icon: "?", tone: "warn" },
  received: { icon: "", tone: "muted" },
};
const status = (m: ChatMessage) => {
  const s = STATUS[m.state];
  return s ? { ...s, text: t(`transcript.status.${m.state}`) } : { icon: "", tone: "muted", text: m.state };
};
const when = (m: ChatMessage) => (m.receivedUtc ?? m.createdUtc).slice(11, 16) + " UTC";

const shown = computed(() => {
  const q = props.filter?.trim().toLowerCase();
  return q ? chat.messages.filter((m) => m.body.toLowerCase().includes(q)) : chat.messages;
});

const at = (m: ChatMessage) => m.receivedUtc ?? m.createdUtc;
const utf8 = (s: string) => new TextEncoder().encode(s).length;

/** "3 min 5 s" / "42 s" between two UTC times. */
function duration(from: string, to: string): string {
  const s = Math.max(0, Math.round((Date.parse(to) - Date.parse(from)) / 1000));
  return s < 60 ? t("transcript.sys.seconds", { n: s }) : t("transcript.sys.minutes", { m: Math.floor(s / 60), s: s % 60 });
}

/** The session's summary: frequency with its source, how long, and the message bytes both ways. */
function summary(q: Qso): string {
  const end = q.endUtc ?? q.startUtc;
  const bytes = chat.messages
    .filter((m) => ["delivered", "received"].includes(m.state) && at(m) >= q.startUtc && at(m) <= end)
    .reduce((n, m) => n + utf8(m.body), 0);
  const freq = q.freq
    ? t("transcript.sys.freq", { mhz: (q.freq.value / 1e6).toFixed(3), source: q.freq.source })
    : t("transcript.sys.freqNone");
  return t("transcript.sys.sessionEnd", { freq, duration: duration(q.startUtc, end), bytes: t("transcript.sys.bytes", bytes) });
}

type Sys = { kind: "session-end" | "cq" | "beacon" | "qsy"; utc: string; key: string; text: string };
/** System rows for the open conversation, oldest first; none while filtering. */
const sysRows = computed<Sys[]>(() => {
  if (!chat.open || props.filter?.trim()) return [];
  const records = new Map(logbook.qsos.map((q) => [q.id, q]));
  const current = snapshot.value?.currentQso;
  if (current) records.set(current.id, current);
  const rows: Sys[] = [];
  for (const q of records.values()) {
    if (q.peer === chat.open && q.endUtc) rows.push({ kind: "session-end", utc: q.endUtc, key: `q${q.id}`, text: summary(q) });
  }
  transmitted.value.forEach((b, i) => {
    rows.push({ kind: b.kind, utc: b.utc, key: `b${i}`, text: t(`transcript.sys.${b.kind}`, { time: b.utc.slice(11, 16) }) });
  });
  // Older history is not loaded yet: its rows wait until it is.
  const first = chat.messages[0];
  const shownRows = chat.hasOlder && first ? rows.filter((r) => r.utc >= at(first)) : rows;
  return shownRows.sort((a, b) => a.utc.localeCompare(b.utc));
});

type Item = { sys: Sys; m?: undefined; day?: string } | { m: ChatMessage; sys?: undefined; day?: string };
/** Messages in their order with the system rows placed by time; `day` marks a new date. */
const items = computed<Item[]>(() => {
  const out: Item[] = [];
  const rows = [...sysRows.value];
  let last = "";
  const push = (item: Item, utc: string) => {
    const d = utc.slice(0, 10);
    out.push(d !== last ? { ...item, day: d } : item);
    last = d;
  };
  for (const m of shown.value) {
    while (rows.length && rows[0].utc < at(m)) push({ sys: rows[0] }, rows.shift()!.utc);
    push({ m }, at(m));
  }
  for (const r of rows) push({ sys: r }, r.utc);
  return out;
});

// Follow new messages only when the reader is already at the bottom.
const list = ref<HTMLElement | null>(null);
let atBottom = true;
function onScroll() {
  const el = list.value;
  if (el) atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
}
watch(
  () => chat.messages.length,
  async (n, old) => {
    const el = list.value;
    if (!el) return;
    const prepended = old !== undefined && n > old && chat.messages[0] !== undefined && el.scrollTop < 10 && !atBottom;
    await nextTick();
    if (atBottom && !prepended) el.scrollTop = el.scrollHeight;
  },
  { flush: "post" },
);
watch(
  () => chat.open,
  async () => {
    atBottom = true;
    await nextTick();
    if (list.value) list.value.scrollTop = list.value.scrollHeight;
  },
);
</script>

<template>
  <div ref="list" class="scroller" @scroll="onScroll">
    <div v-if="chat.hasOlder && !filter" class="older">
      <button class="quiet" @click="loadOlder">{{ t("transcript.loadOlder") }}</button>
    </div>
    <p v-if="!chat.open" class="empty muted">{{ t("transcript.noneOpen") }}</p>
    <p v-else-if="!shown.length" class="empty muted">{{ filter ? t("transcript.nothingMatches") : t("transcript.noMessages", { peer: chat.open }) }}</p>
    <ol class="transcript" :aria-label="t('transcript.messagesAria')" aria-live="polite" aria-relevant="additions">
      <template v-for="it in items" :key="it.sys ? it.sys.key : it.m.direction + it.m.id">
        <li v-if="it.sys" class="sys small muted" :data-kind="it.sys.kind">
          <div v-if="it.day" class="day label">{{ it.day }}</div>
          <span class="sys-line">{{ it.sys.text }}</span>
        </li>
        <li
          v-else
          :data-id="it.m.id"
          :data-state="it.m.state"
          :data-direction="it.m.direction"
          :class="['msg', it.m.direction]"
        >
          <div v-if="it.day" class="day label">{{ it.day }}</div>
          <div class="who mono small">{{ it.m.direction === "out" ? t("transcript.you") : it.m.peer }} · {{ when(it.m) }}</div>
          <div class="body">{{ it.m.body }}</div>
          <div v-if="it.m.direction === 'out'" :class="['status', status(it.m).tone]">
            <span aria-hidden="true">{{ status(it.m).icon }}</span> {{ status(it.m).text }}
            <button v-if="it.m.state === 'uncertain' || it.m.state === 'failed'" class="quiet retry" @click="retry(it.m)">{{ t("transcript.retry") }}</button>
          </div>
          <details v-if="it.m.state === 'uncertain'" class="why small muted">
            <summary>{{ t("transcript.whyUncertain") }}</summary>
            {{ t("transcript.whyBody") }}
          </details>
        </li>
      </template>
    </ol>
  </div>
</template>

<style scoped>
.scroller { overflow-y: auto; min-height: 0; padding: calc(var(--space) * 2) calc(var(--space) * 3); }
.older { text-align: center; margin-bottom: var(--space); }
.empty { text-align: center; margin-top: calc(var(--space) * 4); }
.transcript { list-style: none; margin: 0; padding: 0; display: grid; gap: calc(var(--space) * 1.5); }
.msg { max-width: 82%; }
.msg.out { margin-left: auto; text-align: right; }
.day { text-align: center; margin: var(--space) 0; }
.msg.out .day { margin-right: 0; }
.who { color: var(--text-2); margin-bottom: 4px; }
.body {
  display: inline-block; text-align: left; white-space: pre-wrap; overflow-wrap: anywhere;
  background: var(--bubble-in); padding: 10px 14px; border-radius: 3px 13px 13px 13px; line-height: 1.6;
}
.msg.out .body { background: var(--bubble-out); border-radius: 13px 3px 13px 13px; }
.status { font-size: calc(12px * var(--font-scale)); margin-top: 4px; display: flex; flex-wrap: wrap; gap: 6px; justify-content: flex-end; align-items: center; }
.retry { padding: 2px 10px; font-size: calc(12px * var(--font-scale)); }
.why { margin-top: 4px; text-align: left; max-width: 46ch; margin-left: auto; }
.why summary { cursor: pointer; }
.sys { text-align: center; }
.sys-line { display: inline-block; padding: 2px 12px; border-top: 1px solid var(--border); border-bottom: 1px solid var(--border); }
</style>
