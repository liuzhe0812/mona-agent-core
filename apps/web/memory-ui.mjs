import { MemoryClient, validateMemoryView } from './memory.mjs';
const $ = s => document.querySelector(s);
const node = (tag, cls, text = '') => { const n = document.createElement(tag); n.className = cls; n.textContent = text; return n; };
const action = (label, fn, cls = 'text-button') => { const n = node('button', cls, label); n.type = 'button'; n.addEventListener('click', fn); return n; };
const scopeLabel = name => name === 'personal' ? '个人记忆（跨会话）' : '当前工作区记忆';
export class MemoryUI {
  constructor(hooks) {
    this.events = new AbortController(); const on = (target, type, fn, options = {}) => target.addEventListener(type, fn, { ...options, signal: this.events.signal });
    this.$ = hooks.query || $;
    this.hooks = hooks; this.api = new MemoryClient(); this.generation = 0; this.queryGeneration = 0; this.readGeneration = 0; this.state = null;
    on(this.$('#memory-refresh'), 'click', () => { void this.refresh(); });
    on(this.$('#memory-scope'), 'change', () => this.render());
    on(this.$('#memory-add'), 'click', () => this.edit());
    on(this.$('#memory-form'), 'submit', e => { e.preventDefault(); void this.save(); });
    on(this.$('#memory-cancel'), 'click', () => { if (!this.saving) this.$('#memory-dialog').close(); });
    on(this.$('#memory-dialog'), 'cancel', e => { if (this.saving) e.preventDefault(); });
    on(this.$('#memory-dialog'), 'close', () => { this.editor = null; this.$('#memory-text').value = ''; this.$('#memory-add').focus(); });
    on(this.$('#memory-dialog'), 'click', e => { const d = this.$('#memory-dialog'), r = d.getBoundingClientRect(); if (!this.saving && e.target === d && (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom)) d.close(); });
    on(this.$('#history-search-form'), 'submit', e => { e.preventDefault(); void this.search(); });
    on(this.$('#history-more'), 'click', () => { void this.search(this.nextSearch); });
    on(this.$('#history-read-more'), 'click', () => { if (this.readLocator) void this.read(this.readLocator, this.nextRead); });
    on(this.$('#history-read-close'), 'click', () => { this.readGeneration++; this.$('#history-original').hidden = true; });
    this.render();
  }
  dispose() { this.events.abort(); this.clear(); }
  configure(base, token) { this.clear(); this.api.configure(base, token); }
  resetHistory() {
    this.queryGeneration++; this.readGeneration++; this.searching = null;
    this.lastQuery = null; this.nextSearch = null; this.readLocator = null; this.nextRead = null;
    this.$('#history-hits').replaceChildren(); this.$('#history-neighbors').replaceChildren();
    this.$('#history-read-text').textContent = ''; this.$('#history-read-position').textContent = '';
    this.$('#history-original').hidden = true; this.$('#history-more').hidden = true;
    this.$('#history-more').disabled = false; this.$('#history-read-more').disabled = false;
  }
  clear() {
    this.generation++; this.resetHistory(); this.api.clear(); this.state = null; this.sessionId = null; this.editor = null; this.saving = null;
    if (this.$('#memory-dialog').open) this.$('#memory-dialog').close();
    this.$('#memory-text').value = ''; this.$('#memory-save').disabled = false; this.$('#memory-cancel').disabled = false;
    this.$('#memory-refresh').disabled = true; this.$('#memory-notice').textContent = ''; this.render();
  }
  scope() { return this.state?.scopes.find(s => s.name === this.$('#memory-scope').value); }
  async refresh() {
    const generation = ++this.generation; const id = this.hooks.session()?.id;
    this.resetHistory();
    this.$('#memory-refresh').disabled = true;
    try {
      const value = validateMemoryView(await this.api.view(id)); if (generation !== this.generation) return;
      this.state = value; this.sessionId = id; const select = this.$('#memory-scope'), selected = select.value;
      select.replaceChildren(...value.scopes.map(s => { const option = node('option', '', scopeLabel(s.name)); option.value = s.name; return option; }));
      if (value.scopes.some(s => s.name === selected)) select.value = selected;
      this.$('#memory-notice').textContent = value.enabled ? (value.agent_writes ? '已允许 Agent 更新长期记忆；可在 Agent 工具中关闭。' : 'Agent 当前只读；你可以在此维护记忆，或在 Agent 工具中授权更新。') : '当前宿主未启用长期记忆。';
      this.render(); return true;
    } catch (e) { if (generation !== this.generation || e.name === 'AbortError') return; this.state = null; this.render(); this.$('#memory-notice').textContent = [404, 501].includes(e.status) ? '当前宿主未装配记忆管理。' : e.message; }
    finally { if (generation === this.generation) this.$('#memory-refresh').disabled = !this.api.configured; }
  }
  render() {
    const scope = this.scope(); this.$('#memory-add').disabled = !scope || this.saving; this.$('#memory-scope').disabled = !scope || this.saving;
    this.$('#memory-capacity').textContent = scope ? `${scope.text_bytes} / ${scope.limit_bytes} 字节 · ${scope.entries.length} 条` : '';
    const list = this.$('#memory-entries'); list.replaceChildren();
    for (const e of scope?.entries || []) {
      const row = node('article', 'memory-entry'); row.dataset.memoryId = e.id;
      const copy = node('div', 'memory-entry-copy'); copy.append(node('p', 'memory-entry-text', e.text));
      const source = e.origin?.actor === 'agent' ? `Agent · ${e.origin.run_id || ''} / ${e.origin.call_id || ''}` : '用户编辑';
      const updated = Number.isFinite(e.updated_at) ? new Date(e.updated_at).toLocaleString() : '';
      copy.append(node('small', 'memory-entry-source', `${source} · ${updated}`));
      const actions = node('div', 'memory-entry-actions'); actions.append(action('编辑', () => this.edit(e)), action('删除', () => { void this.remove(e); }, 'text-button danger-button'));
      for (const b of actions.children) b.disabled = this.saving;
      row.append(copy, actions); list.append(row);
    }
    if (!list.children.length) list.append(node('p', 'memory-empty', scope ? '尚未保存长期记忆。只留下跨会话仍有用的事实和偏好。' : '连接并启用长期记忆后可在此管理。'));
    this.$('#history-search-button').disabled = !this.state?.history_enabled || this.searching;
    this.$('#history-query').disabled = !this.state?.history_enabled;
    this.$('#history-notice').textContent = this.state?.history_enabled ? '按关键词查找原始消息；不会把全部历史加入当前对话。' : '当前宿主未启用历史检索。';
  }
  edit(entry = null) {
    const scope = this.scope(); if (!scope || this.saving) return;
    this.editor = { scope: scope.name, revision: scope.revision, session_id: this.sessionId, entry, generation: this.generation };
    this.$('#memory-dialog-title').textContent = entry ? '修改记忆' : '新增记忆'; this.$('#memory-editor-scope').textContent = scopeLabel(scope.name);
    this.$('#memory-text').value = entry?.text || ''; this.$('#memory-form-error').textContent = ''; this.$('#memory-save').disabled = false;
    this.$('#memory-dialog').showModal(); this.$('#memory-text').focus();
  }
  async save() {
    if (!this.editor || this.saving) return;
    const editor = this.editor, text = this.$('#memory-text').value.trim();
    if (!text || new TextEncoder().encode(text).length > 2048) { this.$('#memory-form-error').textContent = '请填写完整事实，单条最多 2048 字节。'; return; }
    const operation = {}; this.saving = operation; this.$('#memory-save').disabled = true; this.$('#memory-cancel').disabled = true;
    try {
      const operations = [editor.entry ? { action: 'replace', id: editor.entry.id, text } : { action: 'add', text }];
      await this.api.update({ scope: editor.scope, revision: editor.revision, session_id: editor.session_id, operations });
      if (editor.generation !== this.generation) return;
      this.$('#memory-dialog').close();
      if (await this.refresh() && this.saving === operation) this.$('#memory-notice').textContent = '记忆已保存，后续模型请求将使用更新内容。';
    } catch (e) { if (editor.generation === this.generation && e.name !== 'AbortError') this.$('#memory-form-error').textContent = e.message; }
    finally { if (this.saving === operation) { this.saving = null; this.$('#memory-save').disabled = false; this.$('#memory-cancel').disabled = false; this.render(); } }
  }
  async remove(entry) {
    const scope = this.scope(); if (!scope || this.saving || !confirm('删除这条长期记忆？不会同时删除以前的会话和已经发送的内容。')) return;
    const generation = this.generation, operation = {}; this.saving = operation; this.render();
    try { await this.api.update({ scope: scope.name, revision: scope.revision, session_id: this.sessionId, operations: [{ action: 'remove', id: entry.id }] }); if (generation === this.generation) await this.refresh(); }
    catch (e) { if (generation === this.generation && e.name !== 'AbortError') this.$('#memory-notice').textContent = e.message; }
    finally { if (this.saving === operation) { this.saving = null; this.render(); } }
  }
  async search(offset = 0) {
    if (!this.state?.history_enabled || this.searching) return;
    const query = offset ? this.lastQuery : this.$('#history-query').value.trim(); if (!query) return;
    const generation = this.generation, sequence = ++this.queryGeneration, operation = {}; this.searching = operation; this.$('#history-search-button').disabled = true; this.$('#history-more').disabled = true;
    if (!offset) { this.$('#history-hits').replaceChildren(); this.$('#history-original').hidden = true; this.readGeneration++; }
    try {
      const page = await this.api.search({ query, offset, limit: 8 }); if (generation !== this.generation || sequence !== this.queryGeneration) return;
      if (!Array.isArray(page?.hits) || page.hits.length > 20) throw new Error('历史搜索响应无效。');
      this.lastQuery = query; this.nextSearch = page.next_offset;
      for (const hit of page.hits) {
        const row = node('article', 'memory-history-hit'); row.append(node('strong', '', `${hit.title} · ${hit.role}`), node('p', 'memory-entry-text', hit.excerpt));
        const actions = node('div', 'memory-entry-actions');
        actions.append(action('读取原文', () => { void this.read(hit); }), action('打开会话', () => { void this.hooks.open(hit.session_id).catch(e => { this.$('#history-notice').textContent = e.message; }); }));
        row.append(actions); this.$('#history-hits').append(row);
      }
      this.$('#history-notice').textContent = this.$('#history-hits').children.length ? '已找到原文位置；记忆搜索不会自动执行历史中的操作。' : '没有匹配记录。可以尝试更短的关键词。';
      this.$('#history-more').hidden = page.next_offset == null;
    } catch (e) { if (generation === this.generation && e.name !== 'AbortError') { this.$('#history-notice').textContent = e.message; this.$('#history-more').hidden = true; } }
    finally { if (this.searching === operation) { this.searching = null; if (generation === this.generation && sequence === this.queryGeneration) { this.$('#history-search-button').disabled = !this.state?.history_enabled; this.$('#history-more').disabled = false; } } }
  }
  async read(hit, offset = 0) {
    const generation = this.generation, sequence = ++this.readGeneration; this.$('#history-read-more').disabled = true;
    try {
      const locator = { session_id: hit.session_id, revision: hit.revision, message_index: hit.message_index, message_hash: hit.message_hash };
      const page = await this.api.read({ ...locator, offset, max_bytes: 8192 }); if (generation !== this.generation || sequence !== this.readGeneration) return;
      if (typeof page?.text !== 'string' || typeof page?.message_hash !== 'string') throw new Error('原文响应无效。');
      this.readLocator = { ...locator, revision: page.revision }; this.nextRead = page.next_offset;
      this.$('#history-original').hidden = false; this.$('#history-read-title').textContent = `原文 · ${page.role} · 第 ${page.message_index + 1} 条`;
      // Each page replaces the previous page: the UI does not accumulate unbounded full histories.
      this.$('#history-read-text').textContent = page.text; this.$('#history-read-position').textContent = `${page.offset}–${page.offset + new TextEncoder().encode(page.text).length} / ${page.total_bytes} 字节`;
      this.$('#history-read-more').hidden = page.next_offset == null;
      this.$('#history-neighbors').replaceChildren();
      for (const [name, neighbor] of [['上一条', page.previous_message], ['下一条', page.next_message]]) if (neighbor) this.$('#history-neighbors').append(action(name, () => { void this.read({ ...locator, ...neighbor, revision: page.revision }); }));
    } catch (e) { if (generation === this.generation && e.name !== 'AbortError') this.$('#history-notice').textContent = e.message; }
    finally { if (sequence === this.readGeneration) this.$('#history-read-more').disabled = false; }
  }
}
