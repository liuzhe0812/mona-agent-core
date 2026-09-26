import { WorkspaceClient } from './workspace.mjs';
import { FilePreview } from './file-preview.mjs';
import { messageFileTarget } from './presentation-path.mjs';
import { WorkspaceFiles } from './workspace-files.mjs';
import { relativeTime } from './run-view.mjs';
const $ = selector => document.querySelector(selector);
const node = (tag, className, value = '') => { const n = document.createElement(tag); n.className = className; n.textContent = value; return n; };
const button = (label, className = 'text-button') => { const n = node('button', className, label); n.type = 'button'; return n; };
function folderIcon() { const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); svg.setAttribute('viewBox', '0 0 24 24'); svg.setAttribute('class', 'line-icon'); svg.setAttribute('aria-hidden', 'true'); const path = document.createElementNS(svg.namespaceURI, 'path'); path.setAttribute('d', 'M3 7V5h6l2 2h10v12H3Z'); svg.append(path); return svg; }
function menuIcon(pathData) { const svg = folderIcon(); svg.replaceChildren(); const path = document.createElementNS(svg.namespaceURI, 'path'); path.setAttribute('d', pathData); svg.append(path); return svg; }
const fileName = path => path.split('/').at(-1) || path;
const PROJECTS_COLLAPSED_KEY = 'mona.web.projects.collapsed.v1';
const PROJECT_ORDER_KEY = 'mona.web.projects.order.v1';
const HEADER_OPEN_MODE_KEY = 'mona.web.header.open-mode.v1';
export class WorkspaceUI {
  constructor(hooks) {
    this.events = new AbortController(); const on = (target, type, fn, options = {}) => target.addEventListener(type, fn, { ...options, signal: this.events.signal });
    this.$ = hooks.query || $;
    this.hooks = hooks; this.pane = hooks.pane; this.api = new WorkspaceClient(); this.available = false; this.settings = null;
    this.projectState = null; this.projectLoading = false; this.projectPicking = false; this.projectSaving = false; this.projectError = ''; this.session = null; this.generation = 0;
    this.projectOrder = []; this.projectOrderKey = PROJECT_ORDER_KEY; this.projectSorting = false; this.dragProjectId = null;
    this.projectMenu = node('div', 'project-options-menu'); this.projectMenu.hidden = true;
    this.projectMenu.setAttribute('role', 'menu'); document.body.append(this.projectMenu);
    this.$('#header-workspace-context').closest('.topbar').append(this.$('#header-workspace-popover'));
    this.$('#header-open-group').append(this.$('#header-open-menu'));
    this.headerFolderIcon = this.$('#header-open-workspace').firstElementChild.cloneNode(true);
    try { this.headerOpenMode = localStorage.getItem(HEADER_OPEN_MODE_KEY) || 'explorer'; }
    catch { this.headerOpenMode = 'explorer'; }
    this.contextPinned = false;
    this.tabStates = new Map(); this.fileBrowsers = new Map();
    this.settingsVisible = false;
    on(this.$('#workspace-files-open'), 'click', () => { if (this.session) void this.openSessionFiles(); });
    on(this.$('#workspace-settings-refresh'), 'click', () => { void this.refreshSettings(); });
    on(this.$('#workspace-root-form'), 'submit', event => { event.preventDefault(); void this.saveRoot(); });
    on(this.$('#project-add'), 'click', () => { void this.addProject(); });
    on(this.$('#project-cancel'), 'click', () => { if (!this.projectSaving) this.$('#project-dialog').close(); });
    on(this.$('#project-dialog'), 'cancel', event => { if (this.projectSaving) event.preventDefault(); });
    on(this.$('#project-form'), 'submit', event => { event.preventDefault(); void this.saveNamedProject(); });
    on(this.$('#project-dialog'), 'close', () => (this.projectDialogReturn?.isConnected ? this.projectDialogReturn : this.$('#project-add')).focus());
    try { this.setProjectsCollapsed(localStorage.getItem(PROJECTS_COLLAPSED_KEY) === '1', false); }
    catch { this.setProjectsCollapsed(false, false); }
    on(this.$('#projects-collapse'), 'click', () => this.setProjectsCollapsed(!this.$('#projects-region').classList.contains('is-collapsed')));
    on(this.$('#projects-sort'), 'click', () => this.setProjectSorting(!this.projectSorting));
    const projectList = this.$('#project-list');
    on(projectList, 'dragstart', event => this.projectDragStart(event));
    on(projectList, 'dragover', event => this.projectDragOver(event));
    on(projectList, 'drop', event => this.projectDrop(event));
    on(projectList, 'dragend', () => this.clearProjectDrag());
    on(projectList, 'keydown', event => this.projectMoveByKey(event));
    on(this.$('#project-picker'), 'click', () => this.toggleProjectPicker());
    const contextTrigger = this.$('#header-workspace-context'), context = this.$('#header-workspace-popover');
    on(contextTrigger, 'pointerenter', () => { if (!this.contextPinned) this.openHeaderContext(); });
    on(contextTrigger, 'pointerleave', () => { if (!this.contextPinned) this.scheduleContextClose(); });
    on(context, 'pointerenter', () => clearTimeout(this.contextCloseTimer));
    on(context, 'pointerleave', () => { if (!this.contextPinned) this.scheduleContextClose(); });
    on(contextTrigger, 'click', () => {
      if (this.contextPinned) this.closeHeaderContext(true);
      else { this.contextPinned = true; this.openHeaderContext(); }
    });
    on(this.$('#header-open-workspace'), 'click', () => this.openHeaderTarget(this.headerOpenMode));
    on(this.$('#header-open-options'), 'click', () => this.toggleHeaderOpenMenu());
    on(this.$('#header-open-menu'), 'keydown', event => {
      if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
      const items = [...this.$('#header-open-menu').querySelectorAll('button:not(:disabled)')];
      if (!items.length) return;
      event.preventDefault();
      const index = items.indexOf(document.activeElement);
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1
        : event.key === 'ArrowDown' ? (index + 1) % items.length : (index < 0 ? items.length - 1 : index - 1 + items.length) % items.length;
      items[next].focus();
    });
    on(this.$('#project-picker').parentElement.querySelector('#project-detach'), 'click', event => {
      event.stopPropagation(); void this.chooseProject(null);
    });
    on(this.$('#project-menu-search'), 'input', () => this.renderProjectPickerOptions());
    on(this.$('#composer-project-menu'), 'keydown', event => {
      if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
      const items = [...this.$('#composer-project-menu').querySelectorAll('button:not(:disabled)')];
      if (!items.length) return;
      event.preventDefault(); event.stopPropagation();
      const index = items.indexOf(document.activeElement);
      const target = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1
        : event.key === 'ArrowDown' ? (index + 1) % items.length : (index < 0 ? items.length - 1 : index - 1 + items.length) % items.length;
      items[target].focus();
    });
    on(document, 'pointerdown', event => {
      if (!this.$('#composer-project-menu').hidden && !this.$('#project-picker').contains(event.target)
        && !this.$('#composer-project-menu').contains(event.target)) this.closeProjectPicker();
      if (!this.projectMenu.hidden && !this.projectMenu.contains(event.target) && !this.projectMenuOpener?.contains(event.target)) this.closeProjectMenu();
      if (!this.$('#header-open-group').contains(event.target)) this.closeHeaderOpenMenu();
      if (!contextTrigger.contains(event.target) && !context.contains(event.target)) this.closeHeaderContext();
    });
    on(document, 'scroll', () => { this.closeProjectMenu(); this.closeHeaderOpenMenu(); this.closeHeaderContext(); }, { capture: true });
    on(window, 'resize', () => { this.closeProjectMenu(); this.closeHeaderOpenMenu(); this.closeHeaderContext(); });
    on(document, 'keydown', event => {
      if (event.key === 'Escape' && !this.projectMenu.hidden) {
        event.preventDefault(); this.closeProjectMenu(true); return;
      }
      if (event.key === 'Escape' && !this.$('#header-open-menu').hidden) { event.preventDefault(); this.closeHeaderOpenMenu(true); return; }
      if (event.key === 'Escape' && !context.hidden) { event.preventDefault(); this.closeHeaderContext(true); return; }
      if (event.key === 'Escape' && !this.$('#composer-project-menu').hidden) {
        event.preventDefault(); this.closeProjectPicker(); this.$('#project-picker').focus();
      }
    });
    this.pane.registerAction({
      id: 'files', label: '浏览工作区文件',
      icon: 'folder', available: () => Boolean(this.available && (this.session || this.hooks.currentProject?.())),
      run: () => { const target = this.currentFileTarget(); if (target) this.openFiles(target); },
    });
    this.renderSettings(); this.renderProjects();
    this.renderHeader();
  }
  dispose() { this.events.abort(); clearTimeout(this.contextCloseTimer); this.clear(); this.projectMenu.remove(); this.pane.configurePersistence('', null); }
  clear() {
    this.generation++; this.api.clear(); this.available = false; this.settings = null; this.projectState = null; this.projectLoading = false; this.projectPicking = false; this.projectSaving = false; this.projectError = '';
    if (this.$('#project-dialog').open) this.$('#project-dialog').close();
    this.closeProjectPicker(); this.closeProjectMenu(); this.setProjectSorting(false); this.projectOrder = [];
    this.session = null; this.lastSessionStamp = ''; this.$('#project-list').replaceChildren(); this.$('#projects-region').hidden = true;
    this.$('#projects-notice').textContent = ''; this.headerOpenError = '';
    this.renderSettings(); this.renderProjects(); this.pane.clear(); this.tabStates.clear(); this.fileBrowsers.clear(); this.features = {};
    this.renderHeader();
  }
  async configure(base, token) {
    this.api.configure(base, token); const generation = ++this.generation;
    this.projectOrderKey = `${PROJECT_ORDER_KEY}:${new URL(base, location.href).origin}`;
    try { const saved = JSON.parse(localStorage.getItem(this.projectOrderKey) || '[]'); this.projectOrder = Array.isArray(saved) ? saved.filter(id => typeof id === 'string').slice(0, 256) : []; }
    catch { this.projectOrder = []; }
    try {
      const settings = await this.api.settings(); if (generation !== this.generation) return;
      this.applySettings(settings); this.available = true;
      this.renderProjectPicker(); this.renderProjectAdd();
      if (settings.projects_enabled) await this.refreshProjects();
      try { this.features = await this.api.workbenchCapabilities(); } catch (e) { if (e.name === 'AbortError') throw e; this.features = {}; }
      if (generation !== this.generation) return;
      this.pane.renderActions();
      this.renderHeader();
      this.pane.configurePersistence(base, (scope, file) => {
        const [kind, id] = scope.split(':');
        if (!['session', 'project'].includes(kind)) return;
        const target = { kind, id, path: kind === 'session' && this.session?.id === id ? this.session.workspace : this.projectState?.projects.find(project => project.id === id)?.path };
        if (file.kind === 'files') this.openFiles(target, file.background);
        else this.openFile(file.path, target, scope, file);
      });
      this.syncSessionScope(); this.hooks.ready?.();
    } catch (e) {
      if (generation !== this.generation || e.name === 'AbortError') return;
      this.available = false; this.settings = null; this.renderSettings(); this.renderProjects(); this.pane.setScope('none');
      this.$('#workspace-notice').textContent = [404, 501].includes(e.status) ? '当前宿主未提供工作区管理。' : e.message;
    }
  }
  applySettings(value) {
    if (!Number.isSafeInteger(value?.revision) || typeof value.default_root !== 'string' || typeof value.projects_enabled !== 'boolean'
      || (value.project_create_mode != null && !['name', 'folder', 'none'].includes(value.project_create_mode))) throw new Error('工作区设置响应无效。');
    const desktop = this.hooks.desktopMode?.() === true && typeof this.hooks.pickDesktopProject === 'function';
    this.settings = { ...value, project_create_mode: desktop && value.projects_enabled ? 'folder' : value.project_create_mode || 'none' };
    this.renderSettings(); this.$('#projects-region').hidden = !value.projects_enabled; this.renderProjectPicker(); this.renderProjectAdd();
  }
  renderSettings() {
    const field = this.$('#workspace-root'); field.value = this.settings?.default_root || ''; field.disabled = !this.settings || this.settings.locked || Boolean(this.rootLoading || this.rootSaving);
    this.$('#workspace-root-save').disabled = field.disabled; this.$('#workspace-settings-refresh').disabled = !this.api.configured || Boolean(this.rootLoading || this.rootSaving);
    this.$('#workspace-notice').textContent = this.settings?.locked ? '默认根目录由部署配置指定。' : !this.settings ? '当前连接未提供工作区管理。' : '';
  }
  async refreshSettings() {
    if (this.rootSaving || this.rootLoading) return;
    const generation = this.generation; this.rootLoading = true; this.renderSettings(); let notice = '';
    try { this.applySettings(await this.api.settings()); }
    catch (e) { if (e.name !== 'AbortError') notice = e.message; }
    finally {
      this.rootLoading = false;
      if (generation === this.generation) { this.renderSettings(); if (notice) this.$('#workspace-notice').textContent = notice; }
    }
  }
  async saveRoot() {
    if (!this.settings || this.settings.locked || this.rootSaving || this.rootLoading) return;
    const root = this.$('#workspace-root').value.trim(), revision = this.settings.revision, generation = this.generation;
    this.rootSaving = true; this.$('#workspace-root').disabled = true; this.$('#workspace-root-save').disabled = true; this.$('#workspace-settings-refresh').disabled = true;
    this.$('#workspace-notice').textContent = '正在保存…'; let notice = '';
    try { this.applySettings(await this.api.saveRoot(revision, root)); notice = '已保存，新建普通会话时生效。'; }
    catch (e) { if (e.name !== 'AbortError') notice = e.message; }
    finally {
      this.rootSaving = false;
      if (generation === this.generation) { this.renderSettings(); this.$('#workspace-notice').textContent = notice; }
    }
  }
  async refreshProjects() {
    if (!this.settings?.projects_enabled) return;
    const generation = this.generation;
    this.projectLoading = true; this.projectError = ''; this.renderProjectPicker();
    try {
      const value = await this.api.projects(); if (generation !== this.generation) return;
      if (!Array.isArray(value.projects) || value.projects.length > 256 || !Number.isSafeInteger(value.revision)) throw new Error('项目列表响应无效。');
      this.projectState = value; this.renderProjects(); this.$('#projects-notice').textContent = '';
    } catch (e) { if (e.name !== 'AbortError' && generation === this.generation) this.projectError = this.$('#projects-notice').textContent = e.message; }
    finally { if (generation === this.generation) { this.projectLoading = false; this.renderProjectPicker(); } }
  }
  renderProjects() {
    this.closeProjectMenu(); this.clearProjectDrag();
    const list = this.$('#project-list'); list.replaceChildren();
    this.renderProjectAdd();
    const projects = this.sortedProjects();
    for (const project of projects) {
      const group = node('div', 'project-group'); group.dataset.projectId = project.id;
      const row = node('div', 'project-item'); row.dataset.projectId = project.id;
      row.draggable = this.projectSorting; row.classList.toggle('is-sortable', this.projectSorting);
      const open = button('', 'project-row'); open.dataset.tooltip = project.path;
      open.append(menuIcon('M3 7V5h6l2 2h8v3M3 19l2-9h16l-2 9H3Z'), node('span', '', project.name));
      open.setAttribute('aria-pressed', String(this.hooks.currentProject()?.id === project.id));
      open.addEventListener('click', () => { if (!this.projectSorting) void this.startProjectTask(project); });
      const actions = node('div', 'project-actions');
      const options = button('', 'icon-button project-options'); options.append(menuIcon('M5 12h.01M12 12h.01M19 12h.01'));
      options.setAttribute('aria-label', `“${project.name}”的操作`); options.setAttribute('aria-haspopup', 'menu'); options.setAttribute('aria-expanded', 'false');
      options.dataset.tooltip = '项目操作';
      options.addEventListener('click', event => { event.stopPropagation(); this.openProjectMenu(project, options); });
      const create = button('', 'icon-button project-new'); create.append(menuIcon('M20.5 11.5a8.5 8.5 0 0 1-8.5 8.5H5l-2.5 2.5v-11a9 9 0 1 1 18 0ZM8 12h8m-4-4v8'));
      create.setAttribute('aria-label', `在“${project.name}”中新建任务`); create.dataset.tooltip = '新建项目任务';
      create.addEventListener('click', event => { event.stopPropagation(); void this.startProjectTask(project); });
      actions.append(options, create);
      const sessions = node('div', 'project-sessions');
      sessions.setAttribute('aria-label', `${project.name}的任务`);
      const more = button('显示更多', 'text-button project-more'); more.hidden = true;
      more.setAttribute('aria-label', `显示更多“${project.name}”任务`);
      more.addEventListener('click', () => { void this.hooks.showMoreProject(project.id); });
      row.append(open, actions); group.append(row, sessions, more); list.append(group);
    }
    this.hooks.projectsRendered?.(projects);
    this.renderProjectPicker();
  }
  sortedProjects() {
    const ranks = new Map(this.projectOrder.map((id, index) => [id, index]));
    return [...(this.projectState?.projects || [])].sort((a, b) => (ranks.get(a.id) ?? 256) - (ranks.get(b.id) ?? 256));
  }
  async startProjectTask(project) {
    try {
      if (await this.hooks.selectProject(project)) { this.syncProjectSelection(); this.renderProjectPicker(); }
    } catch (error) { this.$('#projects-notice').textContent = error.message; }
  }
  setProjectsCollapsed(collapsed, persist = true) {
    this.$('#projects-region').classList.toggle('is-collapsed', collapsed);
    this.$('#projects-collapse').setAttribute('aria-expanded', String(!collapsed));
    if (persist) try { localStorage.setItem(PROJECTS_COLLAPSED_KEY, collapsed ? '1' : '0'); } catch { /* browser storage may be unavailable */ }
  }
  setProjectSorting(enabled) {
    this.projectSorting = enabled; this.clearProjectDrag();
    const sort = this.$('#projects-sort'); sort.setAttribute('aria-pressed', String(enabled));
    sort.setAttribute('aria-label', enabled ? '拖动项目分区；点击结束项目行排序' : '拖动项目分区；点击调整项目行顺序');
    sort.dataset.tooltip = enabled ? '拖动项目分区；点击结束项目行排序' : '拖动项目分区；点击调整项目行顺序';
    if (enabled) this.setProjectsCollapsed(false);
    for (const row of this.$('#project-list').querySelectorAll('.project-item')) {
      row.draggable = enabled; row.classList.toggle('is-sortable', enabled);
    }
  }
  projectDragStart(event) {
    const row = event.target.closest('.project-item');
    if (!this.projectSorting || !row) { event.preventDefault(); return; }
    this.dragProjectId = row.dataset.projectId;
    row.classList.add('is-dragging');
    event.dataTransfer?.setData('text/plain', this.dragProjectId);
    if (event.dataTransfer) event.dataTransfer.effectAllowed = 'move';
  }
  projectDragOver(event) {
    const group = event.target.closest('.project-group');
    if (!this.dragProjectId || !group || group.dataset.projectId === this.dragProjectId) return;
    event.preventDefault();
    for (const item of this.$('#project-list').querySelectorAll('.project-group[data-drop]')) delete item.dataset.drop;
    const rect = group.querySelector('.project-item').getBoundingClientRect();
    group.dataset.drop = event.clientY >= rect.top + rect.height / 2 ? 'after' : 'before';
    if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
  }
  projectDrop(event) {
    const list = this.$('#project-list'), target = event.target.closest('.project-group');
    if (!this.dragProjectId || !target || target.dataset.projectId === this.dragProjectId) { this.clearProjectDrag(); return; }
    event.preventDefault();
    const source = [...list.querySelectorAll('.project-group')].find(group => group.dataset.projectId === this.dragProjectId);
    if (source) { list.insertBefore(source, target.dataset.drop === 'after' ? target.nextSibling : target); this.saveProjectOrder(); }
    this.clearProjectDrag();
  }
  projectMoveByKey(event) {
    if (!this.projectSorting || !['ArrowUp', 'ArrowDown'].includes(event.key)) return;
    const row = event.target.closest('.project-row'), group = row?.closest('.project-group');
    const other = event.key === 'ArrowUp' ? group?.previousElementSibling : group?.nextElementSibling;
    if (!other) return;
    event.preventDefault();
    this.$('#project-list').insertBefore(group, event.key === 'ArrowUp' ? other : other.nextSibling);
    this.saveProjectOrder(); row.focus();
  }
  clearProjectDrag() {
    this.dragProjectId = null;
    for (const group of this.$('#project-list').querySelectorAll('.project-group')) {
      delete group.dataset.drop; group.querySelector('.project-item')?.classList.remove('is-dragging');
    }
  }
  saveProjectOrder() {
    this.projectOrder = [...this.$('#project-list').querySelectorAll('.project-group')].map(group => group.dataset.projectId);
    try { localStorage.setItem(this.projectOrderKey, JSON.stringify(this.projectOrder)); } catch { /* ordering still works for this session */ }
    this.hooks.projectsRendered?.(this.sortedProjects());
  }
  openProjectMenu(project, opener) {
    if (!this.projectMenu.hidden && this.projectMenuOpener === opener) { this.closeProjectMenu(true); return; }
    this.closeProjectMenu(); this.projectMenuOpener = opener;
    const remove = button('移除项目登记'); remove.setAttribute('role', 'menuitem');
    remove.addEventListener('click', () => { this.closeProjectMenu(); void this.removeProject(project); }, { once: true });
    this.projectMenu.replaceChildren(remove);
    this.projectMenu.setAttribute('aria-label', `“${project.name}”的操作`);
    this.projectMenu.hidden = false; opener.setAttribute('aria-expanded', 'true');
    const anchor = opener.getBoundingClientRect(), box = this.projectMenu.getBoundingClientRect();
    this.projectMenu.style.left = `${Math.max(8, Math.min(anchor.right - box.width, innerWidth - box.width - 8))}px`;
    this.projectMenu.style.top = `${Math.max(8, Math.min(anchor.bottom + 4, innerHeight - box.height - 8))}px`;
    remove.focus();
  }
  closeProjectMenu(restoreFocus = false) {
    if (this.projectMenu.hidden) return;
    this.projectMenu.hidden = true; this.projectMenu.replaceChildren();
    this.projectMenuOpener?.setAttribute('aria-expanded', 'false');
    if (restoreFocus && this.projectMenuOpener?.isConnected) this.projectMenuOpener.focus();
    this.projectMenuOpener = null;
  }
  async removeProject(project) {
    if (!this.projectState || !confirm(`移除项目“${project.name}”？文件和历史会话都将保留。`)) return;
    const generation = this.generation;
    try {
      const state = await this.api.removeProject(project.id, this.projectState.revision);
      if (generation !== this.generation) return;
      this.projectState = state; this.renderProjects(); await this.hooks.projectsChanged();
    } catch (error) { if (error.name !== 'AbortError' && generation === this.generation) this.$('#projects-notice').textContent = error.message; }
  }
  syncProjectSelection() {
    const selected = this.hooks.currentProject()?.id;
    for (const row of this.$('#project-list').querySelectorAll('.project-row')) {
      row.setAttribute('aria-pressed', String(row.closest('.project-group')?.dataset.projectId === selected));
    }
  }
  renderProjectAdd() {
    const add = this.$('#project-add');
    const mode = this.projectCreateMode();
    add.disabled = !this.available || mode === 'none' || this.projectPicking || this.projectSaving;
    add.setAttribute('aria-label', mode === 'none' ? '当前宿主无法添加项目' : '添加项目');
    add.dataset.tooltip = mode === 'name' ? '新建项目' : mode === 'folder' ? '选择项目文件夹' : '当前宿主无法添加项目';
  }
  projectCreateMode() {
    const mode = this.settings?.project_create_mode;
    return mode === 'folder' && typeof this.hooks.pickDesktopProject !== 'function' ? 'none' : mode || 'none';
  }
  closeProjectPicker() {
    const menu = this.$('#composer-project-menu');
    menu.hidden = true;
    this.$('#project-picker').setAttribute('aria-expanded', 'false');
  }
  toggleProjectPicker() {
    const menu = this.$('#composer-project-menu');
    if (!menu.hidden) { this.closeProjectPicker(); return; }
    this.renderProjectPicker();
    menu.hidden = false;
    this.$('#project-picker').setAttribute('aria-expanded', 'true');
    this.$('#project-menu-search').focus();
  }
  renderProjectPicker() {
    const picker = this.$('#project-picker'), menu = this.$('#composer-project-menu');
    const chip = picker.parentElement, detach = chip.querySelector('#project-detach');
    picker.hidden = chip.hidden = !this.available || !this.settings?.projects_enabled || Boolean(this.session);
    if (picker.hidden) { this.closeProjectPicker(); return; }
    const current = this.hooks.currentProject?.() || null;
    detach.hidden = !current;
    picker.setAttribute('aria-label', current ? `切换项目：${current.name}` : '选择项目');
    this.renderProjectPickerOptions();
  }
  async chooseProject(project) {
    const picker = this.$('#project-picker');
    try {
      if (await this.hooks.selectProject(project)) {
        this.closeProjectPicker(); this.syncProjectSelection(); this.renderProjectPicker(); picker.focus();
      }
    } catch (error) { this.$('#project-menu-list').append(node('p', 'workspace-empty', error?.message || '切换项目失败。')); }
  }
  renderProjectPickerOptions() {
    const list = this.$('#project-menu-list'), actions = this.$('#project-menu-actions');
    const query = this.$('#project-menu-search').value.trim().toLowerCase();
    const current = this.hooks.currentProject?.()?.id || null;
    const focusedProject = list.contains(document.activeElement) ? document.activeElement.textContent : null;
    list.replaceChildren();
    const matches = this.sortedProjects()
      .filter(project => `${project.name} ${project.path}`.toLowerCase().includes(query)).slice(0, 5);
    for (const project of matches) {
      const item = button('', 'composer-project-option'); item.setAttribute('role', 'menuitemradio');
      item.setAttribute('aria-checked', String(current === project.id));
      item.append(folderIcon(), node('span', '', project.name));
      if (current === project.id) item.append(node('span', 'project-menu-check', '✓'));
      item.addEventListener('click', () => { void this.chooseProject(project); });
      list.append(item);
    }
    if (!this.projectState) list.append(node('p', 'workspace-empty', this.projectLoading ? '正在加载项目…' : this.projectError || '项目列表暂不可用。'));
    else if (!matches.length) list.append(node('p', 'workspace-empty', '没有匹配的工作区'));
    if (focusedProject) [...list.querySelectorAll('button')].find(item => item.textContent === focusedProject)?.focus();
    actions.replaceChildren();
    const open = button('', 'composer-project-add'); open.setAttribute('role', 'menuitem');
    const mode = this.projectCreateMode();
    open.disabled = !this.projectState || mode === 'none' || this.projectPicking || this.projectSaving;
    open.dataset.tooltip = mode === 'name' ? '新建项目' : mode === 'folder' ? '选择项目文件夹' : '当前宿主无法添加项目';
    open.append(menuIcon('M3 7V5h6l2 2h10v12H3Zm9 4v6m-3-3h6'), node('span', '', mode === 'none' ? '此宿主无法添加项目' : '添加项目'));
    open.addEventListener('click', () => { this.closeProjectPicker(); void this.addProject(this.$('#project-picker')); });
    actions.append(open);
    const outside = button('', 'composer-project-outside'); outside.setAttribute('role', 'menuitemcheckbox');
    outside.setAttribute('aria-checked', String(!current));
    outside.append(menuIcon('M21 11.5a9 9 0 0 1-9 9H4l-2 2v-11a9 9 0 0 1 19 0Z'), node('span', '', '不在项目中工作'));
    if (!current) outside.append(node('span', 'project-menu-check', '✓'));
    outside.addEventListener('click', () => { void this.chooseProject(null); });
    actions.append(outside);
  }
  async addProject(opener = this.$('#project-add')) {
    const mode = this.projectCreateMode();
    if (mode === 'none' || this.projectPicking || this.projectSaving) return;
    if (!this.projectState) await this.refreshProjects();
    if (!this.projectState) return;
    if (mode === 'name') {
      this.projectDialogReturn = opener;
      this.$('#project-error').textContent = '';
      this.$('#project-name').value = '';
      this.$('#project-dialog').showModal(); this.$('#project-name').focus();
      return;
    }
    const generation = this.generation, previous = new Set(this.projectState.projects.map(project => project.id));
    this.projectPicking = true; this.renderProjectAdd(); this.renderProjectPickerOptions(); this.$('#projects-notice').textContent = '';
    try {
      const listing = await this.hooks.pickDesktopProject(crypto.randomUUID(), this.projectState.revision);
      if (generation !== this.generation || !listing) return;
      await this.acceptProjectListing(listing, previous);
    } catch (error) {
      if (error.name !== 'AbortError' && generation === this.generation) {
        if (error.status === 409) await this.refreshProjects();
        if (generation === this.generation) this.$('#projects-notice').textContent = error.message;
      }
    } finally {
      if (generation === this.generation) {
        this.projectPicking = false; this.renderProjectAdd(); this.renderProjectPickerOptions();
        if (opener.isConnected) opener.focus();
      }
    }
  }
  async saveNamedProject() {
    if (this.projectSaving || !this.projectState || this.projectCreateMode() !== 'name') return;
    const generation = this.generation, previous = new Set(this.projectState.projects.map(project => project.id));
    const name = this.$('#project-name').value.trim();
    this.projectSaving = true; this.$('#project-save').disabled = true; this.$('#project-cancel').disabled = true;
    try {
      const listing = await this.api.createProject(crypto.randomUUID(), this.projectState.revision, name);
      if (generation !== this.generation) return;
      await this.acceptProjectListing(listing, previous);
      this.projectDialogReturn = this.$('#prompt'); this.$('#project-dialog').close();
    } catch (error) {
      if (error.name !== 'AbortError' && generation === this.generation) {
        if (error.status === 409) await this.refreshProjects();
        if (generation === this.generation) this.$('#project-error').textContent = error.message;
      }
    } finally {
      if (generation === this.generation) {
        this.projectSaving = false; this.$('#project-save').disabled = false; this.$('#project-cancel').disabled = false;
        this.renderProjectAdd(); this.renderProjectPickerOptions();
      }
    }
  }
  async acceptProjectListing(listing, previous) {
    if (!Array.isArray(listing?.projects) || listing.projects.length > 256 || !Number.isSafeInteger(listing.revision)) throw new Error('项目列表响应无效。');
    this.projectState = listing; this.renderProjects();
    const project = listing.projects.find(item => !previous.has(item.id));
    if (project) await this.hooks.selectProject(project);
    this.syncProjectSelection(); this.renderProjectPicker();
  }
  setSession(session) {
    const stamp = session ? `${session.id}:${session.status}:${session.workspace}` : '';
    if (stamp === this.lastSessionStamp) return;
    const changed = session?.id !== this.session?.id;
    if (changed) {
      this.closeHeaderContext(); this.closeHeaderOpenMenu();
      if (this.headerOpenError && this.$('#projects-notice').textContent === this.headerOpenError) this.$('#projects-notice').textContent = '';
      this.headerOpenError = '';
    }
    this.lastSessionStamp = stamp; this.session = session;
    this.$('#workspace-files-open').hidden = !session || !this.available;
    this.syncProjectSelection(); this.renderProjectPicker(); this.syncSessionScope(); this.renderHeader();
    const browser = session && this.fileBrowsers.get('session:' + session.id);
    if (!changed && session?.status === 'completed' && browser && !browser.root.hidden) void browser.reload();
  }
  syncSessionScope() {
    this.$('#workspace-files-open').hidden = !this.session || !this.available;
    if (!this.available) { this.pane.setScope('none'); return; }
    if (!this.session) { const project = this.hooks.currentProject?.(); this.pane.setScope(project ? 'project:' + project.id : 'none'); return; }
    this.pane.setScope('session:' + this.session.id);
  }
  currentFileTarget() {
    const [kind, id] = this.pane.scope.split(':');
    if (kind === 'project') { const project = this.projectState?.projects.find(p => p.id === id); return project ? { kind, id, path: project.path } : null; }
    return this.session ? { kind: 'session', id: this.session.id, path: this.session.workspace } : null;
  }
  renderHeader() {
    const context = this.$('#header-workspace-context'), group = this.$('#header-open-group');
    context.hidden = !this.session || !this.available;
    const modes = this.hooks.desktopOpenWith?.() || [];
    group.hidden = !this.session || !this.available || !modes.length;
    if (context.hidden) this.closeHeaderContext();
    else {
      context.setAttribute('aria-label', `工作区信息：${this.session.metadata?.['project.name'] || this.session.workspace || '当前工作区'}`);
      if (!this.$('#header-workspace-popover').hidden) this.fillHeaderContext();
    }
    if (group.hidden) this.closeHeaderOpenMenu();
    if (!group.hidden && !modes.includes(this.headerOpenMode)) this.headerOpenMode = modes[0];
    const label = this.headerOpenMode === 'terminal' ? '在当前工作区打开终端' : '在当前工作区打开资源管理器';
    this.$('#header-open-workspace').setAttribute('aria-label', label);
    this.$('#header-open-workspace').dataset.tooltip = label;
    if (this.headerRenderedMode !== this.headerOpenMode) {
      this.$('#header-open-workspace').replaceChildren(this.headerOpenMode === 'terminal'
        ? menuIcon('M3 4h18v16H3zM7 9l3 3-3 3m5 4h5') : this.headerFolderIcon.cloneNode(true));
      this.headerRenderedMode = this.headerOpenMode;
    }
    if (!this.$('#header-open-menu').hidden) this.renderHeaderOpenMenu();
  }
  fillHeaderContext() {
    const info = this.$('#header-workspace-popover'), session = this.session;
    if (!session) { info.replaceChildren(); return; }
    const name = session.metadata?.['project.name'] || session.workspace?.split(/[\\/]/).filter(Boolean).at(-1) || '工作区';
    const path = node('span', 'header-context-path', session.workspace || '路径暂不可用');
    const activity = node('span', 'header-context-activity', `最近活动：${relativeTime(session.updated_at) || '暂无记录'}`);
    info.replaceChildren(node('strong', '', name), path, activity);
    if (this.headerBranch && this.headerBranch.path === session.workspace && this.headerBranch.name) {
      info.append(node('span', 'header-context-branch', `分支：${this.headerBranch.name}`));
    }
  }
  openHeaderContext() {
    if (!this.available || !this.session) return;
    clearTimeout(this.contextCloseTimer);
    const info = this.$('#header-workspace-popover');
    this.fillHeaderContext(); info.hidden = false;
    this.$('#header-workspace-context').setAttribute('aria-expanded', 'true');
    const target = this.currentFileTarget(), stamp = `${this.generation}:${this.session.id}:${this.session.workspace}`;
    if (!this.features?.review || !target || this.headerBranchStamp === stamp) return;
    this.headerBranchStamp = stamp;
    void this.api.review(target).then(result => {
      if (stamp !== this.headerBranchStamp || this.session?.id !== target.id) return;
      this.headerBranch = { path: this.session.workspace, name: result.available && typeof result.branch === 'string' ? result.branch : '' };
      if (!info.hidden) this.fillHeaderContext();
    }).catch(() => {});
  }
  scheduleContextClose() {
    clearTimeout(this.contextCloseTimer);
    this.contextCloseTimer = setTimeout(() => { if (!this.contextPinned) this.closeHeaderContext(); }, 120);
  }
  closeHeaderContext(restoreFocus = false) {
    clearTimeout(this.contextCloseTimer); this.contextPinned = false;
    const info = this.$('#header-workspace-popover');
    if (info.hidden) return;
    info.hidden = true; this.$('#header-workspace-context').setAttribute('aria-expanded', 'false');
    if (restoreFocus) this.$('#header-workspace-context').focus();
  }
  renderHeaderOpenMenu() {
    const menu = this.$('#header-open-menu'), selected = this.headerOpenMode;
    const options = (this.hooks.desktopOpenWith?.() || []).map(id => id === 'terminal'
      ? { id, label: '终端', icon: menuIcon('M3 4h18v16H3zM7 9l3 3-3 3m5 4h5') }
      : { id, label: '资源管理器', icon: this.headerFolderIcon.cloneNode(true) });
    menu.replaceChildren();
    for (const option of options) {
      const item = button('', 'session-menu-item'); item.setAttribute('role', 'menuitemradio');
      item.setAttribute('aria-checked', String(selected === option.id));
      item.append(option.icon, node('span', '', option.label));
      if (selected === option.id) item.append(node('span', 'header-menu-check', '✓'));
      item.addEventListener('click', () => {
        this.headerOpenMode = option.id;
        try { localStorage.setItem(HEADER_OPEN_MODE_KEY, option.id); } catch { /* Keep the current window selection. */ }
        this.closeHeaderOpenMenu(); this.renderHeader(); this.openHeaderTarget(option.id);
      });
      menu.append(item);
    }
  }
  toggleHeaderOpenMenu() {
    const menu = this.$('#header-open-menu');
    if (!menu.hidden) { this.closeHeaderOpenMenu(true); return; }
    if (this.$('#header-open-group').hidden) return;
    this.closeHeaderContext(); this.renderHeaderOpenMenu(); menu.hidden = false;
    this.$('#header-open-options').setAttribute('aria-expanded', 'true');
    menu.querySelector('button')?.focus();
  }
  closeHeaderOpenMenu(restoreFocus = false) {
    const menu = this.$('#header-open-menu'); if (menu.hidden) return;
    menu.hidden = true; this.$('#header-open-options').setAttribute('aria-expanded', 'false');
    if (restoreFocus) this.$('#header-open-options').focus();
  }
  async openHeaderTarget(id) {
    const sessionId = this.session?.id, generation = this.generation;
    if (!sessionId || !this.available || !(this.hooks.desktopOpenWith?.() || []).includes(id)) return;
    try {
      await this.hooks.openDesktopWorkspace(sessionId, id);
      if (generation === this.generation && this.session?.id === sessionId) {
        if (this.headerOpenError && this.$('#projects-notice').textContent === this.headerOpenError) this.$('#projects-notice').textContent = '';
        this.headerOpenError = '';
      }
    }
    catch (error) {
      if (generation === this.generation && this.session?.id === sessionId) {
        this.headerOpenError = typeof error === 'string' ? error : error?.message || '打开系统应用失败。';
        this.$('#projects-notice').textContent = this.headerOpenError;
      }
    }
  }
  openSessionFiles() { if (this.session) this.openFiles({ kind: 'session', id: this.session.id, path: this.session.workspace }); }
  openFiles(target, background = false) {
    if (!this.available) return;
    const scope = target.kind + ':' + target.id;
    if (this.pane.hasTab('files', scope)) { this.pane.activate('files', scope); return; }
    const view = new WorkspaceFiles({ api: this.api, target, openFile: path => this.openFile(path, target, scope) });
    this.fileBrowsers.set(scope, view);
    const tab = this.pane.openTab({ id: 'files', title: '文件', hint: target.path, kind: 'files', node: view.root, scope,
      activate: !background,
      onActivate: () => { if (!view.loaded && !view.loading) return view.load(); },
      onClose: () => { view.close(); this.fileBrowsers.delete(scope); }, reopen: () => this.openFiles(target) });
    if (!tab) { view.close(); this.fileBrowsers.delete(scope); }
  }
  settingsVisibility(value) { this.settingsVisible = value; this.pane.settingsVisibility(value); }
  async targetRead(target, path, options) {
    return target.kind === 'project' ? this.api.projectRead(target.id, path, options) : this.api.read(target.id, path, options);
  }
  presentation(session, base = '') {
    return session?.id ? this.targetPresentation({ kind: 'session', id: session.id, path: session.workspace }, base) : {};
  }
  targetPresentation(target, base = '') {
    const root = target.path, generation = this.generation;
    const scope = `${target.kind}:${target.id}`;
    const validate = value => {
      if (!this.available || generation !== this.generation) throw new Error('文件所属连接已失效，请重新打开会话。');
      return messageFileTarget(value, root, base);
    };
    return {
      contextKey: `${generation}:${scope}:${base}`,
      openFile: value => { const location = validate(value); return this.openFile(location.path, target, scope, location); },
      readMedia: (value, options) => this.targetRead(target, validate(value).path, options),
      describeFile: (value, { signal } = {}) => this.api.stat(target, validate(value).path, signal),
      onError: message => { this.pane.notice.textContent = message; },
    };
  }
  openFile(path, explicitTarget = this.currentFileTarget(), explicitScope = this.pane.scope, location) {
    if (!explicitTarget) return;
    const target = { ...explicitTarget }, scope = explicitScope, id = `file:${target.kind}:${target.id}:${path}`;
    if (this.pane.hasTab(id, scope)) { this.pane.activate(id, scope); if (location?.line) void this.tabStates.get(`${scope}\0${id}`)?.focusLine(location.line); return; }
    target.path ||= target.kind === 'session' && this.session?.id === target.id ? this.session.workspace : this.projectState?.projects.find(project => project.id === target.id)?.path;
    const preview = new FilePreview({ api: this.api, target, path, read: (t, p, options) => this.targetRead(t, p, options),
      presentation: this.targetPresentation(target, path.split('/').slice(0, -1).join('/')) });
    if (location?.line) preview.pendingLine = location.line;
    if (location?.mode) preview.mode = location.mode;
    if (location?.wrap) { preview.root.classList.add('is-wrapped'); preview.wrap.setAttribute('aria-pressed', 'true'); }
    this.tabStates.set(`${scope}\0${id}`, preview);
    const tab = this.pane.openTab({ id, title: fileName(path), hint: path, kind: 'file', node: preview.root, scope,
      fileState: () => ({ path, mode: preview.mode, wrap: preview.root.classList.contains('is-wrapped') }),
      activate: location?.background !== true,
      reopen: saved => this.openFile(path, target, scope, saved),
      onClose: () => { preview.close(); this.tabStates.delete(`${scope}\0${id}`); },
      onActivate: () => { if (!preview.revision && !preview.loading) void preview.load(true); } });
    if (!tab) { preview.close(); this.tabStates.delete(`${scope}\0${id}`); }
    else preview.onPreferenceChange = () => this.pane.saveScope(scope);
  }
}
