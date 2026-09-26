const id = value => typeof value === 'string' && /^[a-z][a-z0-9_.-]{0,95}$/.test(value);
export function validateManifest(value) {
  if (value?.version !== 1 || !Array.isArray(value.modules) || value.modules.length > 32
      || value.modules.some(v => !id(v)) || new Set(value.modules).size !== value.modules.length
      || !value.capabilities || typeof value.capabilities !== 'object' || Array.isArray(value.capabilities)
      || Object.keys(value.capabilities).length > 128) throw new Error('UI 装配清单无效或版本不匹配。');
  const capabilities = Object.create(null);
  for (const [key, entry] of Object.entries(value.capabilities)) {
    if (!id(key) || !entry || !['compiled', 'enabled', 'active', 'configurable', 'restart_required'].every(k => typeof entry[k] === 'boolean')
        || (entry.active && !entry.compiled)) throw new Error('UI 能力状态无效。');
    capabilities[key] = Object.freeze({ ...entry });
  }
  return Object.freeze({ version: 1, modules: Object.freeze([...value.modules]), capabilities: Object.freeze(capabilities) });
}
export class ProductClient {
  constructor() { this.epoch = 0; this.controllers = new Set(); this.base = ''; this.bearer = ''; }
  configure(base, bearer) { this.clear(); this.base = String(base || '').replace(/\/+$/, ''); this.bearer = String(bearer || ''); }
  clear() { this.epoch++; for (const c of this.controllers) c.abort(); this.controllers.clear(); this.base = ''; this.bearer = ''; }
  async request(path, body, { signal, maxBytes = 128 * 1024, maxRequestBytes = 16 * 1024 } = {}) {
    if (!this.base || !this.bearer || !path.startsWith('/api/') || path.startsWith('//')) throw new Error('产品接口尚未连接。');
    if (!Number.isSafeInteger(maxRequestBytes) || maxRequestBytes < 1 || maxRequestBytes > 64 * 1024) throw new Error('产品请求上限无效。');
    const epoch = this.epoch, controller = new AbortController(); this.controllers.add(controller);
    const abort = () => controller.abort(signal?.reason); signal?.addEventListener('abort', abort, { once: true }); if (signal?.aborted) abort();
    const timer = setTimeout(() => controller.abort(new DOMException('产品请求超时，请刷新确认状态。', 'TimeoutError')), 20000);
    try {
      const encoded = body == null ? undefined : JSON.stringify(body);
      if (encoded && new TextEncoder().encode(encoded).length > maxRequestBytes) throw new Error('产品请求超过大小限制。');
      const response = await fetch(this.base + path, { method: body == null ? 'GET' : 'POST',
        headers: { Authorization: `Bearer ${this.bearer}`, ...(encoded ? { 'Content-Type': 'application/json' } : {}) },
        credentials: 'omit', redirect: 'error', cache: 'no-store', signal: controller.signal, body: encoded });
      const reader = response.body?.getReader(); let length = 0; const chunks = [];
      if (reader) { try { for (;;) { const { done, value } = await reader.read(); if (done) break; length += value.length;
        if (length > maxBytes) throw new Error('产品响应超过大小限制。'); chunks.push(value); }
      } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); } }
      if (epoch !== this.epoch || controller.signal.aborted) throw new DOMException('连接或请求已经失效。', 'AbortError');
      if (!response.ok && !length) { const error = new Error(`产品接口不可用：HTTP ${response.status}`); error.status = response.status; throw error; }
      const bytes = new Uint8Array(length); let offset = 0; for (const c of chunks) { bytes.set(c, offset); offset += c.length; }
      let value; try { value = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes)); } catch { throw new Error('产品响应格式无效。'); }
      if (!response.ok) { const error = new Error(typeof value.message === 'string' ? value.message.slice(0, 1000) : `产品操作失败：HTTP ${response.status}`); error.status = response.status; throw error; }
      return value;
    } finally { clearTimeout(timer); this.controllers.delete(controller); signal?.removeEventListener('abort', abort); }
  }
  async manifest() { return validateManifest(await this.request('/api/ui', null, { maxBytes: 32 * 1024 })); }
}
