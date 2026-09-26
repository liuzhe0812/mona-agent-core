// Trusted product templates, not model-provided HTML. A remounted controller gets fresh nodes.
const templates = new Map();
export function claim(id) {
  const old = document.getElementById(id);
  if (!templates.has(id)) { if (!old) throw new Error(`UI 模板不存在：${id}`); templates.set(id, old.cloneNode(true)); }
  const node = templates.get(id).cloneNode(true);
  if (old) old.replaceWith(node); else document.body.append(node);
  return node;
}
export const element = (tag, className = '', value = '') => { const node = document.createElement(tag); node.className = className; node.textContent = value; return node; };
export function scoped(roots) { return selector => { for (const root of roots) { if (root.matches(selector)) return root; const found = root.querySelector(selector); if (found) return found; } return null; }; }
export function setting(ctx, id, panel, activate, order = 0) {
  const tab = claim(`settings-${id}-tab`);
  ctx.register('settings', { id, order, value: { tab, panel, activate } });
}
export function dialogDismiss(ctx, dialog, canDismiss = () => true) {
  for (const button of dialog.querySelectorAll('button[value="cancel"]')) ctx.listen(button, 'click', event => { event.preventDefault(); if (canDismiss()) dialog.close('cancel'); });
  ctx.listen(dialog, 'cancel', event => { if (!canDismiss()) event.preventDefault(); });
  ctx.listen(dialog, 'click', event => {
    if (event.target !== dialog || !canDismiss()) return;
    const r = dialog.getBoundingClientRect();
    if (event.clientX < r.left || event.clientX > r.right || event.clientY < r.top || event.clientY > r.bottom) dialog.close('cancel');
  });
}
