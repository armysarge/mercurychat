<script setup lang="ts">
// The open conversation's messages, oldest first. Text only ({{ }}, never
// v-html). Every state is said in words with an icon, not by colour alone
// (docs/04 §3 Chat); there are no read ticks.
import { computed, nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { ChatMessage } from "../bindings/ChatMessage";
import { chat, loadOlder, retry } from "../store";

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
const day = (m: ChatMessage) => (m.receivedUtc ?? m.createdUtc).slice(0, 10);

const shown = computed(() => {
  const q = props.filter?.trim().toLowerCase();
  return q ? chat.messages.filter((m) => m.body.toLowerCase().includes(q)) : chat.messages;
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
      <li
        v-for="(m, i) in shown"
        :key="m.direction + m.id"
        :data-id="m.id"
        :data-state="m.state"
        :data-direction="m.direction"
        :class="['msg', m.direction]"
      >
        <div v-if="i === 0 || day(shown[i - 1]) !== day(m)" class="day label">{{ day(m) }}</div>
        <div class="who mono small">{{ m.direction === "out" ? t("transcript.you") : m.peer }} · {{ when(m) }}</div>
        <div class="body">{{ m.body }}</div>
        <div v-if="m.direction === 'out'" :class="['status', status(m).tone]">
          <span aria-hidden="true">{{ status(m).icon }}</span> {{ status(m).text }}
          <button v-if="m.state === 'uncertain' || m.state === 'failed'" class="quiet retry" @click="retry(m)">{{ t("transcript.retry") }}</button>
        </div>
        <details v-if="m.state === 'uncertain'" class="why small muted">
          <summary>{{ t("transcript.whyUncertain") }}</summary>
          {{ t("transcript.whyBody") }}
        </details>
      </li>
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
  background: var(--raised); padding: 10px 14px; border-radius: 3px 13px 13px 13px; line-height: 1.6;
}
.msg.out .body { background: color-mix(in srgb, var(--accent) 16%, var(--raised)); border-radius: 13px 3px 13px 13px; }
.status { font-size: 12px; margin-top: 4px; display: flex; flex-wrap: wrap; gap: 6px; justify-content: flex-end; align-items: center; }
.retry { padding: 2px 10px; font-size: 12px; }
.why { margin-top: 4px; text-align: left; max-width: 46ch; margin-left: auto; }
.why summary { cursor: pointer; }
</style>
