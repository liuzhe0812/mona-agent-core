// Product UI contract. No Agent state, DOM dependency, module downloads or runtime permissions.
export const UI_VERSION = 1;
export const SLOTS = Object.freeze([
  'settings', 'composer.actions', 'composer.mode', 'composer.dock', 'composer.commands',
  'status', 'right.pane', 'tool.view', 'presentation',
]);
const validId = value => typeof value === 'string' && /^[a-z][a-z0-9_.-]{0,95}$/.test(value);
export class UiRegistry {
  constructor(onError = () => {}) {
    this.slots = new Map(SLOTS.map(id => [id, new Map()])); this.listeners = new Map(); this.onError = onError;
  }
  register(owner, slot, { id, order = 0, value }) {
    const entries = this.slots.get(slot);
    if (!validId(owner) || !entries || !validId(id) || !Number.isSafeInteger(order) || Math.abs(order) > 10000 || value == null) throw new Error('无效的 UI 注册声明。');
    if (entries.has(id)) throw new Error(`重复的 UI 注册：${slot}/${id}`);
    if (entries.size >= 64) throw new Error(`UI 插槽容量已满：${slot}`);
    const entry = Object.freeze({ owner, id, order, value }); entries.set(id, entry); this.notify(slot);
    let disposed = false;
    return () => { if (disposed) return; disposed = true; if (entries.get(id) === entry) { entries.delete(id); this.notify(slot); } };
  }
  entries(slot) {
    if (!this.slots.has(slot)) throw new Error(`未知 UI 插槽：${slot}`);
    return [...this.slots.get(slot).values()].sort((a, b) => a.order - b.order || a.id.localeCompare(b.id));
  }
  resolve(slot, id) { return this.slots.get(slot)?.get(id)?.value; }
  subscribe(slot, fn) {
    if (!this.slots.has(slot) || typeof fn !== 'function') throw new Error('无效的插槽订阅。');
    let listeners = this.listeners.get(slot); if (!listeners) this.listeners.set(slot, listeners = new Set());
    listeners.add(fn); this.invoke(() => fn(this.entries(slot)));
    return () => { listeners.delete(fn); if (!listeners.size) this.listeners.delete(slot); };
  }
  notify(slot) { for (const fn of [...(this.listeners.get(slot) || [])]) this.invoke(() => fn(this.entries(slot))); }
  invoke(action, fallback = null) {
    try { return action(); } catch (error) {
      try { this.onError(error); } catch { /* A broken error view cannot poison the registry. */ }
      return fallback;
    }
  }
  removeOwner(owner) {
    for (const [slot, entries] of this.slots) {
      let changed = false; for (const [id, entry] of entries) if (entry.owner === owner) { entries.delete(id); changed = true; }
      if (changed) this.notify(slot);
    }
  }
}
// One browser shell, with explicitly owned contributions. Rendering never imports a URL from data.
export const uiRegistry = new UiRegistry(error => {
  if (typeof document !== 'undefined') document.dispatchEvent(new CustomEvent('mona:ui-error', { detail: error.message }));
});
