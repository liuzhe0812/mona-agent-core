// Host conversation API; never persists credentials or message content in the browser.
const ROOT = '/api/sessions';
const MAX_BYTES = 16 * 1024 * 1024;
function id(value) {
  if (typeof value !== 'string' || !/^[A-Za-z0-9_-]{1,64}$/.test(value)) throw new Error('无效的会话或请求 ID。');
  return encodeURIComponent(value);
}
function page(values) {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(values)) if (value != null) params.set(key, String(value));
  return params.size ? `?${params}` : '';
}
export class SessionsClient {
  #base = ''; #token = ''; #generation = 0; #requests = new Set();
  configure(base, token) { this.clear(); this.#base = String(base).replace(/\/+$/, ''); this.#token = token; }
  clear() {
    this.#generation++;
    for (const controller of this.#requests) controller.abort();
    this.#requests.clear(); this.#base = ''; this.#token = '';
  }
  get configured() { return Boolean(this.#base && this.#token); }
  async #request(path, body) {
    if (!this.configured) throw new Error('请先连接会话宿主。');
    const generation = this.#generation;
    const controller = new AbortController(); this.#requests.add(controller);
    const timeout = setTimeout(() => controller.abort(new DOMException('会话请求超时，请刷新确认是否已保存。', 'TimeoutError')), 20000);
    try {
      const response = await fetch(`${this.#base}${path}`, {
        method: body === undefined ? 'GET' : 'POST',
        headers: { Authorization: `Bearer ${this.#token}`, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
        body: body === undefined ? undefined : JSON.stringify(body),
        credentials: 'omit', redirect: 'error', cache: 'no-store', signal: controller.signal,
      });
      if (Number(response.headers.get('content-length')) > MAX_BYTES) throw new Error('会话响应过大，请使用分页。');
      const chunks = []; let size = 0;
      if (response.body) {
        const reader = response.body.getReader();
        try {
          while (true) {
            const { value, done } = await reader.read(); if (done) break;
            size += value.byteLength;
            if (size > MAX_BYTES) throw new Error('会话响应过大，请使用分页。');
            chunks.push(value);
          }
        } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
      }
      if (generation !== this.#generation) throw new DOMException('会话连接已切换。', 'AbortError');
      const bytes = new Uint8Array(size); let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
      let payload = null;
      if (size) {
        try { payload = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes)); }
        catch { if (response.ok) throw new Error('宿主返回了无效的会话数据。'); }
      }
      if (!response.ok) {
        const error = new Error(payload?.message || `会话请求失败：HTTP ${response.status}`);
        error.status = response.status; throw error;
      }
      return payload;
    } finally { clearTimeout(timeout); this.#requests.delete(controller); }
  }
  list({ offset = 0, limit = 50, q = '', archived = false, project_id } = {}) {
    if (project_id) id(project_id);
    return this.#request(ROOT + page({ offset, limit, q, archived: archived ? 'true' : undefined, project_id }));
  }
  create(requestId, projectId = null) {
    id(requestId); if (projectId != null) id(projectId);
    return this.#request(ROOT, { request_id: requestId, ...(projectId == null ? {} : { project_id: projectId }) });
  }
  get(session, { before, limit = 10 } = {}) { return this.#request(`${ROOT}/${id(session)}` + page({ before, limit })); }
  turn(session, turn, { before, limit = 50 } = {}) { return this.#request(`${ROOT}/${id(session)}/turns/${id(turn)}` + page({ before, limit })); }
  start(session, value) { id(value.request_id); return this.#request(`${ROOT}/${id(session)}/turns`, value); }
  rename(session, revision, title) { return this.#request(`${ROOT}/${id(session)}/rename`, { revision, title }); }
  pin(session, revision, value) { return this.#request(`${ROOT}/${id(session)}/pin`, { revision, value }); }
  archive(session, revision, value) { return this.#request(`${ROOT}/${id(session)}/archive`, { revision, value }); }
  unread(session, revision, value) { return this.#request(`${ROOT}/${id(session)}/unread`, { revision, value }); }
  delete(session, revision) { return this.#request(`${ROOT}/${id(session)}/delete`, { revision }); }
}
