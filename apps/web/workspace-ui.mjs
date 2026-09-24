import { WorkspaceClient } from './workspace.mjs';
import { FilePreview } from './file-preview.mjs';
const $ = selector => document.querySelector(selector);
const node = (tag, className, value = '') => { const n = document.createElement(tag); n.className = className; n.textContent = value; return n; };
const button = (label, className = 'text-button') => { const n = node('button', className, label); n.type = 'button'; return n; };
function folderIcon() { const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); svg.setAttribute('viewBox', '0 0 24 24'); svg.setAttribute('class', 'line-icon'); svg.setAttribute('aria-hidden', 'true'); const path = document.createElementNS(svg.namespaceURI, 'path'); path.setAttribute('d', 'M3 7V5h6l2 2h10v12H3Z'); svg.append(path); return svg; }
const sizeLabel = value => value < 1024 ? `${value} B` : value < 1024 * 1024 ? `${(value / 1024).toFixed(1)} KB` : `${(value / 1048576).toFixed(1)} MB`;
const fileName = path => path.split('/').at(-1) || path;
const markdownFile = path => /\.(?:md|mdx|markdown)$/i.test(path);
export class WorkspaceUI {
  constructor(hooks) {
    this.hooks = hooks; this.pane = hooks.pane; this.api = new WorkspaceClient(); this.available = false; this.settings = null;
    this.projectState = null; this.session = null; this.generation = 0; this.requestGeneration = 0;
    this.readAbort = new AbortController(); this.directory = ''; this.directoryVersion = null; this.nextDirectory = null; this.fileTarget = null;
    this.tabStates = new Map(); this.searchTimer = 0;
    this.settingsVisible = false;
    $('#workspace-files-open').addEventListener('click', () => { if (this.session) void this.openSessionFiles(); });
    $('#files-back-tasks').addEventListener('click', () => this.backToTasks());
    $('#files-refresh').addEventListener('click', () => { void this.refreshTree(); });
    $('#files-parent').addEventListener('click', () => { void this.list(this.directory.split('/').slice(0, -1).join('/')); });
    $('#files-more').addEventListener('click', () => { void this.list(this.directory, this.nextDirectory); });
    $('#files-search').addEventListener('input', () => {
      clearTimeout(this.searchTimer);
      this.searchTimer = setTimeout(() => { void this.search($('#files-search').value.trim()); }, 160);
    });
    $('#workspace-settings-refresh').addEventListener('click', () => { void this.refreshSettings(); });
    $('#workspace-root-form').addEventListener('submit', event => { event.preventDefault(); void this.saveRoot(); });
    $('#project-add').addEventListener('click', () => this.addDialog());
    $('#project-cancel').addEventListener('click', () => { if (!this.projectSaving) $('#project-dialog').close(); });
    $('#project-dialog').addEventListener('cancel', event => { if (this.projectSaving) event.preventDefault(); });
    $('#project-form').addEventListener('submit', event => { event.preventDefault(); void this.saveProject(); });
    $('#project-dialog').addEventListener('close', () => $('#project-add').focus());
    $('#project-dialog').addEventListener('click', event => {
      const dialog = $('#project-dialog'), r = dialog.getBoundingClientRect();
      if (!this.projectSaving && event.target === dialog && (event.clientX < r.left || event.clientX > r.right || event.clientY < r.top || event.clientY > r.bottom)) dialog.close();
    });
    this.pane.registerAction({
      id: 'files', label: '浏览工作区文件',
      icon: 'folder', available: () => Boolean(this.available && (this.session || this.hooks.currentProject?.())),
      run: () => {
        const project = this.hooks.currentProject?.();
        if (project) void this.openProjectFiles(project);
        else if (this.session) void this.openSessionFiles();
      },
    });
    this.renderSettings(); this.renderProjects();
  }
  clear() {
    this.generation++; this.api.clear(); this.cancelReads(); this.available = false; this.settings = null; this.projectState = null;
    this.session = null; this.lastSessionStamp = ''; this.fileTarget = null; $('#project-list').replaceChildren(); $('#projects-region').hidden = true;
    this.backToTasks(); this.resetTree(); this.renderSettings(); this.renderProjects(); this.pane.clear(); this.tabStates.clear(); this.features = {};
  }
  async configure(base, token) {
    this.api.configure(base, token); const generation = ++this.generation;
    try {
      const settings = await this.api.settings(); if (generation !== this.generation) return;
      this.applySettings(settings); this.available = true;
      if (settings.projects_enabled) await this.refreshProjects();
      try { this.features = await this.api.workbenchCapabilities(); } catch (e) { if (e.name === 'AbortError') throw e; this.features = {}; }
      if (generation !== this.generation) return;
      this.pane.renderActions();
      this.syncSessionScope();
    } catch (e) {
      if (generation !== this.generation || e.name === 'AbortError') return;
      this.available = false; this.settings = null; this.renderSettings(); this.renderProjects(); this.pane.setScope('none');
      $('#workspace-notice').textContent = [404, 501].includes(e.status) ? '当前宿主未提供工作区管理。' : e.message;
    }
  }
  applySettings(value) {
    if (!Number.isSafeInteger(value?.revision) || typeof value.default_root !== 'string' || typeof value.projects_enabled !== 'boolean') throw new Error('工作区设置响应无效。');
    this.settings = value; this.renderSettings(); $('#projects-region').hidden = !value.projects_enabled;
  }
  renderSettings() {
    const field = $('#workspace-root'); field.value = this.settings?.default_root || ''; field.disabled = !this.settings || this.settings.locked || Boolean(this.rootLoading || this.rootSaving);
    $('#workspace-root-save').disabled = field.disabled; $('#workspace-settings-refresh').disabled = !this.api.configured || Boolean(this.rootLoading || this.rootSaving);
    $('#workspace-notice').textContent = this.settings?.locked ? '默认根目录由部署配置指定。' : !this.settings ? '当前连接未提供工作区管理。' : '';
  }
  async refreshSettings() {
    if (this.rootSaving || this.rootLoading) return;
    const generation = this.generation; this.rootLoading = true; this.renderSettings(); let notice = '';
    try { this.applySettings(await this.api.settings()); }
    catch (e) { if (e.name !== 'AbortError') notice = e.message; }
    finally {
      this.rootLoading = false;
      if (generation === this.generation) { this.renderSettings(); if (notice) $('#workspace-notice').textContent = notice; }
    }
  }
  async saveRoot() {
    if (!this.settings || this.settings.locked || this.rootSaving || this.rootLoading) return;
    const root = $('#workspace-root').value.trim(), revision = this.settings.revision, generation = this.generation;
    this.rootSaving = true; $('#workspace-root').disabled = true; $('#workspace-root-save').disabled = true; $('#workspace-settings-refresh').disabled = true;
    $('#workspace-notice').textContent = '正在保存…'; let notice = '';
    try { this.applySettings(await this.api.saveRoot(revision, root)); notice = '已保存，新建普通会话时生效。'; }
    catch (e) { if (e.name !== 'AbortError') notice = e.message; }
    finally {
      this.rootSaving = false;
      if (generation === this.generation) { this.renderSettings(); $('#workspace-notice').textContent = notice; }
    }
  }
  async refreshProjects() {
    if (!this.settings?.projects_enabled) return;
    const generation = this.generation;
    try {
      const value = await this.api.projects(); if (generation !== this.generation) return;
      if (!Array.isArray(value.projects) || value.projects.length > 256 || !Number.isSafeInteger(value.revision)) throw new Error('项目列表响应无效。');
      this.projectState = value; this.renderProjects(); $('#projects-notice').textContent = '';
    } catch (e) { if (e.name !== 'AbortError' && generation === this.generation) $('#projects-notice').textContent = e.message; }
  }
  renderProjects() {
    const list = $('#project-list'); list.replaceChildren();
    for (const project of this.projectState?.projects || []) {
      const row = node('div', 'project-item'); row.dataset.projectId = project.id;
      const open = button('', 'project-row'); open.dataset.tooltip = project.path;
      open.append(folderIcon(), node('span', '', project.name));
      open.setAttribute('aria-pressed', String(this.hooks.currentProject()?.id === project.id));
      open.addEventListener('click', () => { void this.hooks.selectProject(project).then(() => this.renderProjects()); });
      const files = button('', 'icon-button project-files'); files.setAttribute('aria-label', `查看“${project.name}”文件`); files.dataset.tooltip = '查看文件'; files.append(folderIcon());
      files.addEventListener('click', event => { event.stopPropagation(); void this.openProjectFiles(project); });
      const remove = button('×', 'icon-button project-remove'); remove.setAttribute('aria-label', `移除项目“${project.name}”`); remove.dataset.tooltip = '移除项目登记';
      remove.addEventListener('click', async event => {
        event.stopPropagation();
        if (!confirm(`移除项目“${project.name}”？文件和历史会话都将保留。`)) return;
        remove.disabled = true;
        try { this.projectState = await this.api.removeProject(project.id, this.projectState.revision); this.renderProjects(); await this.hooks.projectsChanged(); }
        catch (e) { if (e.name !== 'AbortError') $('#projects-notice').textContent = e.message; }
        finally { remove.disabled = false; }
      });
      row.append(open, files, remove); list.append(row);
    }
    if (!list.children.length) list.append(node('p', 'workspace-empty', '尚未添加项目'));
  }
  addDialog() {
    if (!this.projectState) { void this.refreshProjects(); return; }
    this.projectRequest = crypto.randomUUID(); $('#project-name').value = ''; $('#project-path').value = ''; $('#project-error').textContent = '';
    $('#project-dialog').showModal(); $('#project-path').focus();
  }
  async saveProject() {
    if (this.projectSaving || !this.projectState) return;
    this.projectSaving = true; $('#project-save').disabled = true; $('#project-cancel').disabled = true;
    try {
      this.projectState = await this.api.addProject({ request_id: this.projectRequest, revision: this.projectState.revision, name: $('#project-name').value.trim(), path: $('#project-path').value.trim() });
      $('#project-dialog').close(); this.renderProjects();
    } catch (e) { if (e.name !== 'AbortError') $('#project-error').textContent = e.message; }
    finally { this.projectSaving = false; $('#project-save').disabled = false; $('#project-cancel').disabled = false; }
  }
  setSession(session) {
    const stamp = session ? `${session.id}:${session.status}:${session.workspace}` : '';
    if (stamp === this.lastSessionStamp) return;
    const changed = session?.id !== this.session?.id; this.lastSessionStamp = stamp; this.session = session;
    if (changed) { this.cancelReads(); this.resetTree(); this.backToTasks(); }
    $('#workspace-files-open').hidden = !session;
    this.renderProjects(); this.syncSessionScope();
    if (!changed && session?.status === 'completed' && this.fileTarget) void this.refreshTree();
  }
  cancelReads() { this.requestGeneration++; this.readAbort.abort(); this.readAbort = new AbortController(); }
  syncSessionScope() {
    if (!this.available) { this.pane.setScope('none'); return; }
    if (!this.session) { const project = this.hooks.currentProject?.(); this.pane.setScope(project ? `project:${project.id}` : 'none'); return; }
    this.pane.setScope(`session:${this.session.id}`);
  }
  async openSessionFiles() {
    if (!this.session) return;
    this.fileTarget = { kind: 'session', id: this.session.id, name: this.session.title || '当前任务', path: this.session.workspace || '' };
    this.pane.setScope(`session:${this.session.id}`); await this.enterFileMode();
  }
  async openProjectFiles(project) {
    this.fileTarget = { kind: 'project', id: project.id, name: project.name, path: project.path };
    this.pane.setScope(`project:${project.id}`); await this.enterFileMode();
  }
  async enterFileMode() {
    this.cancelReads(); this.resetTree(); $('#workspace-file-sidebar').hidden = false; $('#chat-sidebar').classList.add('files-mode');
    $('#sidebar-workspace-name').textContent = this.fileTarget.name; $('#workspace-path').textContent = this.fileTarget.path || '';
    $('#workspace-path').dataset.tooltip = this.fileTarget.path || ''; $('#files-search').value = '';
    await this.list('');
  }
  backToTasks() {
    $('#chat-sidebar').classList.remove('files-mode'); $('#workspace-file-sidebar').hidden = true;
    this.cancelReads(); this.fileTarget = null; this.resetTree(); this.syncSessionScope();
  }
  resetTree() {
    this.directory = ''; this.directoryVersion = null; this.nextDirectory = null;
    $('#files-current').textContent = '/'; $('#files-tree').replaceChildren(); $('#files-more').hidden = true; $('#files-parent').disabled = true;
    $('#files-notice').textContent = '';
  }
  settingsVisibility(value) { this.settingsVisible = value; if (value) this.backToTasks(); this.pane.settingsVisibility(value); }
  async targetList(path, options) {
    return this.fileTarget.kind === 'project' ? this.api.projectList(this.fileTarget.id, path, options) : this.api.list(this.fileTarget.id, path, options);
  }
  async targetRead(target, path, options) {
    return target.kind === 'project' ? this.api.projectRead(target.id, path, options) : this.api.read(target.id, path, options);
  }
  async targetSearch(query, options) {
    return this.fileTarget.kind === 'project' ? this.api.projectSearch(this.fileTarget.id, query, options) : this.api.search(this.fileTarget.id, query, options);
  }
  async refreshTree() {
    if (!this.fileTarget) return;
    const path = this.directory; this.cancelReads(); this.directoryVersion = null; this.nextDirectory = null;
    if ($('#files-search').value.trim()) await this.search($('#files-search').value.trim()); else await this.list(path);
  }
  async list(path, offset = 0) {
    if (!this.fileTarget) return;
    const generation = this.requestGeneration, request = (this.listRequest || 0) + 1; this.listRequest = request;
    $('#files-more').disabled = true;
    try {
      const page = await this.targetList(path, { offset, revision: offset ? this.directoryVersion : undefined, signal: this.readAbort.signal });
      if (generation !== this.requestGeneration || request !== this.listRequest) return;
      if (!offset) $('#files-tree').replaceChildren();
      this.directory = path; this.directoryVersion = page.revision; this.nextDirectory = page.next_offset;
      $('#files-current').textContent = path || '/'; $('#files-parent').disabled = !path; this.renderEntries(page.entries, false);
      $('#files-more').hidden = page.next_offset == null; $('#files-notice').textContent = !offset && !page.entries.length ? '此目录暂无文件。' : '';
    } catch (e) { if (e.name !== 'AbortError' && generation === this.requestGeneration && request === this.listRequest) $('#files-notice').textContent = e.message; }
    finally { if (request === this.listRequest) $('#files-more').disabled = false; }
  }
  async search(query) {
    if (!this.fileTarget) return;
    if (!query) { this.cancelReads(); await this.list(this.directory || ''); return; }
    this.cancelReads(); const generation = this.requestGeneration, request = (this.listRequest || 0) + 1; this.listRequest = request;
    $('#files-notice').textContent = '正在搜索…'; $('#files-more').hidden = true; $('#files-parent').disabled = true;
    try {
      const result = await this.targetSearch(query, { signal: this.readAbort.signal });
      if (generation !== this.requestGeneration || request !== this.listRequest) return;
      $('#files-tree').replaceChildren(); this.renderEntries(result.entries, true); $('#files-current').textContent = `搜索：${query}`;
      $('#files-notice').textContent = result.entries.length ? '' : '没有匹配的文件。';
    } catch (e) { if (e.name !== 'AbortError' && generation === this.requestGeneration) $('#files-notice').textContent = e.message; }
  }
  renderEntries(entries, search, container = $('#files-tree')) {
    for (const entry of entries) {
      if (entry.kind === 'directory' && !search) { container.append(this.directoryNode(entry)); continue; }
      const item = button('', 'file-entry'); item.dataset.path = entry.path;
      const icon = entry.kind === 'directory' ? folderIcon() : node('span', 'file-type-icon', entry.kind === 'link' ? 'link' : '·');
      item.append(icon, node('span', 'file-entry-name', search ? entry.path : entry.name), node('small', '', entry.kind === 'file' ? sizeLabel(entry.bytes) : ''));
      item.disabled = !['directory', 'file'].includes(entry.kind); item.dataset.tooltip = entry.kind === 'link' ? '链接不可打开' : entry.path;
      item.addEventListener('click', () => { if (entry.kind === 'directory') { $('#files-search').value = ''; void this.list(entry.path); } else this.openFile(entry.path); });
      container.append(item);
    }
  }
  directoryNode(entry) {
    const branch = document.createElement('details'); branch.className = 'file-directory';
    const summary = document.createElement('summary'); summary.className = 'file-entry'; summary.dataset.path = entry.path; summary.dataset.tooltip = entry.path;
    summary.append(node('span', 'file-chevron', '›'), folderIcon(), node('span', 'file-entry-name', entry.name));
    const children = node('div', 'file-branch'); children.setAttribute('role', 'group');
    branch.append(summary, children); let loaded = false, loading = false, revision;
    const generation = this.requestGeneration;
    const load = async (offset = 0) => {
      if (loading || !branch.isConnected || generation !== this.requestGeneration) return; loading = true;
      const note = node('p', 'workspace-empty', '正在读取…'); children.append(note);
      try {
        const page = await this.targetList(entry.path, { offset, revision: offset ? revision : undefined, signal: this.readAbort.signal });
        if (generation !== this.requestGeneration || !branch.isConnected) return;
        if (!offset) children.replaceChildren(); else note.remove();
        revision = page.revision; this.renderEntries(page.entries, false, children); loaded = true;
        if (page.next_offset != null) {
          const more = button('加载更多', 'text-button'); children.append(more);
          more.addEventListener('click', () => { more.remove(); void load(page.next_offset); });
        } else if (!children.childElementCount) children.append(node('p', 'workspace-empty', '空文件夹'));
      } catch (error) { if (error.name !== 'AbortError' && generation === this.requestGeneration) note.textContent = error.message; }
      finally { loading = false; }
    };
    branch.addEventListener('toggle', () => { if (branch.open && !loaded) void load(); }); return branch;
  }
  openFile(path, explicitTarget = this.fileTarget, explicitScope = this.pane.scope) {
    if (!explicitTarget) return;
    const target = { ...explicitTarget }, scope = explicitScope, id = `file:${target.kind}:${target.id}:${path}`;
    if (this.pane.hasTab(id, scope)) { this.pane.activate(id, scope); return; }
    const preview = new FilePreview({ api: this.api, target, path, read: (t, p, options) => this.targetRead(t, p, options) });
    this.tabStates.set(`${scope}\0${id}`, preview);
    this.pane.openTab({ id, title: fileName(path), hint: path, kind: 'file', node: preview.root, scope,
      reopen: () => this.openFile(path, target, scope),
      onClose: () => { preview.close(); this.tabStates.delete(`${scope}\0${id}`); },
      onActivate: () => { if (!preview.revision && !preview.loading) void preview.load(true); } });
  }
}
