// Per-window file-tab metadata only. Never persists credentials, document bodies or terminal commands.
const KEY = 'mona.web.file-tabs';
const validScope = value => /^(?:session|project):[A-Za-z0-9_-]{1,64}$/.test(value || '');
const validPath = value => typeof value === 'string' && value.length > 0 && value.length <= 4096 && !/^[\\/]|[\\:\u0000-\u001f\u007f]/.test(value) && !value.split('/').includes('..');
function layoutFields(record, files) {
  const keys = new Set(files.map(f => 'file:' + f.path)); if (record.filesPage) keys.add('page:files');
  const panes = record.panes || [[...keys], []];
  if (!Array.isArray(panes) || panes.length !== 2 || panes.some(group => !Array.isArray(group)) || panes.flat().length !== keys.size || new Set(panes.flat()).size !== keys.size || panes.flat().some(key => !keys.has(key))) return null;
  const selected = Array.isArray(record.selected) && record.selected.length === 2 ? record.selected.map((key, side) => panes[side].includes(key) ? key : panes[side][0] || null) : [panes[0][0] || null, panes[1][0] || null];
  return { panes, selected, focus: record.focus === 1 && panes[1].length ? 1 : 0, ratio: Number.isFinite(record.ratio) ? Math.max(20, Math.min(80, record.ratio)) : 50, fullscreen: record.fullscreen === true };
}
export class PaneState {
  constructor(storage) { this.storage = storage; this.endpoint = ''; this.records = []; }
  configure(endpoint) {
    this.records = []; this.endpoint = '';
    try {
      const url = new URL(endpoint); if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) return;
      this.endpoint = url.href.replace(/\/$/, ''); const raw = this.storage()?.getItem(KEY); if (!raw || raw.length > 65536) return;
      const value = JSON.parse(raw);
      if (value.version !== 2 || value.endpoint !== this.endpoint || !Array.isArray(value.records) || value.records.length > 16) return;
      this.records = value.records.filter(record => record && validScope(record.scope) && Array.isArray(record.files) && record.files.length <= 48 && record.files.every(file => file && validPath(file.path) && ['source', 'preview'].includes(file.mode) && typeof file.wrap === 'boolean') && layoutFields(record, record.files)).map(record => ({ scope: record.scope, files: record.files.map(file => ({ path: file.path, mode: file.mode, wrap: file.wrap })), activePath: validPath(record.activePath) ? record.activePath : null, expanded: record.expanded === true, filesPage: record.filesPage === true, activeFilesPage: record.activeFilesPage === true, ...layoutFields(record, record.files) }));
    } catch { /* An unavailable or malformed preference store never blocks the UI. */ }
  }
  get(scope) { return this.records.find(record => record.scope === scope); }
  save(scope, files, activePath, layout = {}) {
    if (!this.endpoint || !validScope(scope)) return false;
    const safe = files.filter(file => validPath(file.path)).slice(0, 48).map(file => ({ path: file.path, mode: file.mode === 'source' ? 'source' : 'preview', wrap: Boolean(file.wrap) }));
    const fields = layoutFields(layout, safe); if (!fields) return false;
    const records = [...this.records.filter(record => record.scope !== scope), { scope, files: safe, activePath: validPath(activePath) ? activePath : null, expanded: layout.expanded === true, filesPage: layout.filesPage === true, activeFilesPage: layout.activeFilesPage === true, ...fields }].slice(-16);
    try {
      let raw = JSON.stringify({ version: 2, endpoint: this.endpoint, records });
      while (raw.length > 65536 && records.length > 1) { records.shift(); raw = JSON.stringify({ version: 2, endpoint: this.endpoint, records }); }
      const storage = this.storage(); if (raw.length > 65536 || !storage) return false;
      storage.setItem(KEY, raw); this.records = records; return true;
    } catch { return false; }
  }
}
