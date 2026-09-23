function trimBaseUrl(value) { return String(value || '').trim().replace(/\/+$/, ''); }

export class SpillClient {
  #baseUrl = '';
  #bearer = '';

  configure(baseUrl, bearer) {
    this.#baseUrl = trimBaseUrl(baseUrl);
    this.#bearer = String(bearer || '');
  }

  clear() { this.#baseUrl = ''; this.#bearer = ''; }
  get configured() { return Boolean(this.#baseUrl && this.#bearer); }
  supports(artifactUri) { return /^spill:sp_[A-Za-z0-9_]{1,93}$/.test(String(artifactUri || '')); }

  async readPage(runId, artifactUri, offset = 0, limit = 16 * 1024) {
    if (!this.configured) throw new Error('完整结果读取仅支持已连接的 HTTP Runtime。');
    const match = /^spill:(sp_[A-Za-z0-9_]{1,93})$/.exec(String(artifactUri || ''));
    if (!match) throw new Error('归档结果引用无效。');
    if (!String(runId || '').trim()) throw new Error('任务 ID 无效。');
    const url = `${this.#baseUrl}/api/spill/${encodeURIComponent(runId)}/${encodeURIComponent(match[1])}?offset=${offset}&limit=${limit}`;
    const response = await fetch(url, {
      headers: { Authorization: `Bearer ${this.#bearer}` },
      credentials: 'omit', redirect: 'error', cache: 'no-store', signal: AbortSignal.timeout(20000),
    });
    let payload = null;
    try { payload = await response.json(); } catch {}
    if (!response.ok) throw new Error(payload?.message || `读取完整结果失败：HTTP ${response.status}`);
    if (!payload || typeof payload.text !== 'string' || !Number.isInteger(payload.next_offset) || typeof payload.eof !== 'boolean') {
      throw new Error('完整结果响应格式无效。');
    }
    return payload;
  }
}

