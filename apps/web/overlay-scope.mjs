// Shell-level modal ownership. Restoring one surface must not undo another owner's inert state.
const scopes = new WeakMap();
function scope(shell) {
  if (!scopes.has(shell)) scopes.set(shell, { base: new Map(), owners: new Map() });
  const state = scopes.get(shell);
  for (const node of shell.children) if (!state.base.has(node)) state.base.set(node, node.inert);
  for (const node of state.base.keys()) if (node.parentElement !== shell) state.base.delete(node);
  return state;
}
function apply(shell, state) {
  const active = [...state.owners.values()].at(-1);
  for (const node of shell.children) {
    const inert = Boolean(state.base.get(node)) || Boolean(active && !active.has(node));
    if (node.inert !== inert) node.inert = inert;
  }
}
export function setBaseInert(shell, node, value) { const state = scope(shell); state.base.set(node, Boolean(value)); apply(shell, state); }
export function setShellOverlay(shell, owner, nodes, active) {
  const state = scope(shell);
  if (active) state.owners.set(owner, new Set(nodes)); else state.owners.delete(owner);
  apply(shell, state);
}
export function focusable(root) {
  return [...root.querySelectorAll('button:not(:disabled),a[href],textarea:not(:disabled),input:not(:disabled),select:not(:disabled),iframe,[tabindex="0"]')]
    .filter(node => node.tabIndex >= 0 && node.getClientRects().length && !node.closest('[hidden],[inert]') && getComputedStyle(node).visibility !== 'hidden');
}
export function trapTab(event, root) {
  const items = focusable(root), current = document.activeElement;
  if (!items.length) return;
  if (!root.contains(current) || (!event.shiftKey && current === items.at(-1)) || (event.shiftKey && current === items[0])) { event.preventDefault(); (event.shiftKey ? items.at(-1) : items[0]).focus({ preventScroll: true }); }
}
