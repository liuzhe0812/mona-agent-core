import {BridgeError, frameSequence, isTerminal} from './index.mjs';
import {readSse} from './sse.mjs';

/** Fetch-based SSE supports Authorization; no tokens in URLs or localStorage. */
export class HttpAgentClient {
  #base; #token; #fetch; #maxEventBytes;
  constructor({baseUrl, token, fetch: fetchImpl = globalThis.fetch, maxEventBytes = 128 * 1024 * 1024}) {
    const url = new URL(baseUrl);
    const local = ['localhost','127.0.0.1','[::1]'].includes(url.hostname);
    if ((url.protocol !== 'https:' && !(url.protocol === 'http:' && local)) || url.username || url.password || url.search || url.hash) {
      throw new Error('Use HTTPS, or loopback HTTP, without URL credentials/query/fragment');
    }
    if (typeof token !== 'string' || token.length < 32 || token.length > 512 || !/^[\x21-\x7e]+$/.test(token)) throw new Error('A bearer token is required');
    if (typeof fetchImpl !== 'function') throw new Error('fetch implementation is required');
    if (!Number.isSafeInteger(maxEventBytes) || maxEventBytes <= 0) throw new Error('Positive maxEventBytes required');
    this.#base = baseUrl.replace(/\/$/, ''); this.#token = token; this.#fetch = fetchImpl; this.#maxEventBytes = maxEventBytes;
  }
  async #request(path, {method = 'GET', body, signal, headers = {}} = {}) {
    const response = await this.#fetch(this.#base + path, {method, signal, redirect:'error', credentials:'omit',
      headers: {Authorization:`Bearer ${this.#token}`, ...(body == null ? {} : {'Content-Type':'application/json'}), ...headers},
      body: body == null ? undefined : JSON.stringify(body)});
    if (!response.ok) {
      let error; try { error = await response.json(); } catch { error = {}; }
      throw new BridgeError(error.message || `Agent HTTP ${response.status}`, {code:error.code, status:response.status});
    }
    return response;
  }
  async #json(path, options) { return (await this.#request(path, options)).json(); }
  start(request) { return this.#json('/v1/runs', {method:'POST', body:request}); }
  cancel(runId) { return this.#json(`/v1/runs/${encodeURIComponent(runId)}/cancel`, {method:'POST'}); }
  input(runId, request) { return this.#json(`/v1/runs/${encodeURIComponent(runId)}/input`, {method:'POST', body:request}); }
  snapshot(runId) { return this.#json(`/v1/runs/${encodeURIComponent(runId)}/snapshot`); }
  result(runId) { return this.#json(`/v1/runs/${encodeURIComponent(runId)}/result`); }
  async forget(runId) { await this.#request(`/v1/runs/${encodeURIComponent(runId)}`, {method:'DELETE'}); }
  subscribe(runId, {after, onFrame, signal} = {}) {
    if (typeof onFrame !== 'function') throw new Error('onFrame callback is required');
    if (after != null && (!Number.isSafeInteger(after) || after < 0)) throw new Error('Invalid cursor');
    const abort = new AbortController();
    const externalAbort = () => abort.abort();
    if (signal?.aborted) abort.abort();
    else signal?.addEventListener('abort', externalAbort, {once:true});
    let cursor = after, terminal = false;
    const closed = (async () => {
      try {
        const response = await this.#request(`/v1/runs/${encodeURIComponent(runId)}/events`, {signal:abort.signal,
          headers:{Accept:'text/event-stream', ...(after == null ? {} : {'Last-Event-ID':String(after)})}});
        if (!response.headers.get('content-type')?.startsWith('text/event-stream')) throw new BridgeError('Expected SSE response', {code:'protocol'});
        for await (const message of readSse(response.body, this.#maxEventBytes)) {
          if (message.event !== 'agent') continue;
          const frame = JSON.parse(message.data);
          await onFrame(frame);
          cursor = frameSequence(frame) ?? cursor;
          if (frame.kind === 'fault') throw new BridgeError(frame.error.message, {code:frame.error.code, cursor});
          terminal ||= isTerminal(frame);
        }
        // Also handles reconnecting at the terminal cursor (zero remaining SSE events).
        if (!terminal && !abort.signal.aborted) {
          const snapshot = await this.snapshot(runId);
          if (snapshot.outcome == null) throw new BridgeError('Stream disconnected before completion; reconnect with the last cursor', {code:'disconnected', cursor});
          await onFrame({kind:'snapshot', reason:'source_resync', snapshot});
        }
      } catch (error) {
        if (!abort.signal.aborted) {
          if (error instanceof BridgeError) { error.cursor ??= cursor; throw error; }
          throw new BridgeError(error.message || 'Stream failed', {cursor});
        }
      } finally { signal?.removeEventListener('abort', externalAbort); }
    })();
    return {closed, close: () => abort.abort()}; // detaches only, never cancels the task
  }
}
