// Host management only. Credentials and memory bodies never enter browser persistent storage.
const MAX_RESPONSE_BYTES = 256 * 1024;
const bytes = text => new TextEncoder().encode(text).length;
async function readResponse(response, signal) {
  if (Number(response.headers.get('content-length')) > MAX_RESPONSE_BYTES) {
    await response.body?.cancel();
    throw new Error('记忆响应超过读取上限，未加载。');
  }
  const reader = response.body?.getReader();
  if (!reader) throw new Error('记忆响应为空，请刷新确认。');
  const decoder = new TextDecoder('utf-8', { fatal: true });
  const cancel = () => { void reader.cancel().catch(() => {}); };
  let text = '', size = 0;
  signal.addEventListener('abort', cancel, { once: true });
  try {
    signal.throwIfAborted();
    while (true) {
      const { value, done } = await reader.read(); signal.throwIfAborted();
      if (done) break;
      size += value.byteLength;
      if (size > MAX_RESPONSE_BYTES) { await reader.cancel(); throw new Error('记忆响应超过读取上限，未加载。'); }
      text += decoder.decode(value, { stream: true });
    }
    text += decoder.decode();
    return JSON.parse(text);
  } finally { signal.removeEventListener('abort', cancel); reader.releaseLock(); }
}
export class MemoryClient {
  #base = ''; #token = ''; #generation = 0; #pending = new Set();
  configure(base, token) { this.clear(); this.#base = String(base || '').replace(/\/+$/, ''); this.#token = String(token || ''); }
  clear() { this.#generation++; for (const c of this.#pending) c.abort(); this.#pending.clear(); this.#base = ''; this.#token = ''; }
  get configured() { return Boolean(this.#base && this.#token); }
  async #request(path, body) {
    if (!this.configured) throw new Error('当前连接未提供记忆管理。');
    const generation = this.#generation, c = new AbortController(); this.#pending.add(c);
    const timer = setTimeout(() => c.abort(), 20000);
    try {
      const payload = body === undefined ? undefined : JSON.stringify(body);
      if (payload !== undefined && bytes(payload) > 64 * 1024) throw new Error('记忆请求超过大小上限。');
      const response = await fetch(this.#base + path, { method: body === undefined ? 'GET' : 'POST', headers: { Authorization: `Bearer ${this.#token}`, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) }, body: payload, signal: c.signal, credentials: 'omit', redirect: 'error', cache: 'no-store' });
      if (generation !== this.#generation) { await response.body?.cancel(); throw new DOMException('连接已切换。', 'AbortError'); }
      let data;
      try { data = await readResponse(response, c.signal); }
      catch (error) { if (!response.ok) error.status = response.status; throw error; }
      if (generation !== this.#generation) throw new DOMException('连接已切换。', 'AbortError');
      if (!response.ok) { const error = new Error(typeof data?.message === 'string' ? data.message : `记忆请求失败：HTTP ${response.status}`); error.status = response.status; throw error; }
      return data;
    } finally { clearTimeout(timer); this.#pending.delete(c); }
  }
  view(sessionId) { return this.#request('/api/memory' + (sessionId ? `?session_id=${encodeURIComponent(sessionId)}` : '')); }
  update(body) { return this.#request('/api/memory/update', body); }
  search(body) { return this.#request('/api/history/search', body); }
  read(body) { return this.#request('/api/history/read', body); }
}
export function validateMemoryView(value) {
  if (typeof value?.enabled !== 'boolean' || typeof value?.history_enabled !== 'boolean' || !Array.isArray(value.scopes) || value.scopes.length > 4) throw new Error('记忆响应无效，请刷新。');
  for (const s of value.scopes) if (!['personal', 'workspace'].includes(s.name) || !/^[a-f0-9]{64}$/.test(s.revision) || !Array.isArray(s.entries) || s.entries.length > 128 || s.entries.some(e => typeof e.id !== 'string' || typeof e.text !== 'string')) throw new Error('记忆范围或条目响应无效。');
  return value;
}
