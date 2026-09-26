import { paneIcon, panelButton } from './pane-icons.mjs';
import { PaneState } from './pane-state.mjs';
import { PaneLayout } from './pane-layout.mjs';
import { PaneMenu, paneButton } from './pane-controls.mjs';
import { resolveColumns } from './layout-geometry.mjs';
import { setShellOverlay, trapTab } from './overlay-scope.mjs';

const $ = selector => document.querySelector(selector);

export class RightPane {
  constructor() {
    this.pane = $('#files-panel');
    this.tabsNode = $('#right-tabs');
    this.content = $('#right-content');
    this.empty = $('#right-empty'); this.empty.querySelector('strong')?.remove();
    this.emptyActions = $('#right-empty-actions');
    this.menu = $('#right-add-menu');
    this.toggle = $('#files-toggle');
    this.closeButton = $('#files-close');
    this.addButton = $('#right-add');
    this.scrim = $('#files-scrim');
    this.resizer = $('#right-resizer');
    this.shell = document.querySelector('.shell');
    this.media = matchMedia('(max-width: 680px)');
    this.tabs = new Map();
    this.active = new Map();
    this.actions = new Map();
    this.scope = 'none';
    this.presentations = new Map(); this.layouts = new Map(); this.fullscreen = false;
    this.headers = [this.pane.querySelector('.right-pane-header')];
    this.headers[0].dataset.pane = '0'; this.tabsNode.dataset.pane = '0';
    this.stripRegion = document.createElement('div'); this.stripRegion.className = 'right-pane-strips'; this.headers[0].before(this.stripRegion); this.stripRegion.append(this.headers[0]);
    const second = document.createElement('header'); second.className = 'right-pane-header'; second.dataset.pane = '1'; second.hidden = true;
    this.secondaryTabs = document.createElement('div'); this.secondaryTabs.id = 'right-tabs-secondary'; this.secondaryTabs.className = 'right-tabs'; this.secondaryTabs.dataset.pane = '1'; this.secondaryTabs.setAttribute('role', 'tablist'); this.secondaryTabs.setAttribute('aria-label', '第二窗格标签');
    const secondaryActions = document.createElement('div'); secondaryActions.className = 'right-pane-actions';
    this.secondaryAdd = paneButton('新建右侧标签', 'add', () => this.setMenu(this.menu.hidden, false, 1)); this.secondaryAdd.id = 'right-add-secondary'; this.secondaryAdd.setAttribute('aria-haspopup', 'menu');
    secondaryActions.append(this.secondaryAdd); second.append(this.secondaryTabs, secondaryActions); this.stripRegion.append(second); this.headers.push(second);
    this.splitDivider = document.createElement('div'); this.splitDivider.className = 'right-split-divider'; this.splitDivider.dataset.splitDivider = ''; this.splitDivider.tabIndex = 0; this.splitDivider.setAttribute('role', 'separator'); this.splitDivider.setAttribute('aria-label', '调整两侧窗格比例'); this.splitDivider.setAttribute('aria-valuemin', '20'); this.splitDivider.setAttribute('aria-valuemax', '80'); this.splitDivider.hidden = true; this.pane.append(this.splitDivider);
    this.track = document.createElement('div'); this.track.className = 'right-fullscreen-track'; this.track.hidden = true; this.pane.before(this.track);
    this.recent = []; this.sequence = 0;
    this.epoch = 0;
    this.saved = new PaneState(() => sessionStorage); this.restoredScopes = new Set();
    window.addEventListener('pagehide', () => { for (const scope of new Set([...this.tabs.values()].map(tab => tab.scope))) this.saveScope(scope); });
    this.overview = document.createElement('section'); this.overview.className = 'right-tab-overview'; this.overview.hidden = true; this.overview.setAttribute('aria-label', '标签概览');
    this.overviewSearch = document.createElement('input'); this.overviewSearch.type = 'search'; this.overviewSearch.placeholder = '搜索标签'; this.overviewSearch.setAttribute('aria-label', '搜索标签');
    this.overviewRows = document.createElement('div'); this.overviewRows.className = 'right-overview-rows';
    this.overview.append(this.overviewSearch, this.overviewRows); this.pane.append(this.overview);
    this.overviewSearch.addEventListener('input', () => this.renderOverview());
    this.overviewButton = panelButton('', () => this.setOverview(this.overview.hidden), 'icon-button right-overview-button');
    this.overviewButton.setAttribute('aria-label', '标签概览'); this.overviewButton.dataset.tooltip = '标签概览'; this.overviewButton.append(paneIcon('overview'));
    this.headers[0].querySelector('.right-pane-actions').prepend(this.overviewButton);
    this.context = document.createElement('div'); this.context.className = 'right-pane-menu right-tab-context'; this.context.hidden = true; this.context.setAttribute('role', 'menu'); this.pane.append(this.context);
    this.contextMenu = new PaneMenu(this.context); this.addMenu = new PaneMenu(this.menu);
    this.notice = document.createElement('p'); this.notice.className = 'right-pane-notice'; this.notice.setAttribute('role', 'status'); this.pane.append(this.notice);
    this.closeButton.replaceChildren(paneIcon('collapse')); this.addButton.replaceChildren(paneIcon('add'));
    this.expandButton = panelButton('', () => this.setFullscreen(!this.fullscreen), 'icon-button'); this.expandButton.id = 'right-fullscreen'; this.expandButton.append(paneIcon('expand')); this.expandButton.setAttribute('aria-label', '全屏查看'); this.expandButton.dataset.tooltip = '全屏查看';
    this.compareButton = panelButton('', () => this.compare(this.layoutFor().split ? null : this.active.get(this.scope)), 'icon-button'); this.compareButton.id = 'right-compare'; this.compareButton.append(paneIcon('split')); this.compareButton.setAttribute('aria-label', '并排查看'); this.compareButton.dataset.tooltip = '并排查看两个标签';
    this.globalActions = document.createElement('span'); this.globalActions.className = 'right-global-actions'; this.globalActions.append(this.overviewButton, this.compareButton, this.expandButton, this.closeButton); this.addButton.after(this.globalActions);
    this.pane.addEventListener('focusin', event => { const tab = [...this.tabs.values()].find(tab => tab.scope === this.scope && tab.node.contains(event.target)); if (tab && this.layoutFor().active !== tab.id) { this.layoutFor().activate(tab.id); this.active.set(this.scope, tab.id); this.render(); } });
    this.splitDivider.addEventListener('pointerdown', event => { if (event.button !== 0) return; this.splitGesture = event.pointerId; this.splitDivider.setPointerCapture(event.pointerId); this.pane.classList.add('is-resizing'); event.preventDefault(); });
    this.splitDivider.addEventListener('pointermove', event => { if (this.splitGesture !== event.pointerId) return; const r = this.pane.getBoundingClientRect(); this.setRatio(this.isStacked ? (event.clientY - r.top) / r.height * 100 : (event.clientX - r.left) / r.width * 100, false); });
    const endSplit = () => { this.splitGesture = null; this.pane.classList.remove('is-resizing'); this.saveScope(); };
    for (const name of ['pointerup', 'pointercancel', 'lostpointercapture']) this.splitDivider.addEventListener(name, endSplit);
    this.splitDivider.addEventListener('dblclick', () => this.setRatio(50));
    this.splitDivider.addEventListener('keydown', event => { const delta = ['ArrowLeft', 'ArrowUp'].includes(event.key) ? -2 : ['ArrowRight', 'ArrowDown'].includes(event.key) ? 2 : 0; if (delta || ['Home', 'End', 'Enter'].includes(event.key)) { event.preventDefault(); this.setRatio(event.key === 'Home' ? 20 : event.key === 'End' ? 80 : event.key === 'Enter' ? 50 : this.layoutFor().ratio + delta); } });
    this.stacked = matchMedia('(max-width: 680px)'); this.stacked.addEventListener('change', () => this.render());
    this.secondaryTabs.addEventListener('wheel', event => { if (this.secondaryTabs.scrollWidth > this.secondaryTabs.clientWidth && Math.abs(event.deltaY) > Math.abs(event.deltaX)) { this.secondaryTabs.scrollLeft += event.deltaY; event.preventDefault(); } }, { passive: false });
    for (const strip of [this.tabsNode, this.secondaryTabs]) {
      strip.addEventListener('dragover', event => { if (this.dragging) event.preventDefault(); });
      strip.addEventListener('drop', event => { if (this.dragging && !event.target.closest('.right-tab-wrap')) { event.preventDefault(); this.moveTab(this.dragging, Number(strip.dataset.pane)); this.dragging = null; } });
    }
    this.tabsNode.addEventListener('wheel', event => { if (this.tabsNode.scrollWidth > this.tabsNode.clientWidth && Math.abs(event.deltaY) > Math.abs(event.deltaX)) { this.tabsNode.scrollLeft += event.deltaY; event.preventDefault(); } }, { passive: false });
    window.addEventListener('resize', () => this.render());
    this.shell.addEventListener('mona:navigation-layout', () => this.render());
    this.geometryResize = new ResizeObserver(() => { if (this.splitMinimum) this.render(); });
    this.geometryResize.observe(this.shell); this.geometryResize.observe($('#chat-sidebar'));
    const tokens = getComputedStyle(document.documentElement);
    const px = (name, fallback) => Number.parseFloat(tokens.getPropertyValue(name)) || fallback;
    this.minWidth = px('--ui-right-pane-min-width', 320);
    this.maxWidth = px('--ui-right-pane-max-width', 720);
    this.width = px('--ui-right-pane-default-width', 420);
    try {
      this.open = false;
      const width = Number(localStorage.getItem('mona.web.right-pane.width'));
      if (Number.isFinite(width) && width > 0) this.width = this.clampWidth(width);
    } catch { this.open = false; }
    this.applyWidth(this.width);

    this.toggle.addEventListener('click', () => this.setOpen(!this.open, true));
    this.closeButton.addEventListener('click', () => this.setOpen(false, true));
    this.scrim.addEventListener('click', () => this.setOpen(false, true));
    this.addButton.addEventListener('click', event => { event.stopPropagation(); this.setMenu(this.menu.hidden); });
    document.addEventListener('pointerdown', event => {
      if (!this.context.contains(event.target) && !this.contextMenu.trigger?.contains(event.target)) this.contextMenu.close();
      if (!this.overview.contains(event.target) && !this.overviewButton.contains(event.target)) this.setOverview(false);
    });
    document.addEventListener('keydown', event => {
      const key = event.key.toLowerCase();
      if (event.defaultPrevented || document.querySelector('dialog[open]') || this.settingsVisible || this.pane.inert) return;
      if (event.key === 'Escape' && (!this.overview.hidden || !this.context.hidden)) { event.preventDefault(); if (!this.context.hidden) this.contextMenu.close(true); else { this.setOverview(false); this.overviewButton.focus(); } event.stopPropagation(); return; }
      if ((event.ctrlKey || event.metaKey) && event.shiftKey && key === 't' && this.pane.contains(event.target)) { event.preventDefault(); this.reopen(this.recent.find(tab => tab.scope === this.scope)); return; }
      if ((event.ctrlKey || event.metaKey) && !event.shiftKey && key === 'w' && this.pane.contains(event.target) && !event.target.closest('.terminal-pane')) { event.preventDefault(); this.closeTab(this.active.get(this.scope)); return; }
      const shortcut = key === 'b' && event.altKey && (event.metaKey || event.ctrlKey);
      if (shortcut && !event.shiftKey) {
        event.preventDefault(); this.setOpen(!this.open, true); return;
      }
      if (event.key === 'Escape' && !this.menu.hidden) { event.preventDefault(); this.setMenu(false, true); return; }
      if (event.key === 'Escape' && this.open && this.fullscreen && !this.isOverlay) { event.preventDefault(); this.setFullscreen(false); return; }
      if (event.key === 'Escape' && this.open && this.isOverlay) { event.preventDefault(); this.setOpen(false, true); }
      if (event.key === 'Tab' && this.open && (this.isOverlay || this.fullscreen) && !this.pane.hidden) trapTab(event, this.pane);
    });
    this.media.addEventListener('change', () => {
      this.render();
    });
    this.splitMinimum = px('--ui-right-pane-split-min-width', 640);
    this.columnMinimum = px('--ui-right-pane-column-min-width', 240);
    this.resizer.setAttribute('aria-valuemin', String(this.minWidth));
    this.resizer.setAttribute('aria-valuemax', String(this.maxWidth));
    this.resizer.addEventListener('pointerdown', event => {
      if (this.isOverlay || this.pane.hidden || this.fullscreen || event.button !== 0) return;
      this.resizeGesture = { pointer: event.pointerId, startX: event.clientX, startWidth: this.pane.getBoundingClientRect().width };
      this.resizer.setPointerCapture(event.pointerId); event.preventDefault(); this.pane.classList.add('is-resizing');
    });
    this.resizer.addEventListener('pointermove', event => {
      if (!this.resizeGesture || event.pointerId !== this.resizeGesture.pointer) return;
      this.applyWidth(this.resizeGesture.startWidth - (event.clientX - this.resizeGesture.startX));
    });
    const endResize = event => {
      if (!this.resizeGesture || (event && event.pointerId !== this.resizeGesture.pointer)) return;
      this.resizeGesture = null; this.pane.classList.remove('is-resizing');
      try { localStorage.setItem('mona.web.right-pane.width', String(this.width)); } catch {}
    };
    this.resizer.addEventListener('pointerup', endResize);
    this.resizer.addEventListener('pointercancel', endResize);
    this.resizer.addEventListener('lostpointercapture', endResize);
    this.resizer.addEventListener('keydown', event => {
      const delta = event.key === 'ArrowLeft' ? 16 : event.key === 'ArrowRight' ? -16 : 0;
      if (!delta) return;
      event.preventDefault(); this.applyWidth(this.pane.getBoundingClientRect().width + delta);
      try { localStorage.setItem('mona.web.right-pane.width', String(this.width)); } catch {}
    });
    this.layout();
  }

  clampWidth(value) { return Math.min(this.maxWidth, Math.max(this.minWidth, Number.isFinite(value) ? Math.round(value) : this.minWidth)); }
  applyWidth(value) { this.width = this.clampWidth(value); if (this.splitMinimum) this.render(); }
  fitColumns() {
    const sidebar = $('#chat-sidebar'), styles = getComputedStyle(sidebar), frame = getComputedStyle(this.shell);
    const left = sidebar.hidden || styles.display === 'none' || styles.position === 'fixed' ? 0 : sidebar.getBoundingClientRect().width + $('#sidebar-resizer').getBoundingClientRect().width;
    const available = this.shell.clientWidth - parseFloat(frame.paddingLeft) - parseFloat(frame.paddingRight);
    const resolved = resolveColumns({ width: available, left, right: this.width, rightMin: this.minWidth, rightMax: this.maxWidth });
    this.isOverlay = this.media.matches || resolved.overlay;
    const width = `${resolved.right}px`;
    if (this.shell.style.getPropertyValue('--right-pane-width') !== width) this.shell.style.setProperty('--right-pane-width', width);
    this.resizer.setAttribute('aria-valuenow', String(Math.round(resolved.right)));
    this.resizer.setAttribute('aria-valuemax', String(Math.round(Math.min(this.maxWidth, Math.max(this.minWidth, resolved.maximum)))));
  }

  registerAction({ id, label, icon = id, available = () => true, run, owner = 'shell' }) {
    if (this.actions.has(id)) throw new Error(`右侧操作重复注册：${id}`);
    const action = { id, label, icon, available, run, owner }; this.actions.set(id, action); this.renderActions();
    return () => { if (this.actions.get(id) === action) { this.actions.delete(id); this.renderActions(); } };
  }
  releaseOwner(owner) {
    for (const [id, action] of this.actions) if (action.owner === owner) this.actions.delete(id);
    for (const tab of [...this.tabs.values()]) if (tab.owner === owner) {
      tab.reopen = null; this.closeTab(tab.id, tab.scope);
    }
    this.recent = this.recent.filter(tab => tab.owner !== owner); this.renderActions();
  }

  layoutFor(scope = this.scope) {
    if (!this.layouts.has(scope)) this.layouts.set(scope, new PaneLayout());
    return this.layouts.get(scope);
  }
  setRatio(ratio, save = true) { this.layoutFor().ratio = Math.max(20, Math.min(80, Number.isFinite(ratio) ? ratio : 50)); this.updateSplitGeometry(); if (save) this.saveScope(); }
  updateSplitGeometry() {
    this.isStacked = this.stacked.matches || (this.layoutFor().split && this.pane.clientWidth < this.splitMinimum);
    this.pane.classList.toggle('is-stacked', this.isStacked);
    const floor = this.isStacked ? 20 : Math.max(20, Math.min(50, this.columnMinimum / Math.max(1, this.pane.clientWidth - 6) * 100));
    const ratio = Math.max(floor, Math.min(100 - floor, this.layoutFor().ratio));
    this.pane.style.setProperty('--right-split-first', `${ratio}fr`);
    this.pane.style.setProperty('--right-split-last', `${100 - ratio}fr`);
    this.splitDivider.setAttribute('aria-valuemin', String(Math.round(floor))); this.splitDivider.setAttribute('aria-valuemax', String(Math.round(100 - floor)));
    this.splitDivider.setAttribute('aria-valuenow', String(Math.round(ratio)));
    this.splitDivider.setAttribute('aria-valuetext', `${Math.round(ratio)}% / ${Math.round(100 - ratio)}%`);
    this.splitDivider.setAttribute('aria-orientation', this.isStacked ? 'horizontal' : 'vertical');
  }
  revealVisible(focusId = this.layoutFor().active) {
    if (!this.open || this.pane.hidden) return;
    const visible = this.scopeTabs().filter(tab => !tab.node.hidden);
    for (const tab of visible.filter(tab => tab.id !== focusId)) if (!tab.visited) { tab.visited = true; this.runSafely(() => tab.onActivate?.()); }
    const active = visible.find(tab => tab.id === focusId); if (active) { active.visited = true; this.runSafely(() => active.onActivate?.()); }
  }
  moveTab(id, side, index) { this.layoutFor().move(id, side, index); this.active.set(this.scope, this.layoutFor().active); this.render(); this.revealVisible(); this.saveScope(); this.tabs.get(`${this.scope}\0${id}`)?.button?.focus({ preventScroll: true }); }

  setScope(scope) {
    const next = scope || 'none';
    if (next !== this.scope) {
      this.presentations.set(this.scope, this.open); this.saveScope(this.scope);
      this.layoutFor().fullscreen = this.fullscreen;
      this.open = this.presentations.get(next) ?? (this.saved.get(next)?.expanded === true && !this.media.matches);
      this.fullscreen = Boolean(this.layoutFor(next).fullscreen ?? this.saved.get(next)?.fullscreen); this.setMenu(false); this.setOverview(false); this.contextMenu.close(); this.notice.textContent = '';
    }
    this.scope = next; this.render();
    this.restoreScope(next); this.revealVisible();
  }

  configurePersistence(endpoint, restoreFile) {
    this.saved.configure(endpoint); this.restoreFile = restoreFile; this.restoredScopes.clear(); this.restoreScope(this.scope);
  }
  saveScope(scope = this.scope) {
    if (this.restoring || scope === 'none') return;
    const files = this.scopeTabs(scope).filter(tab => tab.fileState).map(tab => tab.fileState());
    const current = this.tabs.get(`${scope}\0${this.active.get(scope)}`);
    const expanded = scope === this.scope ? this.open : this.presentations.get(scope);
    const layout = this.layoutFor(scope), key = id => { const tab = this.tabs.get(`${scope}\0${id}`); return tab?.fileState ? 'file:' + tab.fileState().path : tab?.id === 'files' ? 'page:files' : null; };
    const panes = layout.groups.map(group => group.map(key).filter(Boolean));
    if (!this.saved.save(scope, files, current?.fileState?.().path, { expanded, filesPage: this.hasTab('files', scope), activeFilesPage: current?.id === 'files', panes, selected: layout.selected.map(key), focus: layout.focus, ratio: layout.ratio, fullscreen: scope === this.scope ? this.fullscreen : layout.fullscreen }) && this.saved.endpoint && !this.storageWarning) { this.storageWarning = true; this.notice.textContent = '浏览器未保存文件标签状态；本次使用不受影响。'; }
  }
  restoreScope(scope) {
    if (!this.restoreFile || this.restoredScopes.has(scope) || scope === 'none') return;
    this.restoredScopes.add(scope); const record = this.saved.get(scope); if (!record || this.scopeTabs(scope).length) return;
    this.restoring = true; const wasOpen = record.expanded && !this.media.matches;
    try {
      for (const file of record.files) this.restoreFile(scope, { ...file, background: true });
      if (record.filesPage) this.restoreFile(scope, { kind: 'files', background: true });
      const layout = this.layoutFor(scope), byKey = new Map(this.scopeTabs(scope).map(tab => [tab.id === 'files' ? 'page:files' : 'file:' + tab.fileState?.().path, tab.id]));
      if (record.panes) { layout.groups = record.panes.map(group => group.map(key => byKey.get(key)).filter(Boolean)); layout.selected = record.selected.map(key => byKey.get(key) || null); layout.focus = record.focus; layout.ratio = record.ratio; layout.normalize(); }
      this.active.set(scope, layout.active); this.fullscreen = record.fullscreen; layout.fullscreen = record.fullscreen;
      this.open = wasOpen; this.render(); this.revealVisible();
    } catch (error) { this.notice.textContent = '部分文件标签未恢复：' + error.message; }
    finally { this.restoring = false; }
  }
  clear() {
    for (const scope of new Set([...this.tabs.values()].map(tab => tab.scope))) this.saveScope(scope);
    this.epoch++; this.setMenu(false); this.setOverview(false); this.contextMenu.close();
    this.restoreFile = null; this.restoredScopes.clear();
    for (const tab of this.tabs.values()) { this.runSafely(() => tab.onClose?.()); tab.node.remove(); tab.wrap?.remove(); }
    this.tabs.clear(); this.active.clear(); this.recent = []; this.presentations.clear(); this.layouts.clear(); this.fullscreen = false; this.open = false; this.scope = 'none'; this.render();
  }
  runSafely(action) {
    const epoch = this.epoch, scope = this.scope;
    const fail = error => { if (epoch === this.epoch && scope === this.scope && error?.name !== 'AbortError') this.notice.textContent = error?.message || '面板操作失败，请刷新后重试。'; };
    try { Promise.resolve(action()).catch(fail); } catch (error) { fail(error); }
  }
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
    this.recent = this.recent.filter(item => item !== tab); this.runSafely(() => tab.reopen(tab.restoreState));
  }
  contextFor(tab) {
    this.setMenu(false); this.setOverview(false); this.context.replaceChildren();
    const layout = this.layoutFor(), peers = () => this.scopeTabs().filter(t => layout.side(t.id) === layout.side(tab.id));
    const actions = layout.split ? [['移到另一窗格', () => this.moveTab(tab.id, 1 - layout.side(tab.id))], ['合并窗格', () => this.compare(null)]] : this.scopeTabs().length > 1 ? [['在另一窗格查看', () => this.compare(tab.id)]] : [];
    actions.push(['关闭标签', () => this.closeTab(tab.id)], ['关闭其他标签', () => peers().filter(t => t !== tab).forEach(t => this.closeTab(t.id))], ['关闭所有标签', () => this.scopeTabs().forEach(t => this.closeTab(t.id))]);
    for (const [label, action] of actions) { const item = panelButton(label, () => { this.contextMenu.close(true); action(); }, 'right-pane-menu-item'); item.setAttribute('role', 'menuitem'); this.context.append(item); }
    this.contextMenu.open(tab.button);
  }
  reorder(source, target, after = false) { this.layoutFor().reorder(source, target, after); this.active.set(this.scope, this.layoutFor().active); this.render(); this.saveScope(); }

  scopeTabs(scope = this.scope) {
    return this.layoutFor(scope).order.map(id => this.tabs.get(`${scope}\0${id}`)).filter(Boolean);
  }

  hasTab(id, scope = this.scope) {
    return this.tabs.has(`${scope}\0${id}`);
  }

  openTab({ id, title, kind, node, scope = this.scope, onClose, onActivate, reopen, hint = '', fileState, activate = true, owner = 'shell' }) {
    if (!scope || scope === 'none') return null;
    const key = `${scope}\0${id}`;
    let tab = this.tabs.get(key);
    if (tab && tab.owner !== owner) throw new Error('此标签属于另一个 UI 模块。');
    if (!tab) {
      if (this.tabs.size >= 48) { this.notice.textContent = '打开的标签已达 48 个，请先关闭不再使用的标签。'; this.runSafely(() => onClose?.()); return null; }
      tab = { key, id, scope, title, kind, node, onClose, onActivate, reopen, hint, fileState, owner, domId: `right-tab-${++this.sequence}` };
      this.tabs.set(key, tab);
      const layout = this.layoutFor(scope); const current = this.tabs.get(`${scope}\0${layout.active}`);
      const side = kind === 'file' && layout.split && current?.kind === 'files' ? 1 - layout.side(current.id) : layout.focus;
      layout.add(id, side, activate);
      node.id = `${tab.domId}-panel`; node.setAttribute('role', 'tabpanel'); node.setAttribute('aria-labelledby', tab.domId);
      this.content.append(node);
    } else {
      if (title) tab.title = title;
      if (node && tab.node !== node) { tab.node.replaceWith(node); tab.node = node; node.id = `${tab.domId}-panel`; node.setAttribute('role', 'tabpanel'); node.setAttribute('aria-labelledby', tab.domId); }
      if (onActivate) tab.onActivate = onActivate;
    }
    if (activate) {
      this.activate(id, scope);
    } else { node.hidden = true; this.render(); }
    this.saveScope(scope); return tab;
  }

  activate(id, scope = this.scope) {
    const tab = this.tabs.get(`${scope}\0${id}`);
    if (!tab) return;
    if (scope !== this.scope) this.setScope(scope);
    this.layoutFor(scope).activate(id); this.active.set(scope, id); this.setOpen(true); this.saveScope(scope);
  }

  closeTab(id, scope = this.scope) {
    const key = `${scope}\0${id}`, tab = this.tabs.get(key);
    if (!tab) return;
    const peers = this.scopeTabs(scope), index = peers.findIndex(item => item.id === id);
    const restoreFocus = tab.wrap?.contains(document.activeElement) || tab.node.contains(document.activeElement);
    const restoreState = tab.fileState?.();
    this.layoutFor(scope).remove(id);
    this.tabs.delete(key);
    this.runSafely(() => tab.onClose?.()); tab.node.remove(); tab.wrap?.remove();
    if (tab.reopen) { const { key, id, title, scope, hint, kind, reopen, owner } = tab; this.recent = [{ key, id, title, scope, hint, kind, reopen, restoreState, owner }, ...this.recent.filter(t => t.key !== tab.key)].slice(0, 12); }
    if (this.active.get(scope) === id) {
      const rest = this.scopeTabs(scope);
      const next = rest[Math.min(index, Math.max(0, rest.length - 1))];
      if (next) this.active.set(scope, next.id); else this.active.delete(scope);
    }
    this.active.set(scope, this.layoutFor(scope).active);
    if (scope === this.scope && !this.scopeTabs(scope).length) { this.setOpen(false, true); return; }
    this.render(); this.revealVisible(); this.saveScope(scope);
    if (restoreFocus && scope === this.scope) {
      const next = this.tabs.get(`${scope}\0${this.active.get(scope)}`);
      (next?.button || this.addButton || this.closeButton).focus();

    }
  }

  clearScope(scope) {
    for (const tab of this.scopeTabs(scope)) {
      this.tabs.delete(tab.key);
      this.runSafely(() => tab.onClose?.()); tab.node.remove(); tab.wrap?.remove();
    }
    this.active.delete(scope); this.layouts.delete(scope); this.presentations.delete(scope); this.saved.save(scope, [], null);
    if (scope === this.scope) this.setOpen(false, true);
    this.recent = this.recent.filter(tab => tab.scope !== scope);
    this.render();
  }

  setFullscreen(value) {
    if (this.isOverlay) { this.setOpen(false, true); return; }
    this.fullscreen = Boolean(value); this.layoutFor().fullscreen = this.fullscreen;
    this.render(); this.saveScope(); this.expandButton.focus({ preventScroll: true });
  }
  compare(id) {
    const layout = this.layoutFor();
    if (!id) layout.merge();
    else {
      if (!layout.split && this.pane.getBoundingClientRect().width < this.splitMinimum) { this.notice.textContent = '当前面板太窄，请先拉宽或全屏后再分栏。'; return; }
      if (!layout.divide(id)) return;
    }
    this.active.set(this.scope, layout.active); this.notice.textContent = ''; this.render(); this.revealVisible(); this.saveScope(); this.tabs.get(`${this.scope}\0${layout.active}`)?.button?.focus({ preventScroll: true });
  }

  setOpen(value, focus = false) {
    const wasOpen = this.open, returnFocus = !value && (this.pane.contains(document.activeElement) || focus);
    if (value && !wasOpen && !this.pane.contains(document.activeElement)) this.opener = document.activeElement;
    this.open = Boolean(value); this.setMenu(false); this.setOverview(false); this.contextMenu.close();
    this.presentations.set(this.scope, this.open);
    while (this.presentations.size > 64) this.presentations.delete(this.presentations.keys().next().value);
    this.render(); this.saveScope(); this.revealVisible();
    if (returnFocus) { const opener = this.opener?.isConnected && this.opener.matches('button,a[href],input,textarea,[tabindex]') && !this.opener.closest('[hidden],[inert]') ? this.opener : this.toggle; opener.focus({ preventScroll: true }); }
    else if (focus && value) (this.tabs.get(`${this.scope}\0${this.layoutFor().active}`)?.button || this.addButton).focus({ preventScroll: true });
  }

  setMenu(value, focus = false, side = 0) {
    if (!value) { this.addMenu.close(focus); return; }
    this.contextMenu.close(); this.setOverview(false); this.layoutFor().focus = side;
    this.renderActions(); this.addMenu.open(side ? this.secondaryAdd : this.addButton);
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
    this.addButton.hidden = this.secondaryAdd.hidden = this.menu.childElementCount === 0;
  }

  mountTab(tab) {
    const wrap = document.createElement('div'); wrap.className = 'right-tab-wrap'; wrap.draggable = true; wrap.dataset.tabId = tab.id;
    wrap.addEventListener('dragstart', event => { this.dragging = tab.id; event.dataTransfer.effectAllowed = 'move'; event.dataTransfer.setData('text/plain', 'mona-tab'); });
    wrap.addEventListener('dragover', event => { if (this.dragging) { event.preventDefault(); const r = wrap.getBoundingClientRect(); wrap.dataset.drop = event.clientX > r.x + r.width / 2 ? 'after' : 'before'; } });
    wrap.addEventListener('dragleave', () => { delete wrap.dataset.drop; });
    wrap.addEventListener('drop', event => { if (this.dragging) { event.preventDefault(); event.stopPropagation(); this.reorder(this.dragging, tab.id, wrap.dataset.drop === 'after'); this.dragging = null; delete wrap.dataset.drop; } });
    wrap.addEventListener('dragend', () => { this.dragging = null; for (const node of this.pane.querySelectorAll('[data-drop]')) delete node.dataset.drop; });
    wrap.addEventListener('contextmenu', event => { event.preventDefault(); this.contextFor(tab); });
    const button = document.createElement('button'); button.type = 'button'; button.className = 'right-tab'; button.setAttribute('role', 'tab');
    button.dataset.kind = tab.kind || ''; button.dataset.tabId = tab.id; button.id = tab.domId; button.setAttribute('aria-controls', `${tab.domId}-panel`);
    const label = document.createElement('span'); button.append(paneIcon(tab.kind), label);
    button.addEventListener('keydown', event => {
      const layout = this.layoutFor(tab.scope), current = layout.groups[layout.side(tab.id)], i = current.indexOf(tab.id);
      const index = event.key === 'ArrowRight' ? (i + 1) % current.length : event.key === 'ArrowLeft' ? (i + current.length - 1) % current.length : event.key === 'Home' ? 0 : event.key === 'End' ? current.length - 1 : -1;
      if (index >= 0) {
        event.preventDefault(); const next = this.tabs.get(`${tab.scope}\0${current[index]}`);
        if (event.altKey) { this.reorder(tab.id, next.id, index > i); tab.button.focus(); }
        else { for (const id of current) this.tabs.get(`${tab.scope}\0${id}`).button.tabIndex = id === next.id ? 0 : -1; next.button.focus({ preventScroll: true }); this.scrollTab(next); }
      }
      if (event.key === 'Delete') { event.preventDefault(); this.closeTab(tab.id, tab.scope); }
      if (event.key === 'F10' && event.shiftKey) { event.preventDefault(); this.contextFor(tab); }
    });
    button.addEventListener('click', () => this.activate(tab.id, tab.scope));
    button.addEventListener('auxclick', event => { if (event.button === 1) { event.preventDefault(); this.closeTab(tab.id, tab.scope); } });
    const close = paneButton(`关闭“${tab.title}”`, 'close', event => { event.stopPropagation(); this.closeTab(tab.id, tab.scope); }, 'right-tab-close');
    tab.wrap = wrap; tab.button = button; tab.label = label; wrap.append(button, close);
  }
  scrollTab(tab) {
    const strip = tab?.button?.closest('.right-tabs'); if (!strip || !strip.clientWidth) return;
    const r = tab.wrap.getBoundingClientRect(), box = strip.getBoundingClientRect();
    if (r.left < box.left) strip.scrollLeft += r.left - box.left;
    else if (r.right > box.right) strip.scrollLeft += r.right - box.right;
  }
  render() {
    this.renderActions(); const tabs = this.scopeTabs(), layout = this.layoutFor(); layout.normalize();
    this.active.set(this.scope, layout.active); this.headers[1].hidden = !layout.split;
    this.pane.classList.toggle('is-split', layout.split); this.pane.classList.toggle('is-stacked', this.stacked.matches);
    const globalParent = this.headers[layout.split ? 1 : 0].querySelector('.right-pane-actions'); if (this.globalActions.parentElement !== globalParent) globalParent.append(this.globalActions);
    for (const [side, strip] of [this.tabsNode, this.secondaryTabs].entries()) {
      const ids = layout.groups[side];
      for (const wrap of [...strip.children]) if (!ids.includes(wrap.dataset.tabId)) wrap.remove();
      for (const [position, id] of ids.entries()) {
        const tab = this.tabs.get(`${this.scope}\0${id}`); if (!tab) continue; if (!tab.wrap) this.mountTab(tab);
        const selected = layout.selected[side] === id; tab.wrap.dataset.active = String(selected); tab.button.setAttribute('aria-selected', String(selected)); tab.button.tabIndex = selected ? 0 : -1;
        tab.label.textContent = tab.title; tab.button.dataset.tooltip = tab.hint || tab.title;
        if (strip.children[position] !== tab.wrap) strip.insertBefore(tab.wrap, strip.children[position] || null);
      }
      this.headers[side].dataset.focused = String(layout.focus === side);
    }
    for (const tab of this.tabs.values()) {
      const current = tab.scope === this.scope, side = current ? layout.side(tab.id) : 0;
      tab.node.hidden = !current || layout.selected[side] !== tab.id; tab.node.dataset.pane = String(side);
    }
    this.empty.hidden = tabs.length > 0; this.content.hidden = !tabs.length;
    this.overviewButton.hidden = !tabs.length && !this.recent.some(tab => tab.scope === this.scope);
    this.layout(); this.updateSplitGeometry();
    this.compareButton.disabled = tabs.length < 2 || (!layout.split && this.pane.clientWidth < this.splitMinimum);
    this.compareButton.setAttribute('aria-pressed', String(layout.split)); this.compareButton.setAttribute('aria-label', layout.split ? '合并窗格' : '分栏查看');
    this.compareButton.dataset.tooltip = layout.split ? '合并窗格' : tabs.length < 2 ? '先打开两个标签' : this.compareButton.disabled ? '拉宽或全屏后可分栏' : '分栏查看';
    if (!this.overview.hidden) this.renderOverview();
    if (this.open && !this.pane.hidden) for (const id of layout.selected) this.scrollTab(this.tabs.get(`${this.scope}\0${id}`));
    this.shell.dispatchEvent(new Event('mona:right-pane-render'));
  }

  layout() {
    this.fitColumns();
    const show = this.open && !this.settingsVisible && this.scope !== 'none';
    this.pane.hidden = !show;
    const modal = show && (this.isOverlay || this.fullscreen);
    if (modal && !this.announcedModal) { this.announcedModal = true; this.shell.dispatchEvent(new Event('mona:right-overlay')); }
    if (!modal) this.announcedModal = false;
    this.resizer.hidden = !show || this.isOverlay || this.fullscreen;
    this.scrim.hidden = !modal;
    this.pane.classList.toggle('is-fullscreen', this.fullscreen || this.isOverlay);
    this.track.hidden = !show || !this.fullscreen || this.isOverlay;
    this.splitDivider.hidden = !show || !this.layoutFor().split;
    this.expandButton.setAttribute('aria-pressed', String(this.fullscreen)); this.expandButton.dataset.tooltip = this.isOverlay ? '收起面板' : this.fullscreen ? '退出全屏' : '全屏查看'; this.expandButton.setAttribute('aria-label', this.expandButton.dataset.tooltip);
    this.toggle.hidden = this.scope === 'none' || show;
    this.toggle.setAttribute('aria-expanded', String(show));
    document.querySelector('.shell')?.classList.toggle('right-pane-open', show);
    setShellOverlay(this.shell, this, [this.pane, this.scrim, this.resizer], modal);
    if (modal) {
      this.pane.setAttribute('role', 'dialog'); this.pane.setAttribute('aria-modal', 'true');
      if (!this.pane.contains(document.activeElement)) this.closeButton.focus({ preventScroll: true });
    } else { this.pane.setAttribute('role', 'complementary'); this.pane.removeAttribute('aria-modal'); }
  }

  settingsVisibility(value) {
    this.settingsVisible = value; this.setMenu(false); this.setOverview(false); this.contextMenu.close(); this.layout();
  }
}
