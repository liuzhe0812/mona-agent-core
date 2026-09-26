import { SessionsClient } from './sessions.mjs';
import { durationText, relativeTime } from './run-view.mjs';

const $ = selector => document.querySelector(selector);
const label = { idle: '未开始', running: '运行中', completed: '已保存', failed: '失败', interrupted: '已中断', cancelled: '已停止', timed_out: '超时', limited: '达到限制' };
const node = (tag, className, value = '') => { const n = document.createElement(tag); n.className = className; n.textContent = value; return n; };

const TASKS_COLLAPSED_KEY = 'mona.web.tasks.v1';
const SIDEBAR_GROUP_PAGE = 5;
const SESSION_MENU_VIEWPORT_GUTTER = 8;
const SESSION_MENU_ANCHOR_GAP = 4;
const TASK_NAV_LIMIT = 64;
function tasksCollapsed() {
  try { return localStorage.getItem(TASKS_COLLAPSED_KEY) === '1'; } catch { return false; }
}
function applyTasksCollapsed(collapsed) {
  $('.sessions-region')?.classList.toggle('is-collapsed', collapsed);
  $('#sessions-collapse')?.setAttribute('aria-expanded', String(!collapsed));
  try { localStorage.setItem(TASKS_COLLAPSED_KEY, collapsed ? '1' : '0'); }
  catch { /* 存储被拒绝时仍保留本次选择 */ }
}

// Reuse existing rows so a refresh never restarts a running task's spinner or drops its focus.
function syncChildren(container, nodes) {
  nodes.forEach((node, index) => {
    if (container.children[index] !== node) container.insertBefore(node, container.children[index] || null);
  });
  while (container.children.length > nodes.length) container.lastElementChild.remove();
}

const SVG_NS = 'http://www.w3.org/2000/svg';
function icon(...paths) {
  const svg = document.createElementNS(SVG_NS, 'svg');
  svg.setAttribute('class', 'line-icon');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  for (const definition of paths) {
    const path = document.createElementNS(SVG_NS, 'path');
    path.setAttribute('d', definition);
    svg.append(path);
  }
  return svg;
}
// One clean outline plus its needle, then tilted: the pin reads like the reference pushpin.
const PIN_PATHS = ['M12 17.5v4', 'M9 10.8a2 2 0 0 1-1.1 1.8l-1.8.9A2 2 0 0 0 5 15.2V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.8a2 2 0 0 0-1.1-1.8l-1.8-.9A2 2 0 0 1 15 10.8V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z'];
const PIN_TILT = 'rotate(-45deg)';
const ARCHIVE_PATHS = ['M4.5 7.5h15', 'M6 7.5v11h12v-11', 'M9.5 11.5h5'];
const RENAME_PATHS = ['M4.5 19.5h15', 'M14.5 4.5 18 8l-9 9H5.5v-3.5z'];
const UNREAD_PATHS = ['M12 5.5a6.5 6.5 0 1 1 0 13 6.5 6.5 0 0 1 0-13Z', 'M12 12h.01'];
const DELETE_PATHS = ['M5 7.5h14', 'M9.5 7.5V5h5v2.5', 'M7 7.5l.9 12h8.2l.9-12', 'M10.5 10.5v5.5M13.5 10.5v5.5'];

// The stamp slot carries the relative time. A settled failure takes its place; a running task keeps
// the time and instead spins in front of its title, like the reference list.
function rowMeta(entry) {
  const failed = ['failed', 'interrupted', 'limited', 'timed_out'];
  if (failed.includes(entry.status)) return { text: label[entry.status] || entry.status, failed: true };
  return { text: relativeTime(entry.updated_at), failed: false };
}

/** Conversation navigation only. Execution and its live reducer remain in the existing application. */
export class ConversationUI {
  constructor(hooks) {
    this.hooks = hooks; this.api = new SessionsClient();
    this.mode = 'none'; this.loading = false; this.selected = null; this.pending = null;
    this.generation = 0; this.listGeneration = 0; this.entries = []; this.nextOffset = null;
    this.createKey = null; this.searchTimer = undefined; this.searchGeneration = 0; this.results = [];
    this.menuOpener = null; this.rows = new Map();
    this.navigation = []; this.navigationIndex = -1; this.navigationReplay = null;
    this.project = null; this.projectsEnabled = false; this.projects = []; this.groupLimits = new Map(); this.loadingMore = false;
    applyTasksCollapsed(tasksCollapsed());
    $('#sessions-collapse').addEventListener('click', () => applyTasksCollapsed(!$('.sessions-region').classList.contains('is-collapsed')));
    $('#sessions-new').addEventListener('click', () => { if (this.newChat(null)) $('#prompt').focus(); });
    $('#task-back').addEventListener('click', () => { void this.navigateTask(-1); });
    $('#task-forward').addEventListener('click', () => { void this.navigateTask(1); });
    $('#sessions-refresh').addEventListener('click', () => { void this.refresh(); });
    $('#sessions-more').addEventListener('click', () => { void this.showMore('ordinary'); });
    this.showArchived = false;
    $('#sessions-options').addEventListener('click', event => { event.stopPropagation(); this.openOptions(); });
    $('#sessions-archived').addEventListener('click', () => { this.closeMenu(); void this.toggleArchived(); });
    $('#thread-more').addEventListener('click', event => {
      event.stopPropagation();
      if (this.selected) this.openMenu(this.selected, event.currentTarget, event.currentTarget, undefined, true);
    });
    $('#header-archive-dialog').addEventListener('click', event => {
      if (event.target !== $('#header-archive-dialog')) return;
      const box = event.target.getBoundingClientRect();
      if (event.clientX < box.left || event.clientX > box.right || event.clientY < box.top || event.clientY > box.bottom) event.target.close('cancel');
    });
    const menus = [$('#session-menu'), $('#sessions-options-menu')];
    document.addEventListener('pointerdown', event => {
      if (this.menuOpen() && !menus.some(menu => menu.contains(event.target)) && !this.menuOpener?.contains(event.target)) this.closeMenu();
    });
    $('#session-menu').addEventListener('keydown', event => {
      if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
      const items = [...$('#session-menu').querySelectorAll('button:not(:disabled)')];
      if (!items.length) return;
      event.preventDefault();
      const current = items.indexOf(document.activeElement);
      const next = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1
        : event.key === 'ArrowDown' ? (current + 1) % items.length : (current < 0 ? items.length - 1 : current - 1 + items.length) % items.length;
      items[next].focus();
    });
    document.addEventListener('scroll', () => { if (this.menuOpen()) this.closeMenu(); }, true);
    window.addEventListener('resize', () => { if (this.menuOpen()) this.closeMenu(); });
    document.addEventListener('keydown', event => {
      if (event.key !== 'Escape' || !this.menuOpen()) return;
      event.stopPropagation(); this.closeMenu(true);
    });
    // Search is a dialog behind the brand button, not a permanent sidebar field.
    $('#search-button').addEventListener('click', () => this.openSearch());
    $('#search-new').addEventListener('click', () => { this.closeSearch(); $('#new-chat').click(); });
    $('#search-settings').addEventListener('click', () => { this.closeSearch(); this.hooks.openSettings(); });
    $('#search-refresh').addEventListener('click', () => { void this.search($('#sessions-search').value.trim()); });
    const searchDialog = $('#search-dialog');
    searchDialog.addEventListener('close', () => { if (!searchDialog.open) { this.invalidateSearch(); $('#search-button').setAttribute('aria-expanded', 'false'); } });
    searchDialog.addEventListener('click', (event) => {
      if (event.target !== searchDialog) return;
      const rect = searchDialog.getBoundingClientRect();
      const inside = event.clientX >= rect.left && event.clientX <= rect.right
        && event.clientY >= rect.top && event.clientY <= rect.bottom;
      if (!inside) this.closeSearch();
    });
    $('#sessions-search').addEventListener('input', () => {
      this.invalidateSearch();
      this.searchTimer = setTimeout(() => { void this.search($('#sessions-search').value.trim()); }, 200);
    });
    $('#sessions-search').addEventListener('keydown', event => {
      if (!(event.ctrlKey || event.metaKey) || !/^[1-9]$/.test(event.key)) return;
      event.preventDefault(); this.openResult(Number(event.key) - 1);
    });
    document.addEventListener('keydown', event => {
      if (!(event.ctrlKey || event.metaKey) || event.key.toLowerCase() !== 'k' || document.querySelector('dialog[open]')) return;
      event.preventDefault(); this.openSearch();
    });
    document.addEventListener('keydown', event => {
      if (event.defaultPrevented || !(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey
        || document.querySelector('dialog[open]') || $('#chat-workspace').inert || $('#chat-workspace').hidden) return;
      const delta = event.code === 'BracketLeft' ? -1 : event.code === 'BracketRight' ? 1 : 0;
      if (!delta) return;
      event.preventDefault(); void this.navigateTask(delta);
    });
  }
  invalidateSearch() { this.searchGeneration++; clearTimeout(this.searchTimer); }
  openSearch() {
    this.invalidateSearch();
    const dialog = $('#search-dialog');
    if (!dialog.open) dialog.showModal();
    $('#search-button').setAttribute('aria-expanded', 'true');
    $('#sessions-search').value = '';
    if (this.persistent) this.render(this.entries, '还没有任务');
    else this.render([], this.mode === 'none' ? '连接后可以搜索本地任务。' : '当前宿主未提供本地会话存储，刷新或重启不会保留记录。');
    $('#sessions-search').focus();
  }
  closeSearch() { this.invalidateSearch(); const dialog = $('#search-dialog'); if (dialog.open) dialog.close(); }
  async search(query) {
    if (!this.persistent || !this.api.configured || !$('#search-dialog').open) return;
    const generation = ++this.searchGeneration;
    try {
      const page = await this.api.list({ offset: 0, q: query, archived: this.showArchived });
      if (generation !== this.searchGeneration) return;
      if (!page || !Array.isArray(page.sessions)) throw new Error('无效的会话列表');
      this.mode = 'persistent';
      this.render(page.sessions, query ? '没有匹配的任务' : '还没有任务');
    } catch (error) {
      if (generation !== this.searchGeneration) return;
      this.render([], `搜索失败：${error.message}`);
    }
  }
  render(entries, emptyText) {
    this.results = entries.slice(0, 9);
    const list = $('#search-results'); list.replaceChildren();
    if (!this.results.length) { list.append(node('p', 'search-empty', emptyText)); return; }
    this.results.forEach((entry, index) => {
      const button = node('button', 'search-result');
      button.type = 'button'; button.dataset.tooltip = entry.title;
      button.append(node('span', '', entry.title), node('small', '', label[entry.status] || entry.status), node('kbd', '', `Ctrl+${index + 1}`));
      button.addEventListener('click', () => { this.closeSearch(); void this.open(entry.id); });
      list.append(button);
    });
  }
  openResult(index) {
    const entry = this.results[index];
    if (!entry) return;
    this.closeSearch(); void this.open(entry.id);
  }
  get persistent() { return this.mode === 'persistent'; }
  get canSend() {
    return !this.loading && (this.mode === 'ephemeral' || (this.persistent
      && (this.selected?.status !== 'running' || this.hooks.busy())));
  }
  notify() { this.controls(); this.hooks.changed(); }
  notice(message = '', failure = false) {
    const n = $('#sessions-notice'); n.textContent = message; n.classList.toggle('error', failure);
    n.classList.toggle('is-routine', message === '已保存在宿主本地');
  }
  controls() {
    const busy = this.loading || this.hooks.busy();
    $('#sessions-search').disabled = busy || !this.persistent;
    $('#sessions-more').disabled = busy;
    $('#sessions-refresh').disabled = busy || !this.api.configured;
    for (const button of $('#chat-sidebar').querySelectorAll('.session-row')) button.disabled = busy;
    // Navigation flags stay available during a run; only an in-flight list write blocks them.
    for (const button of $('#chat-sidebar').querySelectorAll('.session-action')) button.disabled = this.loading;
    this.syncArchivedChoice();
    this.renderTaskNavigation();
  }
  renderTaskNavigation() {
    const busy = this.loading || this.hooks.busy();
    $('#task-back').disabled = busy || this.navigationIndex <= 0;
    $('#task-forward').disabled = busy || this.navigationIndex >= this.navigation.length - 1;
  }
  rememberTask() {
    const item = { id: this.selected?.id || null, projectId: this.selected ? null : this.project?.id || null };
    if (this.navigationReplay != null) {
      this.navigationIndex = this.navigationReplay; this.navigation[this.navigationIndex] = item; this.navigationReplay = null;
    } else {
      const current = this.navigation[this.navigationIndex];
      if (current?.id === item.id && current?.projectId === item.projectId) return;
      this.navigation = this.navigation.slice(0, this.navigationIndex + 1);
      this.navigation.push(item);
      if (this.navigation.length > TASK_NAV_LIMIT) this.navigation.shift();
      this.navigationIndex = this.navigation.length - 1;
    }
    this.renderTaskNavigation();
  }
  async navigateTask(delta) {
    const index = this.navigationIndex + delta;
    if (index < 0 || index >= this.navigation.length || this.loading || this.hooks.busy()) return;
    const target = this.navigation[index]; this.navigationReplay = index;
    if (target.id) await this.open(target.id);
    else this.newChat(target.projectId ? this.hooks.findProject?.(target.projectId) || null : null);
    if (this.navigationReplay != null) { this.navigationReplay = null; this.renderTaskNavigation(); }
  }
  forgetTaskNavigation(id) {
    this.navigationReplay = null;
    const before = this.navigation.slice(0, this.navigationIndex + 1).filter(item => item.id === id).length;
    this.navigation = this.navigation.filter(item => item.id !== id);
    this.navigationIndex = Math.max(-1, Math.min(this.navigation.length - 1, this.navigationIndex - before));
    this.renderTaskNavigation();
  }
  clear() {
    this.generation++; this.listGeneration++; this.api.clear(); this.mode = 'none'; this.loading = false;
    if ($('#header-archive-dialog').open) $('#header-archive-dialog').close('cancel');
    this.selected = null; this.pending = null; this.createKey = null; this.entries = []; this.project = null; this.projectsEnabled = false; this.projects = []; this.groupLimits.clear();
    this.navigation = []; this.navigationIndex = -1; this.navigationReplay = null;
    this.closeSearch(); this.closeMenu(); $('#sessions-search').value = ''; $('#search-results').replaceChildren(); this.results = [];
    $('#sessions-list').replaceChildren(); $('#sessions-more').hidden = true; this.rows.clear();
    for (const list of $('#project-list').querySelectorAll('.project-sessions')) list.replaceChildren();
    this.hooks.clear(); this.notice('连接后读取本地任务。'); this.notify();
  }
  ephemeral(message) { this.mode = 'ephemeral'; this.notice(message); this.notify(); }
  async configure(base, token, { projectsEnabled = false } = {}) {
    this.projectsEnabled = projectsEnabled;
    this.api.configure(base, token); this.mode = 'checking'; this.notify();
    const generation = this.generation;
    try {
      await this.refreshList();
      if (generation !== this.generation) return;
      const target = /^#session=([A-Za-z0-9_-]{1,64})$/.exec(location.hash)?.[1] || this.entries[0]?.id;
      if (target) await this.open(target);
    } catch (error) {
      if (error.name === 'AbortError' || generation !== this.generation) return;
      if (error.status === 404) this.ephemeral('当前宿主未提供本地会话存储；此连接仅支持临时任务，刷新或重启不会保留记录。');
      else { this.mode = 'error'; this.notice(`本地会话不可用：${error.message}。为避免丢失记录，已暂停发送，请刷新重试。`, true); this.notify(); }
    }
  }
  // Rows are reused across refreshes: rebuilding them restarted the running spinner on every update.
  createRow(entry) {
    const item = node('div', 'session-item'); item.dataset.sessionId = entry.id;
    const button = node('button', 'session-row');
    const glyph = node('span', 'session-glyph');
    const spinner = node('span', 'session-spinner');
    spinner.setAttribute('role', 'img');
    spinner.setAttribute('aria-label', '运行中');
    const unread = node('span', 'session-unread');
    unread.setAttribute('role', 'img');
    unread.setAttribute('aria-label', '未读');
    glyph.append(spinner, unread);
    const title = node('span', 'session-title');
    const stamp = node('span', 'session-meta');
    button.type = 'button'; button.dataset.sessionId = entry.id;
    button.append(glyph, title, stamp);
    // The row offers pin and the menu button; every other action lives in that menu.
    const actions = node('div', 'session-actions');
    const pin = node('button', 'session-action action-pin');
    pin.type = 'button'; pin.dataset.tooltip = '置顶任务'; pin.append(icon(...PIN_PATHS));
    const more = node('button', 'session-action action-more');
    more.type = 'button'; more.dataset.tooltip = '任务操作';
    more.setAttribute('aria-haspopup', 'menu');
    more.append(icon('M5 12h.01', 'M12 12h.01', 'M19 12h.01'));
    actions.append(pin, more);
    const record = { entry, item, button, glyph, spinner, unread, title, stamp, actions, pin, more };
    button.addEventListener('click', () => { void this.open(record.entry.id); });
    pin.addEventListener('click', event => { event.stopPropagation(); void this.togglePin(record.entry); });
    more.addEventListener('click', event => { event.stopPropagation(); this.openMenu(record.entry, more, item); });
    item.addEventListener('contextmenu', event => { event.preventDefault(); this.openMenu(record.entry, null, item, event); });
    item.append(button, actions);
    this.rows.set(entry.id, record);
    return record;
  }
  updateRow(entry) {
    const record = this.rows.get(entry.id) || this.createRow(entry);
    record.entry = entry;
    const selected = entry.id === this.selected?.id;
    record.button.className = `session-row${selected ? ' active' : ''}`;
    record.button.dataset.tooltip = entry.title;
    if (selected) record.button.setAttribute('aria-current', 'page');
    else record.button.removeAttribute('aria-current');
    record.title.textContent = entry.title;
    const meta = rowMeta(entry);
    record.stamp.className = `session-meta${meta.failed ? ' is-failed' : ''}`;
    record.stamp.textContent = meta.text;
    record.stamp.dataset.tooltip = `${label[entry.status] || entry.status} · ${new Date(entry.updated_at).toLocaleString()}`;
    record.pin.setAttribute('aria-label', `${entry.pinned ? '取消置顶' : '置顶'}“${entry.title}”`);
    record.pin.setAttribute('aria-pressed', String(entry.pinned));
    record.pin.dataset.tooltip = entry.pinned ? '取消置顶' : '置顶任务';
    record.more.setAttribute('aria-label', `“${entry.title}”的操作`);
    // The stamp and the actions share one slot: only one of them is ever painted.
    record.item.className = 'session-item';
    // Every row keeps the glyph slot, so switching between running and idle never moves the title.
    const running = entry.status === 'running';
    record.item.classList.toggle('is-running', running);
    record.spinner.setAttribute('aria-hidden', String(!running));
    // The unread mark yields to the spinner: a task being executed is not waiting to be read.
    record.item.classList.toggle('is-unread', Boolean(entry.unread));
    record.unread.setAttribute('aria-hidden', String(!entry.unread || running));
    return record;
  }
  setProjects(projects) {
    this.projects = projects;
    this.renderSidebar();
  }
  groupEntries(group, projectIds = new Set(this.projects.map(project => project.id))) {
    return this.entries.filter(entry => {
      const id = entry.metadata?.['project.id'];
      return group === 'ordinary' ? !id || !projectIds.has(id) : id === group;
    });
  }
  renderGroup(list, entries, group) {
    if (!list) return;
    const limit = this.groupLimits.get(group) || SIDEBAR_GROUP_PAGE;
    const rows = entries.slice(0, limit).map(entry => this.updateRow(entry).item);
    syncChildren(list, rows);
    const more = group === 'ordinary' ? $('#sessions-more') : list.parentElement.querySelector('.project-more');
    if (more) more.hidden = entries.length <= limit && this.nextOffset == null;
  }
  renderSidebar() {
    const projectIds = new Set(this.projects.map(project => project.id));
    for (const [id, record] of this.rows) if (!this.entries.some(entry => entry.id === id)) {
      record.item.remove(); this.rows.delete(id);
    }
    for (const project of this.projects) {
      const group = [...$('#project-list').querySelectorAll('.project-group')]
        .find(item => item.dataset.projectId === project.id);
      this.renderGroup(group?.querySelector('.project-sessions'), this.groupEntries(project.id, projectIds), project.id);
    }
    this.renderGroup($('#sessions-list'), this.groupEntries('ordinary', projectIds), 'ordinary');
  }
  async showMore(group) {
    if (this.loadingMore || !this.api.configured) return;
    this.loadingMore = true;
    const limit = (this.groupLimits.get(group) || SIDEBAR_GROUP_PAGE) + SIDEBAR_GROUP_PAGE;
    this.groupLimits.set(group, limit);
    try {
      while (this.groupEntries(group).length < limit && this.nextOffset != null) {
        const offset = this.nextOffset;
        await this.refreshList(offset);
        if (this.nextOffset === offset) break;
      }
      this.renderSidebar();
    } catch (error) { this.notice(error.message, true); }
    finally { this.loadingMore = false; }
  }
  async refreshList(offset = 0) {
    if (!this.api.configured) return;
    const generation = ++this.listGeneration;
    const page = await this.api.list({ offset: offset ?? 0, archived: this.showArchived });
    if (generation !== this.listGeneration) return;
    if (!page || !Array.isArray(page.sessions)) throw new Error('无效的会话列表');
    this.mode = 'persistent';
    this.entries = offset ? [...this.entries, ...page.sessions].filter((item, i, all) => all.findIndex(other => other.id === item.id) === i) : page.sessions;
    this.nextOffset = page.next_offset;
    this.renderSidebar();
    // Old or damaged files are omitted from navigation; only an attempted open needs a page error.
    this.notice('已保存在宿主本地');
    this.notify();
  }
  async refresh() {
    if (this.hooks.busy() || this.loading) return;
    try {
      await this.refreshList();
      if (this.selected) await this.open(this.selected.id);
    } catch (e) { this.mode = 'error'; this.notice(e.message, true); this.notify(); }
  }
  title() {
    const title = this.selected?.title || '新任务';
    $('#project-context').textContent = this.selected?.metadata?.['project.name'] || this.project?.name || '';
    $('#sessions-section-title').textContent = this.showArchived ? '已归档任务' : '任务';
    $('#thread-title').textContent = title;
    this.rememberTask();
    $('#thread-more').hidden = !this.selected;
    if (this.menuOpener === $('#thread-more')) this.closeMenu();
    if (this.selected) {
      // A fragment-only URL would resolve against index.html's <base>, not the current page.
      const url = new URL(location.href);
      url.hash = `session=${this.selected.id}`;
      history.replaceState(null, '', url);
    }
    this.controls();
  }
  newChat(project = null) {
    if (this.loading || this.hooks.busy()) { this.notice('请先停止当前任务并等待结算。', true); return false; }
    const scopeChanged = this.project?.id !== project?.id;
    this.project = project;
    this.generation++; this.selected = null; this.pending = null; this.createKey = null;
    this.nextBefore = null; this.historyLoading = false;
    if (scopeChanged) this.renderSidebar();
    this.hooks.clear(); history.replaceState(null, '', location.pathname + location.search); this.title();
    for (const button of $('#chat-sidebar').querySelectorAll('.session-row')) { button.classList.remove('active'); button.removeAttribute('aria-current'); }
    this.notify(); return true;
  }
  async selectProject(project) {
    if (!this.newChat(project)) return false;
    this.showArchived = false; this.syncArchivedChoice();
    $('#prompt').focus(); return true;
  }
  async ensureSession() {
    if (!this.persistent || this.loading || this.hooks.busy()) throw new Error('请等待会话就绪后再切换模式。');
    if (this.selected) return this.selected;
    const generation = this.generation; this.loading = true; this.notify();
    try {
      this.createKey ||= crypto.randomUUID();
      const selected = await this.api.create(this.createKey, this.project?.id);
      if (generation !== this.generation) throw new DOMException('会话连接已切换。', 'AbortError');
      this.selected = selected; this.title();
      await this.refreshList();
      if (generation !== this.generation) throw new DOMException('会话连接已切换。', 'AbortError');
      return this.selected;
    } finally { if (generation === this.generation) { this.loading = false; this.notify(); } }
  }
  async refreshHeader(id) {
    const generation = this.generation;
    const page = await this.api.get(id, { limit: 1 });
    if (generation !== this.generation || this.selected?.id !== id) return;
    if (page.session.revision < this.selected.revision) return;
    this.selected = page.session; this.title(); this.notify();
  }
  async submit(value) {
    if (!this.persistent || this.loading) throw new Error('会话尚未就绪。');
    if (this.pending && this.pending.prompt !== value) throw new Error('上一条消息的保存结果尚未确认，请先刷新历史；相同内容可使用原请求重试。');
    this.loading = true; this.notify();
    try {
      // A new turn makes an archived task active again instead of leaving it hidden.
      if (this.selected?.archived) {
        this.selected = await this.api.archive(this.selected.id, this.selected.revision, false);
        this.showArchived = false; this.syncArchivedChoice();
      }
      if (!this.selected) {
        this.createKey ||= crypto.randomUUID();
        this.selected = await this.api.create(this.createKey, this.project?.id); this.title();
      }
      this.pending ||= { request_id: crypto.randomUUID(), revision: this.selected.revision, prompt: value };
      const response = await this.api.start(this.selected.id, this.pending);
      this.selected = response.session; this.pending = null; this.title();
      void this.refreshList().catch(e => this.notice(e.message, true));
      return response;
    } catch (error) {
      this.notice(`${error.message} 请刷新历史确认；不会自动重复执行。`, true); throw error;
    } finally { this.loading = false; this.notify(); }
  }
  async settled() {
    if (!this.selected || !this.persistent) return;
    const id = this.selected.id, generation = this.generation;
    this.loading = true; this.notify();
    try {
      const page = await this.api.get(id, { limit: 1 });
      if (generation !== this.generation) return;
      this.selected = page.session;
      if (page.session.status === 'running') {
        // A failed durable checkpoint may leave an old running generation. Do not label it saved.
        this.notice('本轮尚未确认保存完成；请刷新检查，或重启宿主后检查中断记录。', true);
      } else { await this.refreshList(); }
      this.title();
    } catch (e) { if (e.name !== 'AbortError') this.notice(`无法确认本轮已保存：${e.message}`, true); }
    finally { if (generation === this.generation) { this.loading = false; this.notify(); } }
  }
  async open(id, before = undefined) {
    if (this.loading || this.hooks.busy()) { this.notice('请先停止当前任务并等待结算，再切换会话。', true); return; }
    this.loading = true; const generation = ++this.generation; this.notify();
    try {
      const cursor = before === undefined ? this.hooks.historyBefore?.(id) : before;
      let page = await this.api.get(id, { before: cursor ?? undefined, limit: 10 });
      if (generation !== this.generation) return;
      if (cursor != null && page.session.status === 'running') page = await this.api.get(id, { limit: 10 });
      if (generation !== this.generation) return;
      this.hasNewer = cursor != null && cursor < page.session.turn_count && page.session.status !== 'running';
      const oldScope = this.project?.id;
      this.selected = page.session; this.pending = null;
      this.project = this.hooks.findProject?.(page.session.metadata?.['project.id']) || null;
      this.hooks.clear(); this.title();
      if (oldScope !== this.project?.id) this.renderSidebar();
      if (this.selected.unread) this.markRead(this.selected);
      this.nextBefore = page.next_before; this.historyLoading = false;
      this.renderHistoryNavigation();
      for (const turn of page.turns) {
        if (generation !== this.generation) return;
        const dom = this.hooks.createTurn(turn.prompt, { session: page.session, turn, history: true }); dom.turn.dataset.turnId = turn.id;
        dom.turn.dataset.historyBefore = String((page.next_before || 0) + page.turns.length);
        if (turn.status === 'running' && turn.run_id) {
          await this.hooks.live(turn.run_id, dom);
        } else {
          await this.loadTurn(id, turn, dom, generation);
        }
      }
      if (page.session.status === 'interrupted') this.notice('上次进程已中断。已恢复确认过的记录；工具不会自动重跑，继续前请核对可能的外部影响。');
      this.hooks.historyOpened?.(id);
      for (const button of $('#chat-sidebar').querySelectorAll('.session-row')) {
        const selected = button.dataset.sessionId === id; button.classList.toggle('active', selected);
        if (selected) button.setAttribute('aria-current', 'page'); else button.removeAttribute('aria-current');
      }
    } catch (e) {
      if (e.name !== 'AbortError') {
        if (e.status === 404) this.forgetTaskNavigation(id);
        this.notice(`读取会话失败：${e.message}`, true);
      }
    }
    finally { if (generation === this.generation) { this.loading = false; this.notify(); } }
  }
  renderHistoryNavigation() {
    let nav = $('#timeline > .history-pagination');
    if (!nav) { nav = node('div', 'history-pagination'); $('#timeline').prepend(nav); }
    nav.replaceChildren();
    if (this.hasNewer && this.selected) {
      const latest = node('button', 'text-button', '返回最新对话'); latest.type = 'button'; latest.disabled = this.historyLoading;
      latest.addEventListener('click', () => { const id = this.selected.id; this.hooks.forgetReading?.(id); void this.open(id, null); }); nav.append(latest);
    }
    if (this.nextBefore != null) {
      const older = node('button', 'text-button', this.historyLoading ? '正在加载…' : '加载更早的对话'); older.type = 'button'; older.disabled = this.historyLoading;
      older.addEventListener('click', () => { void this.loadOlder().catch(error => this.notice(error.message, true)); }); nav.append(older);
    }
  }
  async loadOlder() {
    if (this.nextBefore == null || this.historyLoading || this.loading || !this.selected) return false;
    const id = this.selected.id, generation = this.generation, before = this.nextBefore;
    this.historyLoading = true; this.renderHistoryNavigation();
    try {
      const page = await this.api.get(id, { before, limit: 10 });
      if (generation !== this.generation || this.selected?.id !== id) return;
      if (page.next_before != null && page.next_before >= before) throw new Error('历史分页游标没有前进，已停止加载。');
      const first = $('#timeline .turn');
      for (const turn of page.turns) {
        if (generation !== this.generation) return;
        if ($('#timeline').querySelector(`[data-turn-id="${CSS.escape(turn.id)}"]`)) continue;
        const anchor = this.hooks.beforeHistoryChange?.();
        const dom = this.hooks.createTurn(turn.prompt, { session: page.session, turn, history: true, before: first }); dom.turn.dataset.turnId = turn.id;
        dom.turn.dataset.historyBefore = String((page.next_before || 0) + page.turns.length);
        this.hooks.afterHistoryChange?.(anchor);
        await this.loadTurn(id, turn, dom, generation);
      }
      this.nextBefore = page.next_before;
      return true;
    } finally {
      if (generation === this.generation) { const anchor = this.hooks.beforeHistoryChange?.(); this.historyLoading = false; this.renderHistoryNavigation(); this.hooks.afterHistoryChange?.(anchor); }
    }
  }
  async loadTurn(id, turn, dom, generation, before = undefined) {
    if (dom.historyLoading) return;
    dom.historyLoading = true;
    try {
      const page = await this.api.turn(id, turn.id, { before });
      if (generation !== this.generation) return;
      if (before != null && page.next_before != null && page.next_before >= before) throw new Error('执行记录分页没有前进，请刷新后重试。');
      const anchor = this.hooks.beforeHistoryChange?.();
      if (before != null && dom.historySnapshot) {
        const items = new Map([...page.snapshot.items, ...dom.historySnapshot.items].map(item => [item.id, item]));
        page.snapshot.items = [...items.values()]; page.snapshot.pruned_items = Math.max(0, page.total_items - items.size);
      }
      dom.historySnapshot = page.snapshot; dom.update(page.snapshot);
      dom.turn.querySelector('.history-load-error')?.remove();
      await dom.answerReady;
      if (generation !== this.generation || dom.disposed) return;
      if (anchor) this.hooks.afterHistoryChange?.(anchor);
      dom.clock.textContent = turn.finished_at == null ? '历史记录' : `已工作 ${durationText(turn.finished_at - turn.started_at)}`;
      dom.clock.dataset.tooltip = '宿主记录的本轮开始与结束时间';
      dom.setTimestamp(turn.finished_at || turn.started_at);
      dom.turn.querySelector('.history-tools-nav')?.remove();
      dom.turn.querySelector('.history-inputs')?.remove();
      const inputs = node('div', 'history-inputs');
      for (const value of page.supplemental_inputs) inputs.append(node('div', 'user-message supplement', value));
      dom.turn.append(inputs);
      dom.setSupplemental(page.supplemental_inputs.length > 0);
      const nav = node('div', 'history-tools-nav');
      if (page.next_before != null) {
        const older = node('button', 'text-button', '查看更早的执行记录'); older.type = 'button';
        older.addEventListener('click', () => { older.disabled = true; void this.loadTurn(id, turn, dom, generation, page.next_before); }); nav.append(older);
      }
      dom.turn.append(nav);
    } catch (e) {
      if (e.name !== 'AbortError' && generation === this.generation) {
        let note = dom.turn.querySelector('.history-load-error');
        if (!note) { note = node('div', 'history-load-error'); note.setAttribute('role', 'status'); dom.turn.append(note); }
        const retry = node('button', 'text-button', '重试读取'); retry.type = 'button';
        retry.addEventListener('click', () => { retry.disabled = true; void this.loadTurn(id, turn, dom, generation, before); });
        note.replaceChildren(node('span', '', `历史记录读取失败：${e.message}`), retry);
      }
    } finally { dom.historyLoading = false; for (const button of dom.turn.querySelectorAll('.history-tools-nav button,.history-load-error button')) button.disabled = false; }
  }
  busyForEdit() {
    if (this.loading || this.hooks.busy()) { this.notice('请先停止当前任务并等待结算。', true); return true; }
    return false;
  }
  placeMenu(menu, anchor, event, alignStart = false) {
    menu.hidden = false;
    const box = menu.getBoundingClientRect();
    const anchorBox = anchor.getBoundingClientRect();
    const point = event && Number.isFinite(event.clientX)
      ? { x: event.clientX, y: event.clientY }
      : { x: alignStart ? anchorBox.left : anchorBox.right - box.width, y: anchorBox.bottom + SESSION_MENU_ANCHOR_GAP };
    menu.style.left = `${Math.max(SESSION_MENU_VIEWPORT_GUTTER,
      Math.min(point.x, window.innerWidth - box.width - SESSION_MENU_VIEWPORT_GUTTER))}px`;
    menu.style.top = `${Math.max(SESSION_MENU_VIEWPORT_GUTTER,
      Math.min(point.y, window.innerHeight - box.height - SESSION_MENU_VIEWPORT_GUTTER))}px`;
  }
  openMenu(entry, opener, anchor, event, header = false) {
    const menu = $('#session-menu');
    if (!menu.hidden && this.menuOpener === opener) { this.closeMenu(true); return; }
    this.closeMenu();
    this.menuOpener = opener;
    menu.classList.toggle('is-header', header);
    menu.replaceChildren();
    for (const choice of header ? this.headerMenuChoices(entry) : this.menuChoices(entry)) {
      if (choice.separator) { const separator = node('div', 'session-menu-separator'); separator.setAttribute('role', 'separator'); menu.append(separator); continue; }
      const button = node('button', `session-menu-item${choice.danger ? ' is-danger' : ''}`);
      button.type = 'button'; button.setAttribute('role', 'menuitem'); button.disabled = Boolean(choice.disabled);
      if (choice.icon) button.append(icon(...choice.icon));
      button.append(node('span', '', choice.label));
      button.addEventListener('click', () => { this.closeMenu(); void choice.run(); });
      menu.append(button);
    }
    this.placeMenu(menu, anchor, event, header);
    opener?.setAttribute('aria-expanded', 'true');
    menu.querySelector('button:not(:disabled)')?.focus({ preventScroll: true });
  }
  // The section menu holds the list-wide actions: refresh and the archived view.
  openOptions() {
    const menu = $('#sessions-options-menu');
    if (!menu.hidden) { this.closeMenu(); return; }
    this.menuOpener = $('#sessions-options');
    this.syncArchivedChoice();
    this.placeMenu(menu, this.menuOpener);
    $('#sessions-options').setAttribute('aria-expanded', 'true');
    menu.querySelector('button:not(:disabled)')?.focus({ preventScroll: true });
  }
  syncArchivedChoice() {
    const button = $('#sessions-archived');
    button.textContent = this.showArchived ? '隐藏已归档任务' : '显示已归档任务';
    button.setAttribute('aria-pressed', String(this.showArchived));
    button.disabled = !this.api.configured || this.loading;
  }
  // Only operations the host actually stores, each with the icon the reference menu uses.
  menuChoices(entry) {
    const archived = this.showArchived;
    return [
      { label: '重命名任务', icon: RENAME_PATHS, run: () => this.renameEntry(entry) },
      { label: entry.pinned ? '取消置顶' : '置顶聊天', icon: PIN_PATHS, run: () => this.togglePin(entry) },
      { label: archived ? '取消归档' : '归档', icon: ARCHIVE_PATHS, run: () => this.toggleArchive(entry) },
      { label: entry.unread ? '标记为已读' : '标记为未读', icon: UNREAD_PATHS, run: () => this.toggleUnread(entry) },
      { label: '删除任务', icon: DELETE_PATHS, danger: true, run: () => this.removeEntry(entry) },
    ];
  }
  headerMenuChoices(entry) {
    const current = () => this.selected?.id === entry.id ? this.selected : entry;
    const choices = [
      { label: entry.pinned ? '取消置顶' : '置顶聊天', run: () => this.togglePin(current()) },
      { label: '重命名任务', disabled: this.loading || this.hooks.busy(), run: () => this.renameEntry(current()) },
      { label: entry.archived ? '取消归档' : '归档', run: async () => {
        if (!entry.archived && !await this.confirmHeaderArchive(entry)) return;
        if (this.selected?.id === entry.id) return this.toggleArchive(current());
      } },
      { label: entry.unread ? '标记为已读' : '标记为未读', run: () => this.toggleUnread(current()) },
    ];
    const hasFiles = !$('#workspace-files-open').hidden && Boolean(entry.workspace);
    if (hasFiles || entry.workspace || entry.id) choices.push({ separator: true });
    if (hasFiles) choices.push({ label: '打开任务文件夹', run: () => $('#workspace-files-open').click() });
    if (entry.workspace) choices.push({ label: '复制工作区路径', run: () => this.copyHeaderText(entry.workspace, '工作区路径') });
    if (entry.id) choices.push({ label: '复制会话 ID', run: () => this.copyHeaderText(entry.id, '会话 ID') });
    return choices;
  }
  async copyHeaderText(value, label) {
    try { await navigator.clipboard.writeText(value); this.notice(`已复制${label}。`); }
    catch { this.notice(`无法复制${label}，请检查浏览器剪贴板权限。`, true); }
  }
  confirmHeaderArchive(entry) {
    const dialog = $('#header-archive-dialog');
    if (dialog.open) return Promise.resolve(false);
    $('#header-archive-description').textContent = `归档“${entry.title}”？任务会从当前列表移出，可在已归档任务中恢复。`;
    dialog.returnValue = '';
    return new Promise(resolve => {
      dialog.addEventListener('close', () => {
        const confirmed = dialog.returnValue === 'archive';
        if (!$('#thread-more').hidden) $('#thread-more').focus();
        resolve(confirmed);
      }, { once: true });
      dialog.showModal(); dialog.querySelector('[value="cancel"]').focus();
    });
  }
  // Opening a task clears its mark. The write is best effort, and its new revision is kept so a
  // following turn or menu action does not collide with it.
  markRead(header) {
    const entry = this.entries.find(item => item.id === header.id);
    if (entry) { entry.unread = false; this.updateRow(entry); }
    void this.api.unread(header.id, header.revision, false).then(updated => {
      if (this.selected?.id === updated.id) this.selected = updated;
      const row = this.entries.find(item => item.id === updated.id);
      if (row) row.revision = updated.revision;
    }).catch(() => {});
  }
  menuOpen() { return !$('#session-menu').hidden || !$('#sessions-options-menu').hidden; }
  closeMenu(restoreFocus = false) {
    const menu = $('#session-menu');
    const options = $('#sessions-options-menu');
    const open = !menu.hidden || !options.hidden;
    if (!menu.hidden) { menu.hidden = true; menu.replaceChildren(); menu.classList.remove('is-header'); }
    if (!options.hidden) { options.hidden = true; $('#sessions-options').setAttribute('aria-expanded', 'false'); }
    this.menuOpener?.setAttribute('aria-expanded', 'false');
    if (restoreFocus && open && this.menuOpener?.isConnected) this.menuOpener.focus();
    this.menuOpener = null;
  }
  // Pinning and archiving carry no execution meaning, so they are allowed while a task runs.
  async setFlag(entry, request, message) {
    if (this.loading) return;
    this.loading = true; this.notify();
    try {
      const header = await request();
      if (this.selected?.id === entry.id) this.selected = header;
      await this.refreshList();
      // The list refresh writes its own notice, so the flag result is stated afterwards.
      this.notice(message);
    } catch (error) { this.notice(error.message, true); }
    finally { this.loading = false; this.notify(); }
  }
  togglePin(entry) {
    return this.setFlag(entry, () => this.api.pin(entry.id, entry.revision, !entry.pinned),
      entry.pinned ? '已取消置顶。' : '已置顶该任务。');
  }
  toggleArchive(entry) {
    const archived = !entry.archived;
    return this.setFlag(entry, () => this.api.archive(entry.id, entry.revision, archived),
      archived ? '已归档该任务；可在任务列表操作里查看已归档。' : '已取消归档。');
  }
  toggleUnread(entry) {
    return this.setFlag(entry, () => this.api.unread(entry.id, entry.revision, !entry.unread),
      entry.unread ? '已标记为已读。' : '已标记为未读；打开该任务会清除标记。');
  }
  async toggleArchived() {
    this.showArchived = !this.showArchived;
    this.groupLimits.clear(); this.title();
    this.syncArchivedChoice();
    this.notice(this.showArchived ? '正在显示已归档任务。' : '已返回当前任务。');
    try { await this.refreshList(); } catch (error) { this.notice(error.message, true); }
  }
  rename() { if (this.selected) return this.renameEntry(this.selected); }
  remove() { if (this.selected) return this.removeEntry(this.selected); }
  async renameEntry(entry) {
    if (this.busyForEdit()) return;
    const title = globalThis.prompt('任务名称（最多 80 个字符）', entry.title);
    if (title == null || title === entry.title) return;
    this.loading = true; this.notify();
    try {
      const header = await this.api.rename(entry.id, entry.revision, title);
      if (this.selected?.id === entry.id) this.selected = header;
      this.title(); await this.refreshList();
    } catch (error) { this.notice(error.message, true); }
    finally { this.loading = false; this.notify(); }
  }
  async removeEntry(entry) {
    if (this.busyForEdit()) return;
    if (!confirm(`删除任务“${entry.title}”？本地对话记录将被删除，工作文件会保留。此操作不能撤销。`)) return;
    this.loading = true; this.notify();
    try {
      await this.api.delete(entry.id, entry.revision);
      this.forgetTaskNavigation(entry.id);
      if (this.selected?.id === entry.id) { this.loading = false; this.newChat(); this.loading = true; }
      await this.refreshList();
    } catch (error) { this.notice(error.message, true); }
    finally { this.loading = false; this.notify(); }
  }
}
