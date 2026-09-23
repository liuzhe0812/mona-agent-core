import { WorkspaceClient } from './workspace.mjs';
const $ = selector => document.querySelector(selector);
const node = (tag, className, value = '') => { const n = document.createElement(tag); n.className = className; n.textContent = value; return n; };
const button = (label, className = 'text-button') => { const n = node('button', className, label); n.type = 'button'; return n; };
function folderIcon() { const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); svg.setAttribute('viewBox', '0 0 24 24'); svg.setAttribute('class', 'line-icon'); svg.setAttribute('aria-hidden', 'true'); const path = document.createElementNS(svg.namespaceURI, 'path'); path.setAttribute('d', 'M3 7V5h6l2 2h10v12H3Z'); svg.append(path); return svg; }
const sizeLabel = value => value < 1024 ? `${value} B` : value < 1024 * 1024 ? `${(value / 1024).toFixed(1)} KB` : `${(value / 1048576).toFixed(1)} MB`;
export class WorkspaceUI {
  constructor(hooks) {
    this.hooks = hooks; this.api = new WorkspaceClient(); this.available = false; this.settings = null;
    this.projectState = null; this.session = null; this.generation = 0; this.requestGeneration = 0;
    this.readAbort = new AbortController(); this.directory = ''; this.file = null; this.fileVersion = null; this.directoryVersion = null;
    this.media = matchMedia('(max-width: 1180px)');
    try { const saved = localStorage.getItem('mona.web.files.open'); this.open = saved == null ? !this.media.matches : saved === '1' && !this.media.matches; } catch { this.open = !this.media.matches; }
    this.settingsVisible = false;
    $('#files-toggle').addEventListener('click', () => this.setOpen(!this.open, true));
    $('#files-close').addEventListener('click', () => this.setOpen(false, true));
    $('#files-scrim').addEventListener('click', () => this.setOpen(false, true));
    $('#files-refresh').addEventListener('click', () => { void this.refresh(); });
    $('#files-parent').addEventListener('click', () => { void this.list(this.directory.split('/').slice(0, -1).join('/')); });
    $('#files-more').addEventListener('click', () => { void this.list(this.directory, this.nextDirectory); });
    $('#file-more').addEventListener('click', () => { void this.preview(this.file, this.nextFile); });
    $('#file-back').addEventListener('click', () => this.closePreview());
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
    this.media.addEventListener('change', () => { if (this.media.matches) this.setOpen(false); else this.layout(); });
    document.addEventListener('keydown', event => {
      if (!this.open || !this.available || this.settingsVisible || document.querySelector('dialog[open]')) return;
      if (event.key === 'Escape' && (this.media.matches || $('#files-panel').contains(document.activeElement))) { event.preventDefault(); this.setOpen(false, true); }
      if (event.key === 'Tab' && this.media.matches) {
        const items = [...$('#files-panel').querySelectorAll('button:not(:disabled),a[href],[tabindex="0"]')].filter(e => e.getClientRects().length);
        if (!items.length) return;
        const target = event.shiftKey ? items.at(-1) : items[0];
        if (!$('#files-panel').contains(document.activeElement) || (!event.shiftKey && document.activeElement === items.at(-1)) || (event.shiftKey && document.activeElement === items[0])) { event.preventDefault(); target.focus(); }
      }
    });
    this.layout();
  }
  clear() {
    this.generation++; this.api.clear(); this.cancelReads(); this.available = false; this.settings = null; this.projectState = null;
    this.session = null; this.lastSessionStamp = ''; $('#project-list').replaceChildren(); $('#projects-region').hidden = true;
    this.resetFiles(); this.renderSettings(); this.layout();
  }
  async configure(base, token) {
    this.api.configure(base, token); const generation = ++this.generation;
    try {
      const settings = await this.api.settings(); if (generation !== this.generation) return;
      this.applySettings(settings); this.available = true; this.layout();
      if (settings.projects_enabled) await this.refreshProjects();
    } catch (e) {
      if (generation !== this.generation || e.name === 'AbortError') return;
      this.available = false; this.settings = null; this.renderSettings(); this.layout();
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
      const remove = button('×', 'icon-button project-remove'); remove.setAttribute('aria-label', `移除项目“${project.name}”`); remove.dataset.tooltip = '移除项目登记';
      remove.addEventListener('click', async () => {
        if (!confirm(`移除项目“${project.name}”？文件和历史会话都将保留。`)) return;
        remove.disabled = true;
        try { this.projectState = await this.api.removeProject(project.id, this.projectState.revision); this.renderProjects(); await this.hooks.projectsChanged(); }
        catch (e) { if (e.name !== 'AbortError') $('#projects-notice').textContent = e.message; }
        finally { remove.disabled = false; }
      });
      row.append(open, remove); list.append(row);
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
    if (changed) { this.cancelReads(); this.resetFiles(); }
    this.renderProjects(); this.layout();
    if (this.open && this.available && this.session) void this.refresh();
  }
  cancelReads() { this.requestGeneration++; this.readAbort.abort(); this.readAbort = new AbortController(); }
  resetFiles() {
    this.directory = ''; this.directoryVersion = null; this.nextDirectory = null; this.file = null;
    $('#workspace-path').textContent = ''; $('#files-current').textContent = '/'; $('#files-tree').replaceChildren(); $('#files-more').hidden = true;
    $('#files-notice').textContent = '选择一个已保存的会话后查看文件。'; $('#files-parent').disabled = true; this.closePreview();
  }
  closePreview() { this.fileRequest = (this.fileRequest || 0) + 1; this.file = null; this.fileVersion = null; this.nextFile = 0; $('#file-preview').hidden = true; $('#file-content').replaceChildren(); $('#file-more').hidden = true; }
  settingsVisibility(value) { this.settingsVisible = value; this.layout(); }
  layout() {
    const visible = this.available && this.open && !this.settingsVisible;
    $('#files-toggle').hidden = !this.available; $('#files-toggle').setAttribute('aria-expanded', String(visible));
    $('#files-panel').hidden = !visible; $('#files-scrim').hidden = !visible || !this.media.matches;
    $('.shell').classList.toggle('files-open', visible);
    $('#files-refresh').disabled = !this.session;
    if (this.media.matches && visible) { $('#files-panel').setAttribute('role', 'dialog'); $('#files-panel').setAttribute('aria-modal', 'true'); }
    else { $('#files-panel').setAttribute('role', 'complementary'); $('#files-panel').removeAttribute('aria-modal'); }
  }
  setOpen(value, focus = false) {
    this.open = value;
    if (!value) this.cancelReads();
    try { localStorage.setItem('mona.web.files.open', value ? '1' : '0'); } catch {}
    this.layout(); if (value && this.session) void this.refresh();
    if (focus) (value ? $('#files-close') : $('#files-toggle')).focus();
  }
  async refresh() {
    this.cancelReads(); const generation = this.requestGeneration, session = this.session; if (!session || !this.available) return;
    const selectedFile = this.file; $('#files-notice').textContent = '正在读取…';
    try {
      const info = await this.api.info(session.id, this.readAbort.signal); if (generation !== this.requestGeneration) return;
      $('#workspace-path').textContent = info.root; $('#workspace-path').dataset.tooltip = info.root;
      if (!info.available) throw new Error(info.message || '工作目录不可用。');
      await this.list(this.directory);
      if (selectedFile && generation === this.requestGeneration) await this.preview(selectedFile);
    } catch (e) { if (e.name !== 'AbortError' && generation === this.requestGeneration) $('#files-notice').textContent = e.message; }
  }
  async list(path, offset = 0) {
    if (!this.session) return;
    const generation = this.requestGeneration, session = this.session.id, request = (this.listRequest || 0) + 1; this.listRequest = request;
    $('#files-more').disabled = true;
    try {
      const page = await this.api.list(session, path, { offset, revision: offset ? this.directoryVersion : undefined, signal: this.readAbort.signal });
      if (generation !== this.requestGeneration || request !== this.listRequest) return;
      if (!offset) $('#files-tree').replaceChildren();
      this.directory = path; this.directoryVersion = page.revision; this.nextDirectory = page.next_offset;
      $('#files-current').textContent = path || '/'; $('#files-parent').disabled = !path;
      for (const entry of page.entries) {
        const item = button('', 'file-entry'); item.dataset.path = entry.path;
        const icon = entry.kind === 'directory' ? folderIcon() : node('span', 'file-type-icon', entry.kind === 'link' ? '↗' : '·');
        item.append(icon, node('span', 'file-entry-name', entry.name), node('small', '', entry.kind === 'file' ? sizeLabel(entry.bytes) : ''));
        item.disabled = !['directory', 'file'].includes(entry.kind);
        item.dataset.tooltip = entry.kind === 'link' ? '链接不可通过文件栏打开' : entry.name;
        item.addEventListener('click', () => { if (entry.kind === 'directory') { this.closePreview(); void this.list(entry.path); } else void this.preview(entry.path); });
        $('#files-tree').append(item);
      }
      $('#files-more').hidden = page.next_offset == null; $('#files-notice').textContent = !offset && !page.entries.length ? '此目录暂无文件。' : '';
    } catch (e) { if (e.name !== 'AbortError' && generation === this.requestGeneration && request === this.listRequest) $('#files-notice').textContent = e.message; }
    finally { if (request === this.listRequest) $('#files-more').disabled = false; }
  }
  async preview(path, offset = 0) {
    if (!this.session || !path) return;
    const generation = this.requestGeneration, request = (this.fileRequest || 0) + 1; this.fileRequest = request;
    const body = $('#file-content'); $('#file-preview').hidden = false; $('#file-title').textContent = path.split('/').at(-1);
    if (!offset) { body.replaceChildren(); this.fileVersion = null; this.file = path; }
    $('#file-meta').textContent = '正在读取…'; $('#file-more').disabled = true;
    try {
      const page = await this.api.read(this.session.id, path, { offset, revision: offset ? this.fileVersion : undefined, signal: this.readAbort.signal });
      if (generation !== this.requestGeneration || request !== this.fileRequest || path !== this.file) return;
      this.fileVersion = page.revision; this.nextFile = page.next_offset;
      if (page.kind === 'text') {
        let pre = body.querySelector('pre'); if (!pre) { pre = node('pre', 'workspace-file-text'); pre.tabIndex = 0; pre.setAttribute('aria-label', '文件内容'); body.append(pre); }
        pre.append(document.createTextNode(page.text));
      } else if (page.kind === 'image') {
        const img = document.createElement('img'); img.alt = page.name; img.decoding = 'async'; img.src = `data:${page.media_type};base64,${page.base64}`;
        img.addEventListener('error', () => { if (request === this.fileRequest) $('#file-meta').textContent = '图片无法解码。'; }); body.replaceChildren(img);
      } else body.replaceChildren(node('p', 'workspace-empty', '此二进制文件暂不提供内容预览。'));
      $('#file-meta').textContent = `${sizeLabel(page.bytes)}${page.kind === 'text' && !page.eof ? ` · 已读取 ${sizeLabel(page.next_offset)}` : ''}`;
      $('#file-more').hidden = page.eof || page.kind !== 'text';
    } catch (e) {
      if (e.name !== 'AbortError' && generation === this.requestGeneration && request === this.fileRequest) { $('#file-meta').textContent = e.message; $('#file-more').hidden = true; }
    } finally { if (request === this.fileRequest) $('#file-more').disabled = false; }
  }
}
