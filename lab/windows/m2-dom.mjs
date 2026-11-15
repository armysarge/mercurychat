// Fixed page expressions shared by the M2 window checks: what they read and
// press in the chat workspace. They run in the lab instance over DevTools.

export const pill = "document.querySelector('header .pill')?.textContent.trim()";
export const mode = "document.querySelector('header .mode')?.textContent.trim()";

export const button = (label) =>
  `[...document.querySelectorAll('button')].find(b => b.textContent.trim() === ${JSON.stringify(label)})`;

export const click = (label) =>
  `(() => { const b = ${button(label)}; if (!b) throw new Error('no button ' + ${JSON.stringify(label)}); if (b.disabled) throw new Error('disabled ' + ${JSON.stringify(label)}); b.click(); })()`;

export const setInput = (sel, value) => `(() => {
  const i = document.querySelector(${JSON.stringify(sel)});
  if (!i) throw new Error('no input ' + ${JSON.stringify(sel)});
  i.value = ${JSON.stringify(value)};
  i.dispatchEvent(new Event('input'));
  i.dispatchEvent(new Event('change'));
})()`;

/** Every transcript entry as {id, state, direction, status, body}; body is the text node, verbatim. */
export const transcript = `[...document.querySelectorAll('.transcript [data-id]')].map(e => ({
  id: e.dataset.id, state: e.dataset.state, direction: e.dataset.direction,
  status: e.querySelector('.status')?.textContent.trim() ?? '',
  body: e.querySelector('.body').textContent }))`;

/** Type `text` into the composer and press Ctrl+Enter; returns what the textarea held. */
export const compose = (text) => `(() => {
  const t = document.querySelector('textarea.composer');
  t.value = ${JSON.stringify(text)};
  t.dispatchEvent(new Event('input'));
  t.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', ctrlKey: true, bubbles: true }));
  return t.value;
})()`;
