import { element as el, button } from './content-dom.mjs';
import { paneIcon } from './pane-icons.mjs';
import { paneButton } from './pane-controls.mjs';
// A file page belongs to one immutable target and connection, independent of the left task navigation.
export class WorkspaceFiles {
  constructor({ api, target, openFile }) {
    this.api = api.fork(); this.target = target; this.openFile = openFile; this.generation = 0; this.request = 0; this.queryGeneration = 0;
    this.controller = new AbortController(); this.branches = new AbortController(); this.queryController = new AbortController(); this.expanded = new Set(); this.selected = ''; this.directory = ''; this.root = el('section', 'workspace-files');
    const header = el('header', 'workspace-files-head'); this.path = el('p', 'workspace-files-path', target.path || '工作区'); this.path.dataset.tooltip = target.path || '工作区';
    this.refresh = paneButton('刷新文件列表', 'refresh', () => { void this.reload(); });
    this.search = el('input'); this.search.type = 'search'; this.search.maxLength = 256; this.search.placeholder = '搜索工作区文件'; this.search.setAttribute('aria-label', '搜索工作区文件');
    this.searchRow = el('div', 'files-search-row'); this.searchRow.hidden = true; this.searchRow.append(this.search);
    this.searchButton = paneButton('搜索文件', 'search', () => this.showSearch(this.searchRow.hidden));
    this.collapseButton = paneButton('折叠所有文件夹', 'collapseAll', () => { this.expanded.clear(); for (const branch of this.browse.querySelectorAll('details[open]')) branch.open = false; this.focusRow(this.browse.querySelector('.file-entry')); });
    header.append(this.path, this.searchButton, this.collapseButton, this.refresh);
    this.tree = el('div', 'files-tree'); this.tree.setAttribute('aria-label', '目录文件'); this.browse = el('div', 'files-browse'); this.results = el('div', 'files-search-results'); this.results.hidden = true;
    for (const [body, label] of [[this.browse, '工作区目录'], [this.results, '文件搜索结果']]) { body.setAttribute('role', 'tree'); body.setAttribute('aria-label', label); }
    this.tree.append(this.browse, this.results);
    this.notice = el('p', 'files-notice'); this.notice.setAttribute('role', 'status'); this.more = button('加载更多文件', () => { void this.load(this.directory, this.next); }); this.more.hidden = true;
    this.root.append(header, this.searchRow, this.tree, this.more, this.notice);
    this.search.addEventListener('input', () => { clearTimeout(this.timer); this.queryController.abort(); this.queryGeneration++; this.timer = setTimeout(() => { void this.searchFiles(); }, 180); });
    this.search.addEventListener('keydown', event => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); this.showSearch(false); } });
    this.root.addEventListener('keydown', event => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'f') { event.preventDefault(); event.stopPropagation(); this.showSearch(true); }
    });
    this.tree.addEventListener('keydown', event => this.navigate(event));
  }
  close() { this.closed = true; clearTimeout(this.timer); this.request++; this.generation++; this.queryGeneration++; this.controller.abort(); this.branches.abort(); this.queryController.abort(); this.api.clear(); }
  list(path, options) { return this.target.kind === 'project' ? this.api.projectList(this.target.id, path, options) : this.api.list(this.target.id, path, options); }
  showSearch(show) {
    this.searchRow.hidden = !show; this.searchButton.setAttribute('aria-expanded', String(show));
    if (show) this.search.focus();
    else { clearTimeout(this.timer); this.search.value = ''; void this.searchFiles(); this.searchButton.focus(); }
  }
  reload() { return this.search.value.trim() ? this.searchFiles() : this.load(this.directory); }
  async searchFiles() {
    if (this.closed) return;
    this.queryController.abort(); this.queryController = new AbortController(); const generation = ++this.queryGeneration, query = this.search.value.trim();
    this.browse.hidden = Boolean(query); this.results.hidden = !query; this.more.hidden = Boolean(query) || this.next == null; this.notice.textContent = '';
    if (!query) { if (!this.loaded) await this.load(this.directory); return; }
    this.results.replaceChildren(); this.notice.textContent = '正在搜索…';
    try {
      const method = this.target.kind === 'project' ? 'projectSearch' : 'search';
      const result = await this.api[method](this.target.id, query, { signal: this.queryController.signal });
      if (generation !== this.queryGeneration || this.closed) return;
      this.renderEntries(result.entries, this.results, true);
      this.notice.textContent = result.truncated ? '结果较多，请缩小搜索范围。' : result.entries.length ? '' : '没有匹配的文件。';
    } catch (error) { if (generation === this.queryGeneration && error.name !== 'AbortError' && !this.closed) this.showError(error, () => this.searchFiles()); }
  }
  showError(error, retry) { this.notice.textContent = error.message; this.notice.append(button('重试', () => { void retry(); })); }
  async load(path = '', offset = 0) {
    if (this.closed || (offset && this.loading)) return;
    this.controller.abort(); this.controller = new AbortController(); const request = ++this.request;
    this.loading = true; this.more.disabled = true; this.root.setAttribute('aria-busy', 'true'); this.notice.textContent = '正在读取…';
    const scrollTop = this.tree.scrollTop;
    try {
      const page = await this.list(path, { offset, revision: offset ? this.revision : undefined, signal: this.controller.signal });
      if (request !== this.request || this.closed) return;
      if (!offset) { this.generation++; this.branches.abort(); this.branches = new AbortController(); this.browse.replaceChildren(); }
      this.directory = path; this.revision = page.revision; this.next = page.next_offset;
      this.renderEntries(page.entries, this.browse); this.loaded = true;
      if (!this.search.value.trim()) { this.more.hidden = page.next_offset == null; this.notice.textContent = this.browse.childElementCount ? '' : '文件夹为空。'; this.tree.scrollTop = scrollTop; }
    } catch (error) { if (request === this.request && error.name !== 'AbortError' && !this.closed && !this.search.value.trim()) this.showError(error, () => this.load(path, offset)); }
    finally { if (request === this.request) { this.loading = false; this.more.disabled = false; this.root.setAttribute('aria-busy', 'false'); } }
  }
  focusRow(row, moveFocus = true) {
    if (!row) return;
    for (const peer of this.tree.querySelectorAll('.file-entry')) peer.tabIndex = peer === row ? 0 : -1;
    if (moveFocus) row.focus({ preventScroll: true }); row.scrollIntoView({ block: 'nearest' });
  }
  navigate(event) {
    const row = event.target.closest('.file-entry'); if (!row || event.altKey || event.ctrlKey || event.metaKey) return;
    const rows = [...this.tree.querySelectorAll('.file-entry')].filter(n => n.getClientRects().length && !n.disabled), index = rows.indexOf(row), branch = row.parentElement.matches('details') ? row.parentElement : null;
    let target;
    if (event.key === 'ArrowDown') target = rows[Math.min(index + 1, rows.length - 1)];
    if (event.key === 'ArrowUp') target = rows[Math.max(index - 1, 0)];
    if (event.key === 'Home') target = rows[0]; if (event.key === 'End') target = rows.at(-1);
    if (event.key === 'ArrowRight' && branch) { if (!branch.open) branch.open = true; else target = branch.querySelector('.file-branch .file-entry'); }
    if (event.key === 'ArrowLeft') { if (branch?.open) branch.open = false; else target = row.closest('.file-branch')?.parentElement.querySelector(':scope > summary'); }
    if (target || ['ArrowRight', 'ArrowLeft'].includes(event.key)) { event.preventDefault(); event.stopPropagation(); if (target) this.focusRow(target); }
  }
  renderEntries(entries, parent, searching = false) {
    for (const entry of entries) {
      if (entry.kind === 'directory') { parent.append(this.branch(entry)); continue; }
      const item = button('', () => { this.selected = entry.path; for (const peer of this.tree.querySelectorAll('[aria-selected]')) peer.setAttribute('aria-selected', String(peer === item)); this.focusRow(item, false); Promise.resolve().then(() => this.openFile(entry.path)).catch(error => { if (!this.closed) this.notice.textContent = error.message; }); }, 'file-entry');
      item.dataset.path = entry.path; item.dataset.tooltip = entry.path; item.setAttribute('role', 'treeitem'); item.setAttribute('aria-selected', String(this.selected === entry.path)); item.tabIndex = parent.childElementCount ? -1 : 0;
      item.append(paneIcon('file'), el('span', 'file-entry-name', searching ? entry.path : entry.name));
      item.disabled = entry.kind !== 'file'; if (item.disabled) item.dataset.tooltip = '不允许打开链接或特殊文件。'; parent.append(item);
    }
    if (parent === this.browse || parent === this.results) { const first = parent.querySelector('.file-entry:not(:disabled)'); if (first && !parent.querySelector('.file-entry[tabindex="0"]')) first.tabIndex = 0; }
  }
  branch(entry) {
    const root = el('details', 'file-directory'), summary = el('summary', 'file-entry'), children = el('div', 'file-branch'); summary.dataset.path = entry.path; summary.dataset.tooltip = entry.path;
    summary.setAttribute('role', 'treeitem'); summary.setAttribute('aria-expanded', 'false'); summary.tabIndex = -1; children.setAttribute('role', 'group');
    summary.append(el('span', 'file-chevron', '›'), paneIcon('folder'), el('span', 'file-entry-name', entry.name)); root.append(summary, children);
    let loaded = false, pending = false, revision; const generation = this.generation;
    const load = async (offset = 0) => {
      if (pending || generation !== this.generation || this.closed) return; pending = true; const note = el('p', 'workspace-empty', '正在读取…'); children.append(note);
      try {
        const page = await this.list(entry.path, { offset, revision: offset ? revision : undefined, signal: this.branches.signal });
        if (generation !== this.generation || !root.isConnected || this.closed) return;
        note.remove(); if (!offset) children.replaceChildren(); revision = page.revision; this.renderEntries(page.entries, children); loaded = true;
        if (page.next_offset != null) { const more = button('加载更多', () => { more.remove(); void load(page.next_offset); }); children.append(more); }
        else if (!children.childElementCount) children.append(el('p', 'workspace-empty', '空文件夹'));
      } catch (error) { if (generation === this.generation && error.name !== 'AbortError') { note.textContent = error.message; note.append(button('重试', () => { note.remove(); void load(offset); })); } }
      finally { pending = false; }
    };
    root.addEventListener('toggle', () => { if (this.closed || generation !== this.generation) return; summary.setAttribute('aria-expanded', String(root.open)); if (root.open) this.expanded.add(entry.path); else this.expanded.delete(entry.path); if (root.open && !loaded) void load(); });
    root.open = this.expanded.has(entry.path); return root;
  }
}
