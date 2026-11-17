// MercuryChat is an application, not a web page: no reload, no inspector, no
// browser context menu. In a dev server run (`vite` / `tauri dev`) everything
// is left alone. The shell also builds without the webview's devtools feature
// (src-tauri/Cargo.toml); these guards cover the keys and the menu.
import { reactive } from "vue";

/** The small edit menu offered on editable fields and selected text. */
export const editMenu = reactive({
  open: false,
  x: 0,
  y: 0,
  canCut: false,
  canCopy: false,
  canPaste: false,
  target: null as HTMLElement | null,
});

const TEXT_INPUTS = new Set(["text", "search", "email", "url", "tel", "password", "number", ""]);

function editableAt(target: EventTarget | null): HTMLInputElement | HTMLTextAreaElement | HTMLElement | null {
  if (!(target instanceof HTMLElement)) return null;
  if (target instanceof HTMLTextAreaElement) return target;
  if (target instanceof HTMLInputElement) return TEXT_INPUTS.has(target.type) ? target : null;
  return target.isContentEditable ? target : null;
}

function selectedText(el: HTMLElement | null): string {
  if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) {
    return (el.selectionStart ?? 0) === (el.selectionEnd ?? 0) ? "" : el.value.slice(el.selectionStart ?? 0, el.selectionEnd ?? 0);
  }
  return window.getSelection()?.toString() ?? "";
}

function onContextMenu(e: MouseEvent) {
  e.preventDefault(); // never the browser's own menu (Reload, Inspect, Save as...)
  const field = editableAt(e.target);
  const selected = selectedText(field);
  if (!field && !selected) {
    editMenu.open = false;
    return;
  }
  const writable = !!field && !(field as HTMLInputElement).readOnly && !(field as HTMLInputElement).disabled;
  Object.assign(editMenu, {
    open: true,
    x: e.clientX,
    y: e.clientY,
    canCut: writable && !!selected,
    canCopy: !!selected,
    canPaste: writable,
    target: field,
  });
}

function onKey(e: KeyboardEvent) {
  const key = e.key.toLowerCase();
  const ctrl = e.ctrlKey || e.metaKey;
  const reload = e.key === "F5" || (ctrl && key === "r");
  const inspector = e.key === "F12" || (ctrl && e.shiftKey && ["i", "j", "c"].includes(key)) || (ctrl && key === "u");
  if (reload || inspector) {
    e.preventDefault();
    e.stopPropagation();
  }
}

export function installShellGuards() {
  if (import.meta.env.DEV) return;
  window.addEventListener("keydown", onKey, true);
  document.addEventListener("contextmenu", onContextMenu, true);
}
