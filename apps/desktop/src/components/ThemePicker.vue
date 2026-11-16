<script setup lang="ts">
// Appearance: theme presets with swatch previews, your own themes (start from
// any theme, change colours, contrast warning with auto-fix, export/import as
// a small JSON file), density and text size. Everything applies instantly.
import { computed, onBeforeUnmount, reactive, ref } from "vue";
import { useI18n } from "vue-i18n";
import { cleanName, exportTheme, parseThemeImport, type ImportFailure } from "../themes/apply.ts";
import { autoFix, checkTheme } from "../themes/contrast.ts";
import { LOGO_KEYS, PRESETS, SYSTEM_ID, TOKEN_KEYS, daylight, midnight, type Scheme, type TokenKey, type Tokens } from "../themes/presets.ts";
import {
  activeTheme, appearance, baseFor, deleteCustom, previewTheme, saveCustom, setDensity, setFontScale, setTheme, undoPreview,
} from "../themes/state.ts";

const { t } = useI18n();

type Swatch = { id: string; name: string; custom: boolean; tokens: Tokens; alt?: Tokens };
const swatches = computed<Swatch[]>(() => [
  { id: SYSTEM_ID, name: t("theme.system"), custom: false, tokens: midnight.tokens, alt: daylight.tokens },
  ...PRESETS.map((p) => ({ id: p.id, name: t(`theme.name.${p.id}`), custom: false, tokens: p.tokens })),
  ...appearance.custom.map((c) => ({ id: c.id, name: c.name, custom: true, tokens: c.tokens })),
]);
/** The dots shown on a card: surfaces, the two bubbles and the accent. */
const dots = (tk: Tokens) => ({ "--p-canvas": tk.canvas, "--p-surface": tk.surface, "--p-in": tk["bubble-in"], "--p-out": tk["bubble-out"], "--p-accent": tk.accent, "--p-text": tk.text, "--p-border": tk["border-strong"] });

// ---- editor

/** The colours listed in the editor ("more colours"); the accent has its own row. */
const EDITABLE: TokenKey[] = TOKEN_KEYS.filter((k) => k !== "scrim" && k !== "shadow" && k !== "accent" && !LOGO_KEYS.includes(k));
const editor = reactive({
  open: false,
  editingId: null as string | null,
  name: "",
  base: "midnight",
  scheme: "dark" as Scheme,
  tokens: { ...midnight.tokens } as Tokens,
  note: "",
});
const draft = computed(() => ({ aaa: false, scheme: editor.scheme, tokens: editor.tokens }));
const failures = computed(() => checkTheme(draft.value));
const preview = () => previewTheme({ scheme: editor.scheme, tokens: editor.tokens });

function startEditor(editingId: string | null) {
  const from = editingId ? appearance.custom.find((c) => c.id === editingId) : null;
  const source = from ?? activeTheme.value;
  editor.editingId = editingId;
  editor.name = from ? from.name : "";
  editor.base = from ? from.base : source.id;
  editor.scheme = source.scheme;
  editor.tokens = { ...source.tokens };
  editor.note = "";
  editor.open = true;
}
function setToken(key: TokenKey, value: string) {
  editor.tokens = { ...editor.tokens, [key]: value };
  editor.note = "";
  preview();
}
function fix() {
  const r = autoFix(draft.value);
  editor.tokens = r.tokens;
  editor.note = r.passes ? t("theme.contrast.fixed", { count: r.changed.length }) : t("theme.contrast.cannotFix");
  preview();
}
function cancel() {
  editor.open = false;
  undoPreview();
}
function save() {
  const name = cleanName(editor.name);
  if (!name) return;
  const id = saveCustom({ name, base: editor.base, scheme: editor.scheme, tokens: { ...editor.tokens } }, editor.editingId ?? undefined);
  if (id === null) {
    editor.note = t("theme.custom.tooMany");
    return;
  }
  editor.open = false;
}
onBeforeUnmount(() => editor.open && undoPreview());

const selectedCustom = computed(() => appearance.custom.find((c) => c.id === appearance.theme) ?? null);
const confirmDelete = ref(false);
function remove() {
  if (!selectedCustom.value) return;
  deleteCustom(selectedCustom.value.id);
  confirmDelete.value = false;
}

// ---- export and import

const fileText = ref("");
const fileNote = ref("");
const fileError = ref("");
function reason(f: ImportFailure): string {
  return f.reason === "badColour" ? t("theme.file.badColour", { token: f.detail ?? "" }) : t(`theme.file.${f.reason}`);
}
async function exportIt() {
  const c = selectedCustom.value;
  fileError.value = "";
  if (!c) return;
  fileText.value = exportTheme(c);
  try {
    await navigator.clipboard.writeText(fileText.value);
    fileNote.value = t("theme.file.exportedCopied");
  } catch {
    fileNote.value = t("theme.file.exported");
  }
}
function importIt() {
  fileNote.value = "";
  fileError.value = "";
  const r = parseThemeImport(fileText.value, baseFor);
  if (!r.ok) {
    fileError.value = reason(r);
    return;
  }
  if (saveCustom(r.theme) === null) {
    fileError.value = t("theme.custom.tooMany");
    return;
  }
  fileNote.value = t("theme.file.imported", { name: r.theme.name });
}
async function chooseFile(e: Event) {
  const input = e.target as HTMLInputElement;
  const f = input.files?.[0];
  input.value = "";
  fileNote.value = "";
  fileError.value = "";
  if (!f) return;
  if (f.size > 16 * 1024) {
    fileError.value = t("theme.file.tooLarge");
    return;
  }
  fileText.value = await f.text();
}
</script>

<template>
  <section class="card stack" aria-labelledby="h-theme">
    <h2 id="h-theme">{{ t("theme.title") }}</h2>
    <div class="swatches" role="radiogroup" :aria-label="t('theme.pickerAria')">
      <button
        v-for="s in swatches"
        :key="s.id"
        type="button"
        role="radio"
        class="swatch"
        :class="{ on: appearance.theme === s.id }"
        :data-theme-id="s.id"
        :aria-checked="appearance.theme === s.id"
        @click="setTheme(s.id)"
      >
        <span class="preview" :style="dots(s.tokens)" aria-hidden="true">
          <span class="rail"></span>
          <span class="pane"><i class="in"></i><i class="out"></i><i class="dot"></i></span>
          <span v-if="s.alt" class="half" :style="dots(s.alt)"><span class="pane"><i class="in"></i><i class="out"></i><i class="dot"></i></span></span>
        </span>
        <span class="sname">{{ s.name }}</span>
      </button>
    </div>
    <p v-if="appearance.theme === 'system'" class="muted small">{{ t("theme.systemHint") }}</p>
    <p v-if="appearance.saveError" class="err" role="alert">{{ t("theme.saveFailed") }}</p>

    <div class="row">
      <label>
        {{ t("theme.density.label") }}
        <select :value="appearance.density" @change="setDensity(($event.target as HTMLSelectElement).value === 'compact' ? 'compact' : 'normal')">
          <option value="normal">{{ t("theme.density.normal") }}</option>
          <option value="compact">{{ t("theme.density.compact") }}</option>
        </select>
      </label>
      <label class="scale">
        {{ t("theme.fontScale.label") }}: {{ t("theme.fontScale.value", { percent: appearance.fontScale }) }}
        <input type="range" min="90" max="130" step="5" :value="appearance.fontScale" @input="setFontScale(Number(($event.target as HTMLInputElement).value))" />
      </label>
      <button class="quiet" :disabled="appearance.fontScale === 100" @click="setFontScale(100)">{{ t("theme.fontScale.reset") }}</button>
    </div>

    <div class="row">
      <button class="customise" @click="startEditor(null)">{{ t("theme.custom.create") }}</button>
      <template v-if="selectedCustom">
        <button @click="startEditor(selectedCustom.id)">{{ t("theme.custom.edit") }}</button>
        <template v-if="confirmDelete">
          <button class="danger" @click="remove">{{ t("theme.custom.deleteConfirm", { name: selectedCustom.name }) }}</button>
          <button @click="confirmDelete = false">{{ t("settings.cancel") }}</button>
        </template>
        <button v-else class="quiet" @click="confirmDelete = true">{{ t("theme.custom.delete") }}</button>
      </template>
    </div>

    <form v-if="editor.open" class="editor stack" @submit.prevent="save">
      <h3>{{ t("theme.custom.editing", { base: editor.base }) }}</h3>
      <label>
        {{ t("theme.custom.name") }}
        <input v-model="editor.name" name="themeName" maxlength="40" required />
      </label>
      <label class="colour">
        {{ t("theme.token.accent") }}
        <input type="color" name="accent" :value="editor.tokens.accent" @input="setToken('accent', ($event.target as HTMLInputElement).value)" />
      </label>
      <details>
        <summary>{{ t("theme.custom.more") }}</summary>
        <div class="grid">
          <label v-for="k in EDITABLE" :key="k" class="colour">
            {{ t(`theme.token.${k}`) }}
            <input type="color" :name="k" :value="editor.tokens[k]" @input="setToken(k, ($event.target as HTMLInputElement).value)" />
          </label>
        </div>
      </details>
      <p v-if="failures.length === 0" class="ok small" role="status">{{ t("theme.contrast.ok") }}</p>
      <div v-else class="warnbox" role="alert">
        <p class="warn small">{{ t("theme.contrast.warn", { count: failures.length }) }}</p>
        <button type="button" class="autofix" @click="fix">{{ t("theme.contrast.autoFix") }}</button>
      </div>
      <p v-if="editor.note" class="muted small" role="status">{{ editor.note }}</p>
      <div class="row">
        <button type="submit" class="primary" :disabled="!cleanName(editor.name)">{{ t("theme.custom.save") }}</button>
        <button type="button" @click="cancel">{{ t("settings.cancel") }}</button>
      </div>
    </form>

    <details class="file">
      <summary>{{ t("theme.file.title") }}</summary>
      <div class="stack">
        <p class="muted small">{{ t("theme.file.hint") }}</p>
        <textarea v-model="fileText" name="themeFile" rows="5" class="mono small" spellcheck="false" :aria-label="t('theme.file.title')"></textarea>
        <div class="row">
          <button :disabled="!selectedCustom" @click="exportIt">{{ t("theme.file.export") }}</button>
          <button :disabled="!fileText.trim()" @click="importIt">{{ t("theme.file.import") }}</button>
          <label class="choose">
            <span class="btn">{{ t("theme.file.choose") }}</span>
            <input type="file" accept=".json,application/json" @change="chooseFile" />
          </label>
        </div>
        <p v-if="fileNote" class="ok small" role="status">{{ fileNote }}</p>
        <p v-if="fileError" class="err small" role="alert">{{ fileError }}</p>
      </div>
    </details>
  </section>
</template>

<style scoped>
.swatches { display: grid; grid-template-columns: repeat(auto-fill, minmax(124px, 1fr)); gap: var(--space); }
.swatch {
  display: grid; gap: 6px; padding: 8px; text-align: left; border-radius: 12px; background: var(--raised);
}
.swatch.on { border-color: var(--accent); outline: 2px solid var(--accent); outline-offset: 0; }
.preview {
  position: relative; display: flex; height: 52px; border-radius: 8px; overflow: hidden;
  background: var(--p-canvas); border: 1px solid var(--p-border);
}
.rail { width: 16px; background: var(--p-surface); }
.pane { flex: 1; display: grid; align-content: center; gap: 4px; padding: 6px; position: relative; }
.pane i { display: block; height: 8px; border-radius: 5px; }
.pane .in { width: 60%; background: var(--p-in); }
.pane .out { width: 50%; margin-left: auto; background: var(--p-out); }
.pane .dot { position: absolute; right: 6px; bottom: 6px; width: 8px; height: 8px; border-radius: 50%; background: var(--p-accent); }
.half { position: absolute; top: 0; right: 0; bottom: 0; width: 50%; display: flex; background: var(--p-canvas); }
.sname { font-size: calc(12px * var(--font-scale)); overflow-wrap: anywhere; }
.scale { min-width: 14em; }
.scale input { padding: 0; }
.editor { padding: calc(var(--space) * 1.5); border: 1px solid var(--border); border-radius: 12px; background: var(--raised); }
.colour { flex-direction: row; align-items: center; justify-content: space-between; gap: var(--space); }
.colour input[type="color"] { width: 44px; height: 28px; padding: 0; }
.grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 6px var(--space); margin-top: var(--space); }
.warnbox { display: flex; gap: var(--space); align-items: center; flex-wrap: wrap; }
.file summary, .editor summary { cursor: pointer; color: var(--text-2); }
.file textarea { width: 100%; resize: vertical; }
.choose { flex-direction: row; position: relative; cursor: pointer; }
.choose .btn {
  background: var(--raised); color: var(--text); border: 1px solid var(--border-strong); border-radius: 9px; padding: 6px 12px;
}
.choose input { position: absolute; inset: 0; opacity: 0; cursor: pointer; width: 100%; }
</style>
