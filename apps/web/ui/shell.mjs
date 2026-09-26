import { ComposerCommandMenu } from './command-menu.mjs';

// A finite set of host-owned insertion points; arbitrary selectors/HTML are not a protocol.
export class UiShell {
  constructor(registry, pane, onError = () => {}) {
    this.registry = registry; this.pane = pane; this.onError = onError; this.cleanups = []; this.settingsKey = null; this.settingsVisible = false;
    this.settings = new Map(); this.ownedNodes = new Map();
    for (const node of document.querySelectorAll('#settings-page > .settings-content, .settings-sidebar nav > button')) node.hidden = true;
    this.cleanups.push(registry.subscribe('settings', entries => this.renderSettings(entries)));
    for (const [slot, selector] of [['composer.actions','#composer-ui-actions'],['composer.mode','#composer-ui-modes'],['composer.dock','#composer-ui-dock'],['status','#conversation-status-slots']]) {
      this.cleanups.push(registry.subscribe(slot, entries => this.renderNodes(slot, selector, entries)));
    }
    const commandButton = document.getElementById('composer-add'), commandRoot = document.getElementById('composer-command-menu');
    if (commandButton && commandRoot) {
      this.commandMenu = new ComposerCommandMenu(registry, commandButton, commandRoot, onError);
      this.cleanups.push(() => this.commandMenu.dispose());
    }
    this.right = new Map();
    this.cleanups.push(registry.subscribe('right.pane', entries => this.renderActions(entries)));
  }
  safe(action) {
    try { return action(); } catch (error) {
      try { this.onError(error.message); } catch { /* Keep unrelated slots usable. */ }
    }
  }
  renderNodes(slot, selector, entries) {
    const root = document.querySelector(selector); if (!root) throw new Error(`缺少 UI 插槽 ${slot}`);
    const next = new Set();
    for (const entry of entries) this.safe(() => {
      if (!(entry.value instanceof HTMLElement) || next.has(entry.value)) throw new Error(`UI 插槽 ${slot}/${entry.id} 需要独立 HTMLElement`);
      next.add(entry.value);
    });
    for (const node of this.ownedNodes.get(slot) || []) if (!next.has(node)) node.remove();
    [...next].forEach((node, index) => {
      if (root.children[index] !== node) root.insertBefore(node, root.children[index] || null);
    });
    this.ownedNodes.set(slot, next); root.hidden = !next.size;
  }
  renderActions(entries) {
    const valid = [];
    for (const entry of entries) this.safe(() => {
      if (typeof entry.value.run !== 'function' || (entry.value.available != null && typeof entry.value.available !== 'function')) throw new Error(`右侧插槽 ${entry.id} 缺少有效操作`);
      valid.push(entry);
    });
    if (valid.length === this.right.size && valid.every((entry, index) => [...this.right.values()][index]?.entry === entry)) return;
    for (const record of this.right.values()) this.safe(record.dispose);
    this.right.clear();
    for (const entry of valid) this.safe(() => {
      const dispose = this.pane.registerAction({ ...entry.value, id: entry.id, owner: entry.owner,
        available: (...args) => this.safe(() => entry.value.available?.(...args) ?? true) ?? false,
        run: (...args) => Promise.resolve().then(() => entry.value.run(...args)).catch(error => this.safe(() => { throw error; })),
      });
      this.right.set(entry.id, { entry, dispose });
    });
  }
  renderSettings(entries) {
    const next = new Map(), nodes = new Set(), previousKey = this.settingsKey;
    const nav = document.querySelector('.settings-sidebar nav'), page = document.getElementById('settings-page');
    for (const entry of entries) this.safe(() => {
      const { tab, panel, activate } = entry.value;
      if (!(tab instanceof HTMLElement) || !(panel instanceof HTMLElement) || tab === panel || nodes.has(tab) || nodes.has(panel)
          || (activate != null && typeof activate !== 'function')) throw new Error(`设置插槽 ${entry.id} 需要独立按钮、面板与有效回调`);
      nodes.add(tab); nodes.add(panel);
      const old = this.settings.get(entry.id);
      next.set(entry.id, old?.entry === entry ? old : { ...entry, entry });
    });
    for (const [id, old] of this.settings) if (next.get(id) !== old) {
      old.removeListener?.();
      for (const node of [old.value.tab, old.value.panel]) if (!nodes.has(node)) node.remove();
    }
    let index = 0;
    for (const entry of next.values()) {
      const { tab, panel } = entry.value;
      if (nav.children[index] !== tab) nav.insertBefore(tab, nav.children[index] || null);
      index++;
      if (panel.parentElement !== page) page.append(panel);
      tab.hidden = false;
      if (!entry.removeListener) {
        const listener = () => this.select(entry.id, true); tab.addEventListener('click', listener);
        entry.removeListener = () => tab.removeEventListener('click', listener);
      }
    }
    this.settings = next;
    this.select(next.has(previousKey) ? previousKey : next.keys().next().value);
    if (previousKey !== this.settingsKey && this.settingsVisible) this.activate();
  }
  select(id, activate = false) {
    if (!this.settings.has(id)) id = this.settings.keys().next().value;
    this.settingsKey = id || null;
    for (const [key, entry] of this.settings) {
      const active = key === id; entry.value.tab.classList.toggle('active', active);
      if (active) entry.value.tab.setAttribute('aria-current', 'page'); else entry.value.tab.removeAttribute('aria-current');
      entry.value.panel.hidden = !active;
    }
    if (activate && this.settingsVisible) this.activate();
  }
  activate() {
    const entry = this.settings.get(this.settingsKey); if (!entry) return;
    this.safe(() => Promise.resolve(entry.value.activate?.()).catch(error => {
      if (this.settings.get(entry.id) === entry && this.settingsKey === entry.id) this.safe(() => { throw error; });
    }));
  }
  show(id) { this.settingsVisible = true; this.select(id || this.settingsKey); this.activate(); }
  hide() { this.settingsVisible = false; }
  ownedPane(owner) {
    const pane = this.pane;
    return new Proxy(pane, { get: (target, key) => {
      if (key === 'registerAction') return action => { const dispose = this.registry.register(owner, 'right.pane', { id: action.id, value: action }); return dispose; };
      if (key === 'openTab') return options => pane.openTab({ ...options, owner });
      if (key === 'clear') return () => pane.releaseOwner(owner);
      const value = Reflect.get(target, key); return typeof value === 'function' ? value.bind(target) : value;
    } });
  }
  releaseOwner(owner) { this.pane.releaseOwner(owner); }
  dispose() {
    for (const cleanup of this.cleanups.splice(0).reverse()) this.safe(cleanup);
    this.settingsVisible = false; this.renderSettings([]);
    for (const nodes of this.ownedNodes.values()) for (const node of nodes) node.remove();
    this.ownedNodes.clear();
    for (const record of this.right.values()) this.safe(record.dispose);
    this.right.clear();
  }
}
