<script setup lang="ts">
// The persistent composer (CHT-01): live UTF-8 byte count, Ctrl+Enter sends,
// Enter is a newline. The draft is kept per peer and never sent by itself.
// The box grows with its text, from one line up to about eight, then scrolls.
// A macro (or the station's email) goes in at the cursor, never sent by itself;
// the note under the box says how many bytes went in and which tags had no value.
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import MacroPicker from "./MacroPicker.vue";
import { chat, MAX_BYTES, pickFile, saveDraft, send, sendFileBlock } from "../store";

const { t } = useI18n();
const box = ref<HTMLTextAreaElement | null>(null);
const MAX_LINES = 8;

/** Fit the box to its text: one line up to MAX_LINES, scrolling beyond. */
function fit() {
  const el = box.value;
  if (!el) return;
  const cs = getComputedStyle(el);
  const line = parseFloat(cs.lineHeight) || parseFloat(cs.fontSize) * 1.25;
  const frame = parseFloat(cs.paddingTop) + parseFloat(cs.paddingBottom) + parseFloat(cs.borderTopWidth) + parseFloat(cs.borderBottomWidth);
  const cap = Math.round(line * MAX_LINES + frame);
  el.style.height = "auto";
  const want = el.scrollHeight + parseFloat(cs.borderTopWidth) + parseFloat(cs.borderBottomWidth);
  el.style.height = `${Math.min(want, cap)}px`;
  el.style.overflowY = want > cap ? "auto" : "hidden";
}
onMounted(() => {
  fit();
  window.addEventListener("resize", fit);
});
onUnmounted(() => window.removeEventListener("resize", fit));
const bytes = computed(() => new TextEncoder().encode(chat.draft).length);

/** What the last macro put in: its bytes and the tags it could not fill. */
const inserted = ref<{ bytes: number; missing: string[] } | null>(null);
watch(() => chat.open, () => (inserted.value = null));

/** Put `text` at the cursor (or over the selection) and leave the cursor after it.
 *  The station's email (`asEmail`) fills a macro's literal <EMAIL> tags instead, if any. */
async function insert(text: string, missing: string[], asEmail = false) {
  if (asEmail && chat.draft.includes("<EMAIL>")) {
    chat.draft = chat.draft.replaceAll("<EMAIL>", text);
    inserted.value = { bytes: new TextEncoder().encode(text).length, missing: [] };
    return;
  }
  const el = box.value;
  const at = el ? el.selectionStart : chat.draft.length;
  const to = el ? el.selectionEnd : chat.draft.length;
  chat.draft = chat.draft.slice(0, at) + text + chat.draft.slice(to);
  inserted.value = { bytes: new TextEncoder().encode(text).length, missing };
  await nextTick();
  if (el) {
    el.focus();
    el.setSelectionRange(at + text.length, at + text.length);
  }
}
const sendable = computed(() => !!chat.open && !!chat.draft.trim() && bytes.value <= MAX_BYTES);

watch(
  () => chat.draft,
  (text, old) => {
    if (old !== undefined) saveDraft(text);
    if (!text) inserted.value = null;
    void nextTick(fit);
  },
);

async function submit() {
  if (!sendable.value) return;
  const text = chat.draft;
  await send(text);
  if (chat.draft === text && !chat.error) chat.draft = "";
}
</script>

<template>
  <form class="composer-box" @submit.prevent="submit">
    <div class="compose-row">
    <div class="tools">
      <MacroPicker @insert="insert" />
      <button
        type="button"
        class="attach quiet icon-btn"
        :disabled="!chat.open || !!sendFileBlock"
        :title="sendFileBlock ?? t('composer.attach')"
        :aria-label="t('composer.attach')"
        @click="pickFile"
      >
        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M20 12.5 12.6 20a5 5 0 0 1-7.1-7.1l8-8a3.4 3.4 0 0 1 4.8 4.8l-8 8a1.8 1.8 0 0 1-2.5-2.5l7.2-7.2" /></svg>
      </button>
    </div>
    <textarea
      ref="box"
      v-model="chat.draft"
      class="composer"
      rows="1"
      :disabled="!chat.open"
      :placeholder="chat.open ? t('composer.placeholder', { peer: chat.open }) : t('composer.openFirst')"
      :aria-label="chat.open ? t('composer.placeholder', { peer: chat.open }) : t('composer.aria')"
      @keydown.enter.ctrl.prevent="submit"
    ></textarea>
    <button type="submit" class="primary send" :disabled="!sendable">
      <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 12 20 4l-4 16-4.5-6.5L4 12Zm7.5 1.5L20 4" /></svg>
      {{ t("composer.send") }}
    </button>
    </div>
    <div class="row below">
      <span :class="['mono', 'small', bytes > MAX_BYTES ? 'err' : 'muted']" aria-live="off">{{ t("composer.bytes", { used: bytes, max: MAX_BYTES }) }}</span>
      <span class="muted small hint">{{ t("composer.hint") }}</span>
    </div>
    <p v-if="inserted" :class="['macro-note', 'small', inserted.missing.length ? 'warn' : 'muted']" role="status">
      {{ t("macros.insertedBytes", inserted.bytes) }}
      <template v-if="inserted.missing.length">{{ t("macros.missing", { tags: inserted.missing.join(", ") }) }}</template>
    </p>
    <p v-if="bytes > MAX_BYTES" class="field-error">{{ t("composer.tooLong", { max: MAX_BYTES }) }}</p>
  </form>
</template>

<style scoped>
.row { flex-wrap: wrap; }
.composer-box {
  margin: 0 max(calc(var(--space) * 2), calc((100% - 760px) / 2)) calc(var(--space) * 2); padding: var(--space) calc(var(--space) * 1.5);
  border: 1px solid var(--border-strong); border-radius: 18px; background: var(--surface); display: grid; gap: 6px;
}
.composer-box:focus-within { border-color: color-mix(in srgb, var(--accent) 60%, var(--border-strong)); }
.composer { flex: 1; min-width: 0; border: 0; background: transparent; resize: none; padding: 8px 4px; line-height: 1.4; overflow-y: hidden; }
.compose-row { display: flex; align-items: flex-end; gap: 8px; }
.tools { display: flex; align-items: center; gap: 2px; padding-bottom: 2px; }
.icon-btn { min-height: 0; padding: 8px; border-color: transparent; }
.icon-btn svg, .send svg { width: 18px; height: 18px; fill: none; stroke: currentColor; stroke-width: 1.8; stroke-linecap: round; stroke-linejoin: round; }
.send { display: inline-flex; align-items: center; gap: 6px; border-radius: 12px; }
.below { gap: 12px; padding-left: 4px; }
@media (max-height: 560px) {
  .hint { display: none; }
}
</style>
