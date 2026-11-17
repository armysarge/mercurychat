<script setup lang="ts">
// The persistent composer (CHT-01): live UTF-8 byte count, Ctrl+Enter sends,
// Enter is a newline. The draft is kept per peer and never sent by itself.
// The box grows with its text, from one line up to about eight, then scrolls.
// A macro (or the station's email) goes in at the cursor, never sent by itself;
// the note under the box says how many bytes went in and which tags had no value.
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import MacroPicker from "./MacroPicker.vue";
import { chat, MAX_BYTES, saveDraft, send } from "../store";

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
    <div class="row">
      <MacroPicker @insert="insert" />
      <span :class="['mono', 'small', bytes > MAX_BYTES ? 'err' : 'muted']" aria-live="off">{{ t("composer.bytes", { used: bytes, max: MAX_BYTES }) }}</span>
      <span class="muted small hint">{{ t("composer.hint") }}</span>
      <span class="grow"></span>
      <button type="submit" class="primary" :disabled="!sendable">{{ t("composer.send") }}</button>
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
  margin: 0 calc(var(--space) * 2) calc(var(--space) * 2); padding: var(--space) calc(var(--space) * 1.5);
  border: 1px solid var(--border-strong); border-radius: 13px; background: var(--surface); display: grid; gap: 6px;
}
.composer { width: 100%; border: 0; background: transparent; resize: none; padding: 4px; line-height: 1.4; overflow-y: hidden; }
@media (max-height: 560px) {
  .hint { display: none; }
}
</style>
