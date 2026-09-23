const CAPABILITIES_PATH = '/api/capabilities';

function trimBaseUrl(value) {
  return String(value || '').trim().replace(/\/+$/, '');
}

function abortError(message) {
  if (typeof DOMException === 'function') return new DOMException(message, 'AbortError');
  const error = new Error(message);
  error.name = 'AbortError';
  return error;
}

function errorFromResponse(response, payload) {
  const message = payload && typeof payload.message === 'string' && payload.message.trim()
    ? payload.message.trim()
    : `Agent 设置请求失败：HTTP ${response.status}`;
  const error = new Error(message);
  error.status = response.status;
  error.code = payload && typeof payload.code === 'string' ? payload.code : undefined;
  error.payload = payload;
  return error;
}

/**
 * HTTP-only management client for optional Agent components and tools.
 * The bearer is kept in memory and never sent as a cookie or query parameter.
 */
export class CapabilitiesClient {
  #baseUrl = '';
  #bearer = '';
  #generation = 0;
  #controllers = new Set();

  configure(baseUrl, bearer) {
    this.clear();
    this.#baseUrl = trimBaseUrl(baseUrl);
    this.#bearer = String(bearer || '');
  }

  clear() {
    this.#generation += 1;
    for (const controller of this.#controllers) controller.abort();
    this.#controllers.clear();
    this.#baseUrl = '';
    this.#bearer = '';
  }

  get configured() {
    return Boolean(this.#baseUrl && this.#bearer);
  }

  async #request(path, { method = 'GET', body, generation = this.#generation } = {}) {
    if (!this.configured) throw new Error('Agent 设置需要先连接 HTTP Runtime。');
    if (generation !== this.#generation) throw new Error('Agent 设置连接已切换，请重试。');

    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(new DOMException('Agent 设置请求超时，请重试。', 'TimeoutError')), 20000);
    this.#controllers.add(controller);
    const headers = { Authorization: `Bearer ${this.#bearer}` };
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    try {
      const response = await fetch(`${this.#baseUrl}${path}`, {
        method,
        headers,
        body: body === undefined ? undefined : JSON.stringify(body),
        credentials: 'omit',
        redirect: 'error',
        cache: 'no-store',
        signal: controller.signal,
      });
      let payload = null;
      try { payload = await response.json(); } catch {}
      if (generation !== this.#generation) throw abortError('Agent 设置连接已切换，请重试。');
      if (!response.ok) throw errorFromResponse(response, payload);
      return payload;
    } finally {
      clearTimeout(timeout);
      this.#controllers.delete(controller);
    }
  }

  get() {
    return this.#request(CAPABILITIES_PATH);
  }

  update(id, { enabled, revision } = {}) {
    const capabilityId = String(id || '').trim();
    if (!capabilityId) throw new TypeError('设置项 ID 不能为空。');
    if (typeof enabled !== 'boolean') throw new TypeError('启用状态必须是布尔值。');
    if (!Number.isInteger(revision) || revision < 0) throw new TypeError('设置版本无效。');
    return this.#request(`${CAPABILITIES_PATH}/${encodeURIComponent(capabilityId)}`, {
      method: 'PUT',
      body: { enabled, revision },
    });
  }
}
