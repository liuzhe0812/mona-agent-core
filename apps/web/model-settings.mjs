const SETTINGS_PATH = '/api/model-settings';

function trimBaseUrl(value) {
  return String(value || '').trim().replace(/\/+$/, '');
}

function errorFromResponse(response, payload) {
  const message = payload && typeof payload.message === 'string' && payload.message.trim()
    ? payload.message.trim()
    : `模型设置请求失败：HTTP ${response.status}`;
  const error = new Error(message);
  error.status = response.status;
  error.payload = payload;
  return error;
}

/**
 * HTTP-only management client for the optional model-settings host module.
 * Credentials stay in memory and every request is isolated from AgentClient.
 */
export class ModelSettingsClient {
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
    if (!this.configured) throw new Error('模型设置需要先连接 HTTP Runtime。');
    if (generation !== this.#generation) throw new Error('模型设置连接已切换，请重试。');

    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(new DOMException('模型设置请求超时，请重试。', 'TimeoutError')), 20000);
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
      if (generation !== this.#generation) {
        const stale = new DOMException('模型设置连接已切换，请重试。', 'AbortError');
        throw stale;
      }
      if (!response.ok) throw errorFromResponse(response, payload);
      return payload;
    } finally {
      clearTimeout(timeout);
      this.#controllers.delete(controller);
    }
  }

  get() {
    return this.#request(SETTINGS_PATH);
  }

  saveProvider(value) {
    return this.#request(`${SETTINGS_PATH}/providers`, { method: 'POST', body: value });
  }

  deleteProvider(value) {
    return this.#request(`${SETTINGS_PATH}/delete`, { method: 'POST', body: value });
  }

  setDefault(value) {
    return this.#request(`${SETTINGS_PATH}/default`, { method: 'POST', body: value });
  }

  setVisibility(value) {
    return this.#request(`${SETTINGS_PATH}/visibility`, { method: 'POST', body: value });
  }

  discover(value) {
    return this.#request(`${SETTINGS_PATH}/discover`, { method: 'POST', body: value });
  }
}

export function createProviderId() {
  if (typeof globalThis.crypto?.randomUUID !== 'function') throw new Error('当前浏览器不支持安全的供应商 ID。');
  return globalThis.crypto.randomUUID();
}
