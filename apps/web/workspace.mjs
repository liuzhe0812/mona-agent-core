// Authenticated workspace transport. No model calls, raw host filesystem URLs or persisted credentials.
const MAX_RESPONSE = 6 * 1024 * 1024;
const sessionId = value => {
  if (typeof value !== 'string' || !/^[A-Za-z0-9_-]{1,64}$/.test(value)) throw new Error('无效的会话或项目 ID。');
  return encodeURIComponent(value);
};
function query(values) {
  const q = new URLSearchParams();
  for (const [key, value] of Object.entries(values)) if (value != null) q.set(key, String(value));
  return q.size ? `?${q}` : '';
}
export function relativePath(value) {
  if (typeof value !== 'string' || value.length > 4096 || value.startsWith('/') || /[\\:\0]/.test(value)
    || value.split('/').includes('..')) throw new Error('只允许工作区内的相对路径。');
  return value;
}
function validatePageRequest(offset, limit, maxLimit, minLimit = 1) {
  if (!Number.isSafeInteger(offset) || offset < 0 || !Number.isSafeInteger(limit)
    || limit < minLimit || limit > maxLimit) throw new Error('工作区分页参数无效。');
}
export class WorkspaceClient {
  #base = ''; #token = ''; #generation = 0; #requests = new Set();
  configure(base, token) { this.clear(); this.#base = String(base).replace(/\/+$/, ''); this.#token = String(token || ''); }
  clear() { this.#generation++; for (const controller of this.#requests) controller.abort(); this.#requests.clear(); this.#base = ''; this.#token = ''; }
  get configured() { return Boolean(this.#base && this.#token); }
  async #request(path, { method = 'GET', body, signal, binary = false } = {}) {
    const responseLimit = binary ? 16 * 1024 * 1024 : MAX_RESPONSE;
    if (!this.configured) throw new Error('请先连接工作区宿主。');
    const generation = this.#generation, controller = new AbortController();
    this.#requests.add(controller);
    const abort = () => controller.abort(signal.reason);
    signal?.addEventListener('abort', abort, { once: true });
    if (signal?.aborted) abort();
    const timer = setTimeout(() => controller.abort(new DOMException('工作区请求超时，请刷新确认。', 'TimeoutError')), 20000);
    try {
      const response = await fetch(this.#base + path, {
        method, headers: { Authorization: `Bearer ${this.#token}`, ...(body == null ? {} : { 'Content-Type': 'application/json' }) },
        body: body == null ? undefined : JSON.stringify(body), signal: controller.signal,
        credentials: 'omit', cache: 'no-store', redirect: 'error',
      });
      if (Number(response.headers.get('content-length')) > responseLimit) { await response.body?.cancel(); throw new Error('文件响应超过预览大小限制。'); }
      if (response.status === 204) {
        if (generation !== this.#generation || controller.signal.aborted) throw new DOMException('工作区请求已失效。', 'AbortError');
        return null;
      }
      const chunks = []; let size = 0;
      if (response.body) {
        const reader = response.body.getReader();
        try {
          while (true) { const { done, value } = await reader.read(); if (done) break;
            size += value.byteLength; if (size > responseLimit) throw new Error('文件响应超过预览大小限制。'); chunks.push(value); }
        } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
      }
      if (generation !== this.#generation || controller.signal.aborted) throw new DOMException('工作区请求已失效。', 'AbortError');
      const bytes = new Uint8Array(size); let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
      if (binary && response.ok) return bytes;
      let value;
      try { value = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes)); }
      catch { if (response.ok) throw new Error('工作区响应格式无效。'); }
      if (!response.ok) { const error = new Error(value?.message || `工作区请求失败：HTTP ${response.status}`); error.status = response.status; error.code = value?.code; throw error; }
      if (!value || typeof value !== 'object') throw new Error('工作区响应格式无效。');
      return value;
    } finally { clearTimeout(timer); signal?.removeEventListener('abort', abort); this.#requests.delete(controller); }
  }
  workbenchCapabilities() { return this.#request('/api/workbench/capabilities'); }
  targetPath(target) {
    if (!['session', 'project'].includes(target?.kind)) throw new Error('工作区类型无效。');
    return `/api/workbench/${target.kind}/${sessionId(target.id)}`;
  }
  review(target, signal) { return this.#request(this.targetPath(target) + '/review', { signal }); }
  diff(target, path, staged, signal) { relativePath(path); return this.#request(this.targetPath(target) + '/diff' + query({ path, staged: Boolean(staged) }), { signal }); }
  bytes(target, path, revision, signal) { relativePath(path); return this.#request(this.targetPath(target) + '/bytes' + query({ path, revision }), { signal, binary: true }); }
  terminalOpen(target, requestId, rows = 24, cols = 80) { sessionId(requestId); return this.#request(this.targetPath(target) + '/terminals', { method: 'POST', body: { request_id: requestId, rows, cols } }); }
  terminalOutput(target, id, after = 0, signal) { return this.#request(this.targetPath(target) + `/terminals/${sessionId(id)}/output` + query({ after }), { signal }); }
  terminalInput(target, id, sequence, data) { return this.#request(this.targetPath(target) + `/terminals/${sessionId(id)}/input`, { method: 'POST', body: { sequence, data } }); }
  terminalSize(target, id, rows, cols) { return this.#request(this.targetPath(target) + `/terminals/${sessionId(id)}/size`, { method: 'POST', body: { rows, cols } }); }
  terminalClose(target, id) { return this.#request(this.targetPath(target) + `/terminals/${sessionId(id)}`, { method: 'DELETE' }); }
  settings() { return this.#request('/api/workspace-settings'); }
  saveRoot(revision, defaultRoot) { return this.#request('/api/workspace-settings', { method: 'PUT', body: { revision, default_root: defaultRoot } }); }
  projects() { return this.#request('/api/projects'); }
  addProject(value) { sessionId(value.request_id); return this.#request('/api/projects', { method: 'POST', body: value }); }
  removeProject(id, revision) { return this.#request(`/api/projects/${sessionId(id)}/remove`, { method: 'POST', body: { revision } }); }
  async search(id, q, { limit = 100, signal } = {}) {
    if (typeof q !== 'string' || !q.trim() || q.length > 320) throw new Error('文件搜索关键词无效。');
    validatePageRequest(0, limit, 200);
    const value = await this.#request(`/api/sessions/${sessionId(id)}/files/search` + query({ q: q.trim(), limit }), { signal });
    return this.#validateSearch(value);
  }
  async projectList(id, path = '', { offset = 0, limit = 100, revision, signal } = {}) {
    sessionId(id); relativePath(path); validatePageRequest(offset, limit, 200);
    return this.#validateListing(await this.#request(`/api/projects/${sessionId(id)}/files` + query({ path, offset, limit, revision }), { signal }), path, offset, limit, revision);
  }
  async projectRead(id, path, { offset = 0, limit = 65536, revision, signal } = {}) {
    sessionId(id); relativePath(path); validatePageRequest(offset, limit, 65536, 4);
    return this.#validateFile(await this.#request(`/api/projects/${sessionId(id)}/file` + query({ path, offset, limit, revision }), { signal }), path, offset, limit, revision);
  }
  async projectSearch(id, q, { limit = 100, signal } = {}) {
    sessionId(id);
    if (typeof q !== 'string' || !q.trim() || q.length > 320) throw new Error('文件搜索关键词无效。');
    validatePageRequest(0, limit, 200);
    return this.#validateSearch(await this.#request(`/api/projects/${sessionId(id)}/files/search` + query({ q: q.trim(), limit }), { signal }));
  }
  #validateSearch(value) {
    if (!Array.isArray(value.entries) || value.entries.length > 200) throw new Error('文件搜索响应无效。');
    for (const entry of value.entries) {
      if (typeof entry.name !== 'string' || entry.kind !== 'file') throw new Error('文件搜索响应无效。');
      relativePath(entry.path);
    }
    return value;
  }
  #validateListing(value, path, offset, limit, revision) {
    if (value.path !== path || !Array.isArray(value.entries) || value.entries.length > limit || typeof value.revision !== 'string'
      || (revision != null && value.revision !== revision)
      || (value.next_offset != null && (!Number.isSafeInteger(value.next_offset)
        || value.next_offset !== offset + value.entries.length || value.next_offset <= offset))) throw new Error('目录分页响应无效。');
    for (const entry of value.entries) {
      if (typeof entry.name !== 'string' || !['directory', 'file', 'link', 'other'].includes(entry.kind)) throw new Error('文件信息无效。');
      relativePath(entry.path);
    }
    return value;
  }
  #validateFile(value, path, offset, limit, revision) {
    if (value.path !== path || value.offset !== offset || typeof value.revision !== 'string'
      || (revision != null && value.revision !== revision) || !Number.isSafeInteger(value.bytes) || value.bytes < 0
      || !Number.isSafeInteger(value.next_offset) || value.next_offset < offset || value.next_offset > value.bytes
      || typeof value.eof !== 'boolean' || value.eof !== (value.next_offset === value.bytes)
      || (!value.eof && value.next_offset <= offset)) throw new Error('文件分页响应无效。');
    if (value.kind === 'text' && (typeof value.text !== 'string' || value.next_offset - offset > limit
      || new TextEncoder().encode(value.text).length !== value.next_offset - offset)) throw new Error('文本预览响应无效。');
    if (value.kind === 'image' && (typeof value.base64 !== 'string'
      || !['image/png', 'image/jpeg', 'image/gif', 'image/webp'].includes(value.media_type)
      || !/^[A-Za-z0-9+/]+={0,2}$/.test(value.base64))) throw new Error('图片预览响应无效。');
    if (!['text', 'image', 'binary'].includes(value.kind)) throw new Error('不支持的文件预览类型。');
    return value;
  }
  info(id, signal) { return this.#request(`/api/sessions/${sessionId(id)}/workspace`, { signal }); }
  async list(id, path = '', { offset = 0, limit = 100, revision, signal } = {}) {
    relativePath(path); validatePageRequest(offset, limit, 200);
    return this.#validateListing(await this.#request(`/api/sessions/${sessionId(id)}/files` + query({ path, offset, limit, revision }), { signal }), path, offset, limit, revision);
  }
  async read(id, path, { offset = 0, limit = 65536, revision, signal } = {}) {
    relativePath(path); validatePageRequest(offset, limit, 65536, 4);
    return this.#validateFile(await this.#request(`/api/sessions/${sessionId(id)}/file` + query({ path, offset, limit, revision }), { signal }), path, offset, limit, revision);
  }
}
