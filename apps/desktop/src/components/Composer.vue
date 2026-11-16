<script setup lang="ts">
// The persistent composer (CHT-01): live UTF-8 byte count, Ctrl+Enter sends,
// Enter is a newline. The draft is kept per peer and never sent by itself.
import { computed, watch } from "vue";
import { useI18n } from "vue-i18n";
import { chat, MAX_BYTES, saveDraft, send } from "../store";

const { t } = useI18n();
const bytes = computed(() => new TextEncoder().encode(chat.draft).length);
const sendable = computed(() => !!chat.open && !!chat.draft.trim() && bytes.value <= MAX_BYTES);

watch(
  () => chat.draft,
  (text, old) => {
    if (old !== undefined) saveDraft(text);
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
      v-model="chat.draft"
      class="composer"
      rows="3"
      :disabled="!chat.open"
      :placeholder="chat.open ? t('composer.placeholder', { peer: chat.open }) : t('composer.openFirst')"
      :aria-label="chat.open ? t('composer.placeholder', { peer: chat.open }) : t('composer.aria')"
      @keydown.enter.ctrl.prevent="submit"
    ></textarea>
    <div class="row">
      <span :class="['mono', 'small', bytes > MAX_BYTES ? 'err' : 'muted']" aria-live="off">{{ t("composer.bytes", { used: bytes, max: MAX_BYTES }) }}</span>
      <span class="muted small hint">{{ t("composer.hint") }}</span>
      <span class="grow"></span>
      <button type="submit" class="primary" :disabled="!sendable">{{ t("composer.send") }}</button>
    </div>
    <p v-if="bytes > MAX_BYTES" class="field-error">{{ t("composer.tooLong", { max: MAX_BYTES }) }}</p>
  </form>
</template>

<style scoped>
.row { flex-wrap: wrap; }
.composer-box {
  margin: 0 calc(var(--space) * 2) calc(var(--space) * 2); padding: var(--space) calc(var(--space) * 1.5);
  border: 1px solid var(--border); border-radius: 13px; background: var(--surface); display: grid; gap: 6px;
}
.composer { width: 100%; border: 0; background: transparent; resize: vertical; min-height: 3.2em; padding: 4px; }
@media (max-height: 560px) {
  .composer { min-height: 2.4em; }
  .hint { display: none; }
}
</style>
