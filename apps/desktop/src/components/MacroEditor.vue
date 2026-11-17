<script setup lang="ts">
// The station's macros (CHT-06): add, change and delete. A macro is text with
// tags such as <CALL>, <MYCALL>, <NAME>, <QTH>, <LOC>, <RIG>, <PWR>, <ANT>,
// <ABOUT> and <UTC>; the composer's Macros menu expands it into the draft.
// The core checks every macro and its errors are shown beside the field.
import { onMounted, reactive, ref } from "vue";
import { useI18n } from "vue-i18n";
import { describeError } from "../api";
import type { AppError } from "../bindings/AppError";
import type { MacroDef } from "../bindings/MacroDef";
import { activeProfile, deleteMacro, loadMacros, macros, saveMacro } from "../store";

const { t } = useI18n();
onMounted(() => void loadMacros());

const TAGS = "<CALL> <MYCALL> <NAME> <QTH> <LOC> <RIG> <PWR> <ANT> <ABOUT> <UTC>";
/** The macro being edited (id "" for a new one), or null. */
const editing = ref<MacroDef | null>(null);
const draft = reactive({ name: "", body: "" });
const errors = ref<Record<string, string>>({});
const error = ref("");
const confirming = ref<string | null>(null);

function edit(m: MacroDef | null) {
  editing.value = m ?? { id: "", name: "", body: "" };
  draft.name = editing.value.name;
  draft.body = editing.value.body;
  errors.value = {};
  error.value = "";
}
async function save() {
  if (!editing.value) return;
  errors.value = {};
  error.value = "";
  try {
    await saveMacro({ id: editing.value.id, name: draft.name, body: draft.body });
    editing.value = null;
  } catch (e) {
    const err = e as AppError;
    if (err && typeof err === "object" && err.code === "validation") for (const f of err.errors) errors.value[f.field] = f.message;
    else error.value = describeError(e);
  }
}
async function remove(id: string) {
  confirming.value = null;
  try {
    await deleteMacro(id);
  } catch (e) {
    error.value = describeError(e);
  }
}
</script>

<template>
  <section v-if="activeProfile" class="card stack macro-editor" aria-labelledby="h-macros">
    <h2 id="h-macros">{{ t("macros.title") }}</h2>
    <p class="muted small">{{ t("macros.intro") }} <span class="mono">{{ TAGS }}</span></p>
    <ul class="list">
      <li v-for="m in macros.list" :key="m.id" class="macro">
        <div class="text">
          <b class="name">{{ m.name }}</b>
          <span class="muted small body">{{ m.body }}</span>
        </div>
        <template v-if="confirming === m.id">
          <button class="danger" @click="remove(m.id)">{{ t("macros.deleteConfirm", { name: m.name }) }}</button>
          <button @click="confirming = null">{{ t("settings.cancel") }}</button>
        </template>
        <template v-else>
          <button class="quiet" :aria-label="t('macros.editAria', { name: m.name })" @click="edit(m)">{{ t("macros.edit") }}</button>
          <button class="quiet" :aria-label="t('macros.deleteAria', { name: m.name })" @click="confirming = m.id">{{ t("macros.delete") }}</button>
        </template>
      </li>
    </ul>
    <p v-if="!macros.list.length" class="muted small">{{ t("macros.none") }}</p>
    <form v-if="editing" class="stack macro-edit" @submit.prevent="save">
      <label>{{ t("macros.name") }} <input v-model="draft.name" name="name" maxlength="32" autocomplete="off" /></label>
      <span v-if="errors.name" class="field-error">{{ errors.name }}</span>
      <label>{{ t("macros.body") }} <textarea v-model="draft.body" name="body" rows="3"></textarea></label>
      <span v-if="errors.body" class="field-error">{{ errors.body }}</span>
      <div class="row">
        <button type="submit" class="primary">{{ t("macros.save") }}</button>
        <button type="button" @click="editing = null">{{ t("settings.cancel") }}</button>
      </div>
    </form>
    <div v-else><button class="macro-add" @click="edit(null)">{{ t("macros.add") }}</button></div>
    <p v-if="error || macros.error" class="err" role="alert">{{ error || macros.error }}</p>
  </section>
</template>

<style scoped>
.list { list-style: none; margin: 0; padding: 0; display: grid; gap: 6px; }
.macro { display: flex; flex-wrap: wrap; align-items: center; gap: var(--space); padding: 6px 8px; border-radius: 8px; background: var(--raised); }
.text { display: grid; flex: 1; min-width: 12em; }
.body { white-space: pre-wrap; overflow-wrap: anywhere; }
textarea { width: 100%; resize: vertical; }
</style>
