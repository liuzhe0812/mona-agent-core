import { paneIcon, panelButton } from './pane-icons.mjs';

const $ = selector => document.querySelector(selector);

export class RightPane {
  constructor() {
    this.pane = $('#files-panel');
    this.tabsNode = $('#right-tabs');
    this.content = $('#right-content');
    this.empty = $('#right-empty');
    this.emptyActions = $('#right-empty-actions');
    this.menu = $('#right-add-menu');
    this.toggle = $('#files-toggle');
    this.closeButton = $('#files-close');
    this.addButton = $('#right-add');
    this.scrim = $('#files-scrim');
    this.resizer = $('#right-resizer');
    this.shell = document.querySelector('.shell');
    this.media = matchMedia('(max-width: 1180px)');
    this.tabs = new Map();
    this.active = new Map();
    this.actions = new Map();
    this.scope = 'none';
    this.recent = []; this.sequence = 0;
    this.overview = document.createElement('section'); this.overview.className = 'right-tab-overview'; this.overview.hidden = true; this.overview.setAttribute('aria-label', '标签概览');
    this.overviewSearch = document.createElement('input'); this.overviewSearch.type = 'search'; this.overviewSearch.placeholder = '搜索标签'; this.overviewSearch.setAttribute('aria-label', '搜索标签');
    this.overviewRows = document.createElement('div'); this.overviewRows.className = 'right-overview-rows';
    this.overview.append(this.overviewSearch, this.overviewRows); this.pane.append(this.overview);
    this.overviewSearch.addEventListener('input', () => this.renderOverview());
    this.overviewButton = panelButton('', () => this.setOverview(this.overview.hidden), 'icon-button right-overview-button');
    this.overviewButton.setAttribute('aria-label', '标签概览'); this.overviewButton.dataset.tooltip = '标签概览'; this.overviewButton.append(paneIcon('overview'));
    this.pane.querySelector('.right-pane-header').prepend(this.overviewButton);
    this.context = document.createElement('div'); this.context.className = 'right-pane-menu right-tab-context'; this.context.hidden = true; this.context.setAttribute('role', 'menu'); this.pane.append(this.context);
    this.notice = document.createElement('p'); this.notice.className = 'right-pane-notice'; this.notice.setAttribute('role', 'status'); this.pane.append(this.notice);
    this.closeButton.replaceChildren(paneIcon('collapse')); this.addButton.replaceChildren(paneIcon('add'));
    this.tabsNode.addEventListener('wheel', event => { if (this.tabsNode.scrollWidth > this.tabsNode.clientWidth && Math.abs(event.deltaY) > Math.abs(event.deltaX)) { this.tabsNode.scrollLeft += event.deltaY; event.preventDefault(); } }, { passive: false });
    window.addEventListener('resize', () => this.applyWidth(this.width));
    const tokens = getComputedStyle(document.documentElement);
    const px = (name, fallback) => Number.parseFloat(tokens.getPropertyValue(name)) || fallback;
    this.minWidth = px('--ui-right-pane-min-width', 320);
    this.maxWidth = px('--ui-right-pane-max-width', 720);
    this.width = px('--ui-right-pane-default-width', 420);
    try {
      const saved = localStorage.getItem('mona.web.right-pane.open');
      this.open = saved == null ? !this.media.matches : saved === '1' && !this.media.matches;
      const width = Number(localStorage.getItem('mona.web.right-pane.width'));
      if (Number.isFinite(width) && width > 0) this.width = this.clampWidth(width);
    } catch { this.open = !this.media.matches; }
    this.applyWidth(this.width);

    this.toggle.addEventListener('click', () => this.setOpen(!this.open, true));
    this.closeButton.addEventListener('click', () => this.setOpen(false, true));
    this.scrim.addEventListener('click', () => this.setOpen(false, true));
    this.addButton.addEventListener('click', event => { event.stopPropagation(); this.setMenu(this.menu.hidden); });
    document.addEventListener('pointerdown', event => {
      if (!this.menu.hidden && !this.menu.contains(event.target) && event.target !== this.addButton) this.setMenu(false);
      if (!this.context.contains(event.target)) this.context.hidden = true;
      if (!this.overview.contains(event.target) && !this.overviewButton.contains(event.target)) this.setOverview(false);
    });
    document.addEventListener('keydown', event => {
      const key = event.key.toLowerCase();
      if (document.querySelector('dialog[open]') || this.settingsVisible) return;
      if (event.key === 'Escape' && (!this.overview.hidden || !this.context.hidden)) { event.preventDefault(); this.setOverview(false); this.context.hidden = true; this.overviewButton.focus(); return; }
      if ((event.ctrlKey || event.metaKey) && event.shiftKey && key === 't' && this.pane.contains(event.target)) { event.preventDefault(); this.reopen(this.recent.find(tab => tab.scope === this.scope)); return; }
      if ((event.ctrlKey || event.metaKey) && !event.shiftKey && key === 'w' && this.pane.contains(event.target)) { event.preventDefault(); this.closeTab(this.active.get(this.scope)); return; }
      const shortcut = key === 'b' && event.altKey && (event.metaKey || event.ctrlKey);
      if (shortcut && !event.shiftKey) {
        event.preventDefault(); this.setOpen(!this.open, true); return;
      }
      if (event.key === 'Escape' && !this.menu.hidden) { event.preventDefault(); this.setMenu(false, true); return; }
      if (event.key === 'Escape' && this.open && this.media.matches) { event.preventDefault(); this.setOpen(false, true); }
      if (event.key === 'Tab' && this.open && this.media.matches && !this.pane.hidden) {
        const items = [...this.pane.querySelectorAll('button:not(:disabled),textarea:not(:disabled),input:not(:disabled),a[href],[tabindex="0"]')]
          .filter(item => item.getClientRects().length);
        if (!items.length) return;
        const first = items[0], last = items.at(-1), active = document.activeElement;
        if ((!event.shiftKey && active === last) || (event.shiftKey && active === first) || !this.pane.contains(active)) {
          event.preventDefault(); (event.shiftKey ? last : first).focus();
        }
      }
    });
    this.media.addEventListener('change', () => {
      if (this.media.matches) this.setOpen(false);
      else this.layout();
    });
    this.resizer.setAttribute('aria-valuemin', String(this.minWidth));
    this.resizer.setAttribute('aria-valuemax', String(this.maxWidth));
    this.resizer.addEventListener('pointerdown', event => {
      if (this.media.matches || this.pane.hidden) return;
      this.resizeGesture = { pointer: event.pointerId, startX: event.clientX, startWidth: this.width };
      this.resizer.setPointerCapture(event.pointerId); event.preventDefault();
    });
    this.resizer.addEventListener('pointermove', event => {
      if (!this.resizeGesture || event.pointerId !== this.resizeGesture.pointer) return;
      this.applyWidth(this.resizeGesture.startWidth - (event.clientX - this.resizeGesture.startX));
    });
    const endResize = event => {
      if (!this.resizeGesture || (event && event.pointerId !== this.resizeGesture.pointer)) return;
      this.resizeGesture = null;
      try { localStorage.setItem('mona.web.right-pane.width', String(this.width)); } catch {}
    };
    this.resizer.addEventListener('pointerup', endResize);
    this.resizer.addEventListener('pointercancel', endResize);
    this.resizer.addEventListener('keydown', event => {
      const delta = event.key === 'ArrowLeft' ? 16 : event.key === 'ArrowRight' ? -16 : 0;
      if (!delta) return;
      event.preventDefault(); this.applyWidth(this.width + delta);
      try { localStorage.setItem('mona.web.right-pane.width', String(this.width)); } catch {}
    });
    this.layout();
  }

  clampWidth(value) {
    const sidebar = $('#chat-sidebar');
    const occupied = sidebar && !sidebar.hidden && sidebar.getClientRects().length ? sidebar.getBoundingClientRect().width : 0;
    const maximum = Math.min(this.maxWidth, Math.max(this.minWidth, window.innerWidth - occupied - 430));
    return Math.min(maximum, Math.max(this.minWidth, Math.round(value)));
  }
  applyWidth(value) {
    this.width = this.clampWidth(value);
    this.shell.style.setProperty('--right-pane-width', `${this.width}px`);
    this.resizer.setAttribute('aria-valuenow', String(this.width));
  }

  registerAction({ id, label, icon = id, available = () => true, run }) {
    this.actions.set(id, { id, label, icon, available, run });
    this.renderActions();
  }

  setScope(scope) {
    const next = scope || 'none';
    if (next !== this.scope) { this.setMenu(false); this.setOverview(false); this.context.hidden = true; this.notice.textContent = ''; }
    this.scope = next; this.render();
  }

  clear() {
    for (const tab of this.tabs.values()) { this.runSafely(() => tab.onClose?.()); tab.node.remove(); }
    this.tabs.clear(); this.active.clear(); this.recent = []; this.scope = 'none'; this.render();
  }
  runSafely(action) { try { Promise.resolve(action()).catch(error => { this.notice.textContent = error?.message || '面板操作失败。'; }); } catch (error) { this.notice.textContent = error?.message || '面板操作失败。'; } }
  setOverview(show) { this.overview.hidden = !show; this.overviewButton.setAttribute('aria-expanded', String(show)); if (show) { this.setMenu(false); this.overviewSearch.value = ''; this.renderOverview(); this.overviewSearch.focus(); } }
  renderOverview() {
    const query = this.overviewSearch.value.toLocaleLowerCase(); this.overviewRows.replaceChildren();
    for (const [label, values, closed] of [['已打开', this.scopeTabs(), false], ['最近关闭', this.recent.filter(tab => tab.scope === this.scope), true]]) {
      const matches = values.filter(tab => (tab.title + ' ' + (tab.hint || '')).toLocaleLowerCase().includes(query));
      if (!matches.length) continue;
      const heading = document.createElement('strong'); heading.textContent = label; this.overviewRows.append(heading);
      for (const tab of matches) {
        const row = panelButton('', () => { this.setOverview(false); if (closed) this.reopen(tab); else this.activate(tab.id); }, 'right-overview-row');
        row.append(paneIcon(tab.kind), document.createTextNode(tab.title)); row.dataset.tooltip = tab.hint || tab.title; this.overviewRows.append(row);
      }
    }
    if (!this.overviewRows.children.length) this.overviewRows.textContent = '没有匹配的标签。';
  }
  reopen(tab) {
    if (!tab?.reopen) return;
    this.recent = this.recent.filter(item => item !== tab); this.runSafely(tab.reopen);
  }
  contextFor(tab) {
    this.setMenu(false); this.setOverview(false); this.context.replaceChildren(); this.context.hidden = false;
    for (const [label, action] of [['关闭标签', () => this.closeTab(tab.id)], ['关闭其他标签', () => this.scopeTabs().filter(item => item !== tab).forEach(item => this.closeTab(item.id))], ['关闭所有标签', () => this.scopeTabs().forEach(item => this.closeTab(item.id))]]) {
      const item = panelButton(label, () => { this.context.hidden = true; action(); }, 'right-pane-menu-item'); item.setAttribute('role', 'menuitem'); this.context.append(item);
    }
    this.context.firstElementChild?.focus();
  }
  reorder(source, target) {
    const values = [...this.tabs.values()], from = values.findIndex(t => t.scope === this.scope && t.id === source), to = values.findIndex(t => t.scope === this.scope && t.id === target);
    if (from < 0 || to < 0 || from === to) return;
    const [tab] = values.splice(from, 1); values.splice(to, 0, tab); this.tabs = new Map(values.map(t => [t.key, t])); this.render();
  }

  scopeTabs(scope = this.scope) {
    return [...this.tabs.values()].filter(tab => tab.scope === scope);
  }

  hasTab(id, scope = this.scope) {
    return this.tabs.has(`${scope}\0${id}`);
  }

  openTab({ id, title, kind, node, scope = this.scope, onClose, onActivate, reopen, hint = '' }) {
    if (!scope || scope === 'none') return null;
    const key = `${scope}\0${id}`;
    let tab = this.tabs.get(key);
    if (!tab) {
      if (this.tabs.size >= 48) { this.notice.textContent = '打开的标签已达 48 个，请先关闭不再使用的标签。'; this.runSafely(() => onClose?.()); return null; }
      tab = { key, id, scope, title, kind, node, onClose, onActivate, reopen, hint, domId: `right-tab-${++this.sequence}` };
      this.tabs.set(key, tab);
      node.id = `${tab.domId}-panel`; node.setAttribute('role', 'tabpanel'); node.setAttribute('aria-labelledby', tab.domId);
      this.content.append(node);
    } else {
      if (title) tab.title = title;
      if (node && tab.node !== node) { tab.node.replaceWith(node); tab.node = node; node.id = `${tab.domId}-panel`; node.setAttribute('role', 'tabpanel'); node.setAttribute('aria-labelledby', tab.domId); }
      if (onActivate) tab.onActivate = onActivate;
    }
    this.scope = scope; this.active.set(scope, id);
    this.setOpen(true);
    this.render();
    this.runSafely(() => tab.onActivate?.());
    return tab;
  }

  activate(id, scope = this.scope) {
    const tab = this.tabs.get(`${scope}\0${id}`);
    if (!tab) return;
    this.scope = scope; this.active.set(scope, id);
    this.setOpen(true);
    this.render();
    this.runSafely(() => tab.onActivate?.());
  }

  closeTab(id, scope = this.scope) {
    const key = `${scope}\0${id}`, tab = this.tabs.get(key);
    if (!tab) return;
    const peers = this.scopeTabs(scope), index = peers.findIndex(item => item.id === id);
    this.tabs.delete(key);
    this.runSafely(() => tab.onClose?.()); tab.node.remove();
    if (tab.reopen) { const { key, id, title, scope, hint, kind, reopen } = tab; this.recent = [{ key, id, title, scope, hint, kind, reopen }, ...this.recent.filter(t => t.key !== tab.key)].slice(0, 12); }
    if (this.active.get(scope) === id) {
      const rest = this.scopeTabs(scope);
      const next = rest[Math.min(index, Math.max(0, rest.length - 1))];
      if (next) this.active.set(scope, next.id); else this.active.delete(scope);
    }
    this.render();
  }

  clearScope(scope) {
    for (const tab of this.scopeTabs(scope)) {
      this.tabs.delete(tab.key);
      this.runSafely(() => tab.onClose?.()); tab.node.remove();
    }
    this.active.delete(scope);
    this.recent = this.recent.filter(tab => tab.scope !== scope);
    this.render();
  }

  setOpen(value, focus = false) {
    this.open = Boolean(value);
    this.setMenu(false); this.setOverview(false); this.context.hidden = true;
    try { localStorage.setItem('mona.web.right-pane.open', this.open ? '1' : '0'); } catch {}
    this.layout();
    if (focus) (this.open ? this.addButton : this.toggle).focus();
  }

  setMenu(value, focus = false) {
    this.menu.hidden = !value;
    this.addButton.setAttribute('aria-expanded', String(Boolean(value)));
    if (value) this.renderActions();
    if (focus && !value) this.addButton.focus();
  }

  renderActions() {
    if (!this.menu || !this.emptyActions) return;
    const build = className => {
      const fragment = document.createDocumentFragment();
      for (const action of this.actions.values()) {
        if (!action.available(this.scope)) continue;
        const button = document.createElement('button');
        button.type = 'button'; button.className = className; button.append(paneIcon(action.icon), document.createTextNode(action.label));
        if (className === 'right-pane-menu-item') button.setAttribute('role', 'menuitem');
        button.addEventListener('click', () => { this.setMenu(false); this.runSafely(() => action.run(this.scope)); });
        fragment.append(button);
      }
      return fragment;
    };
    this.menu.replaceChildren(build('right-pane-menu-item'));
    this.emptyActions.replaceChildren(build('right-empty-action'));
    this.addButton.hidden = this.menu.childElementCount === 0;
  }

  render() {
    this.renderActions();
    const tabs = this.scopeTabs();
    let activeId = this.active.get(this.scope);
    if (!tabs.some(tab => tab.id === activeId)) {
      activeId = tabs.at(-1)?.id;
      if (activeId) this.active.set(this.scope, activeId); else this.active.delete(this.scope);
    }
    this.tabsNode.replaceChildren();
    for (const tab of tabs) {
      const wrap = document.createElement('div'); wrap.className = 'right-tab-wrap'; wrap.dataset.active = String(tab.id === activeId);
      wrap.draggable = true; wrap.dataset.tabId = tab.id;
      wrap.addEventListener('dragstart', event => { this.dragging = tab.id; event.dataTransfer.effectAllowed = 'move'; event.dataTransfer.setData('text/plain', 'mona-tab'); });
      wrap.addEventListener('dragover', event => { if (this.dragging) { event.preventDefault(); event.dataTransfer.dropEffect = 'move'; } });
      wrap.addEventListener('drop', event => { if (this.dragging) { event.preventDefault(); this.reorder(this.dragging, tab.id); this.dragging = null; } });
      wrap.addEventListener('dragend', () => { this.dragging = null; });
      wrap.addEventListener('contextmenu', event => { event.preventDefault(); this.contextFor(tab); });
      const button = document.createElement('button'); button.type = 'button'; button.className = 'right-tab';
      button.setAttribute('role', 'tab'); button.setAttribute('aria-selected', String(tab.id === activeId));
      button.dataset.kind = tab.kind || ''; button.dataset.tabId = tab.id; button.id = tab.domId; button.dataset.tooltip = tab.hint || tab.title;
      button.setAttribute('aria-controls', `${tab.domId}-panel`); button.tabIndex = tab.id === activeId ? 0 : -1;
      const label = document.createElement('span'); label.textContent = tab.title; button.append(paneIcon(tab.kind), label);
      button.addEventListener('keydown', event => {
        const i = tabs.indexOf(tab), index = event.key === 'ArrowRight' ? (i + 1) % tabs.length : event.key === 'ArrowLeft' ? (i + tabs.length - 1) % tabs.length : event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : -1;
        if (index >= 0) { event.preventDefault(); this.activate(tabs[index].id); document.getElementById(tabs[index].domId)?.focus(); }
        if (event.key === 'Delete') { event.preventDefault(); this.closeTab(tab.id); }
      });
      button.addEventListener('click', () => this.activate(tab.id));
      button.addEventListener('auxclick', event => { if (event.button === 1) { event.preventDefault(); this.closeTab(tab.id); } });
      const close = document.createElement('button'); close.type = 'button'; close.className = 'right-tab-close';
      close.setAttribute('aria-label', `关闭“${tab.title}”`); close.textContent = '×';
      close.addEventListener('click', event => { event.stopPropagation(); this.closeTab(tab.id); });
      wrap.append(button, close); this.tabsNode.append(wrap);
    }
    const current = tabs.find(tab => tab.id === activeId);
    this.empty.hidden = Boolean(current);
    this.content.hidden = !current;
    for (const tab of this.tabs.values()) tab.node.hidden = tab !== current;
    this.overviewButton.hidden = !tabs.length && !this.recent.some(tab => tab.scope === this.scope);
    if (!this.overview.hidden) this.renderOverview();
    this.layout();
  }

  layout() {
    const show = this.open && !this.settingsVisible && this.scope !== 'none';
    this.pane.hidden = !show;
    this.resizer.hidden = !show || this.media.matches;
    this.scrim.hidden = !show || !this.media.matches;
    this.toggle.hidden = this.scope === 'none';
    this.toggle.setAttribute('aria-expanded', String(show));
    document.querySelector('.shell')?.classList.toggle('right-pane-open', show);
    if (this.media.matches && show) {
      this.pane.setAttribute('role', 'dialog'); this.pane.setAttribute('aria-modal', 'true');
    } else {
      this.pane.setAttribute('role', 'complementary'); this.pane.removeAttribute('aria-modal');
    }
  }

  settingsVisibility(value) {
    this.settingsVisible = value; this.setMenu(false); this.setOverview(false); this.context.hidden = true; this.layout();
  }
}
