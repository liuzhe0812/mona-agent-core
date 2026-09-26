import { RunView, requestId } from '../../packages/client/src/index.mjs';
import { TurnView } from './run-view.mjs';

const node = (tag, className, value = '') => { const n = document.createElement(tag); if (className) n.className = className; n.textContent = value; return n; };
const id = value => {
  if (typeof value !== 'string' || !/^[A-Za-z0-9_.-]{1,128}$/.test(value)) throw new Error('无效的侧边对话身份。');
  return encodeURIComponent(value);
};

class SideClient {
  #base = ''; #token = ''; #generation = 0; #requests = new Set();
  configure(base, token) { this.clear(); this.#base = String(base).replace(/\/+$/, ''); this.#token = String(token || ''); }
  clear() { this.#generation++; for (const c of this.#requests) c.abort(); this.#requests.clear(); this.#base = ''; this.#token = ''; }
  get configured() { return Boolean(this.#base && this.#token); }
  fork() { const client = new SideClient(); client.configure(this.#base, this.#token); return client; }
  async #request(path, { method = 'POST', body } = {}) {
    if (!this.configured) throw new Error('当前连接不支持侧边对话。');
    const generation = this.#generation, controller = new AbortController(); this.#requests.add(controller);
    const timer = setTimeout(() => controller.abort(new DOMException('侧边对话请求超时。', 'TimeoutError')), 20000);
    try {
      const response = await fetch(this.#base + path, {
        method, signal: controller.signal, redirect: 'error', credentials: 'omit', cache: 'no-store',
        headers: { Authorization: `Bearer ${this.#token}`, ...(body == null ? {} : { 'Content-Type': 'application/json' }) },
        body: body == null ? undefined : JSON.stringify(body),
      });
      if (generation !== this.#generation) throw new DOMException('侧边对话请求已失效。', 'AbortError');
      if (response.status === 204) return null;
      const reader = response.body?.getReader(); const chunks = []; let length = 0;
      if (reader) { try { for (;;) { const { done, value } = await reader.read(); if (done) break; length += value.length; if (length > 8 * 1024 * 1024) throw new Error('旁支展示内容超过限制。'); chunks.push(value); } } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); } }
      if (generation !== this.#generation || controller.signal.aborted) throw new DOMException('侧边请求已失效。', 'AbortError');
      const bytes = new Uint8Array(length); let offset = 0; for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
      let value; try { value = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes)); } catch { throw new Error('旁支响应格式无效。'); }
      if (!response.ok) { const error = new Error(value.message || `侧边对话请求失败：HTTP ${response.status}`); error.status = response.status; throw error; }
      return value;
    } finally { clearTimeout(timer); this.#requests.delete(controller); }
  }
  open(parent, windowId) { return this.#request(`/api/sessions/${id(parent)}/side`, { body: { window_id: windowId } }); }
  start(parent, windowId, prompt, key) { return this.#request(`/api/sessions/${id(parent)}/side/turns`, { body: { window_id: windowId, request_id: key, prompt } }); }
  close(parent, windowId) { return this.#request(`/api/sessions/${id(parent)}/side/${id(windowId)}`, { method: 'DELETE' }); }
}

function windowIdentity() {
  try {
    const key = 'mona.web.window-id'; let value = sessionStorage.getItem(key);
    if (!value) { value = crypto.randomUUID(); sessionStorage.setItem(key, value); }
    return value;
  } catch { return crypto.randomUUID(); }
}

export class SideConversationUI {
  constructor({ pane, session, client, artifactReader, available = () => false, presentation = () => ({}) }) {
    this.events = new AbortController(); const on = (target, type, fn, options = {}) => target.addEventListener(type, fn, { ...options, signal: this.events.signal });
    this.presentation = presentation;
    this.available = available;
    this.pane = pane; this.session = session; this.client = client; this.artifactReader = artifactReader;
    this.api = new SideClient(); this.windowId = windowIdentity(); this.threads = new Map();
    this.desktop = matchMedia('(min-width: 681px)');
    on(this.desktop, 'change', () => this.pane.renderActions());
    this.pane.registerAction({
      id: 'side', label: '侧边对话',
      available: scope => this.api.configured && this.available() && this.desktop.matches && scope.startsWith('session:') && Boolean(this.session()?.id),
      run: () => this.open(),
    });
  }
  dispose() { this.events.abort(); this.clear(); }
  configure(base, token) { this.api.configure(base, token); this.pane.renderActions(); }
  clear() {
    this.api.clear();
    for (const thread of this.threads.values()) void this.disposeThread(thread, false).catch(() => {});
    this.threads.clear(); this.pane.renderActions();
  }
  async open(initial = '', { draft = false } = {}) {
    if (!this.available()) throw new Error('当前宿主未开放侧边对话。');
    const parent = this.session();
    if (!parent?.id) throw new Error('请先打开一个已保存的任务。');
    if (!this.desktop.matches) throw new Error('侧边对话当前仅在桌面布局中使用。');
    const scope = `session:${parent.id}`, tabId = 'side-conversation';
    let thread = this.threads.get(scope);
    if (!thread) {
      thread = this.createThread(parent, scope, tabId);
      this.threads.set(scope, thread);
      try {
        thread.ready = thread.api.open(parent.id, this.windowId);
        const view = await thread.ready;
        if (thread.disposed) throw new DOMException('旁支所属连接已关闭。', 'AbortError');
        this.renderSaved(thread, view);
      } catch (error) {
        if (this.threads.get(scope) === thread) this.threads.delete(scope);
        void this.disposeThread(thread, false).catch(() => {}); throw error;
      }
    }
    if (thread.ready) await thread.ready;
    if (thread.disposed) throw new DOMException('旁支已关闭。', 'AbortError');
    this.pane.openTab({
      id: tabId, title: '侧边对话', kind: 'side', node: thread.root, scope,
      onClose: () => { this.threads.delete(scope); return this.disposeThread(thread, true); },
      onActivate: () => thread.input.focus(),
    });
    if (initial.trim()) {
      thread.input.value = initial.trim();
      if (!draft) await this.submit(thread);
      else thread.input.focus();
    }
  }
  createThread(parent, scope, tabId) {
    const root = node('section', 'side-conversation'), timeline = node('div', 'side-timeline');
    const empty = node('div', 'side-empty'); empty.append(node('strong', '', '侧边对话'), node('p', '', '继承主任务上下文并共享工作文件，不改写主聊天记录。'));
    timeline.append(empty);
    const form = document.createElement('form'); form.className = 'side-composer';
    const input = document.createElement('textarea'); input.rows = 3; input.placeholder = '继续追问、调查或执行一个旁支任务…'; input.setAttribute('aria-label', '侧边对话内容');
    const actions = node('div', 'side-composer-actions'), status = node('span', 'side-status');
    const stop = node('button', 'secondary', '停止'); stop.type = 'button'; stop.hidden = true;
    const send = node('button', 'send', '↑'); send.type = 'submit'; send.setAttribute('aria-label', '发送侧边对话');
    actions.append(status, stop, send); form.append(input, actions); root.append(timeline, form);
    const thread = { parent, scope, tabId, root, timeline, input, status, stop, send, api: this.api.fork(), client: this.client(), turns: new Map(), active: null, subscriptions: new Set() };
    thread.following = true;
    const bottom = node('button', 'text-button side-back-bottom', '回到底部'); bottom.type = 'button'; bottom.hidden = true; root.insertBefore(bottom, form);
    bottom.addEventListener('click', () => { thread.following = true; timeline.scrollTop = timeline.scrollHeight; bottom.hidden = true; });
    timeline.addEventListener('scroll', () => { thread.following = timeline.scrollHeight - timeline.scrollTop - timeline.clientHeight <= 24; bottom.hidden = thread.following; }, { passive: true });
    thread.scheduleScroll = () => {
      if (thread.scrollFrame || thread.disposed) return;
      thread.scrollFrame = requestAnimationFrame(() => { thread.scrollFrame = 0; if (thread.following) timeline.scrollTop = timeline.scrollHeight; bottom.hidden = thread.following || timeline.scrollHeight <= timeline.clientHeight + 24; });
    };
    thread.observer = new ResizeObserver(thread.scheduleScroll);
    timeline.addEventListener('mona:content-resized', thread.scheduleScroll);
    form.addEventListener('submit', event => { event.preventDefault(); void this.submit(thread); });
    input.addEventListener('keydown', event => {
      if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) { event.preventDefault(); void this.submit(thread); }
    });
    stop.addEventListener('click', async () => {
      const runId = thread.active || thread.unconfirmed;
      if (!runId || stop.disabled) return; stop.disabled = true;
      try { await thread.client?.cancel(runId); thread.status.textContent = '正在停止…'; }
      catch (error) { thread.error = `停止请求未确认：${error.message}`; thread.status.textContent = thread.error; }
      finally { stop.disabled = false; }
    });
    return thread;
  }
  renderSaved(thread, view) {
    for (const turn of view?.turns || []) {
      const dom = this.turn(thread, turn.request_id, turn.prompt);
      if (turn.snapshot) {
        const state = new RunView(turn.run_id); state.apply({ kind: 'snapshot', reason: 'initial', snapshot: turn.snapshot }); dom.update(state.state);
        if (!state.state.outcome) void this.watch(thread, turn.run_id, dom, state);
      }
    }
    this.sync(thread);
  }
  turn(thread, key, prompt) {
    let dom = thread.turns.get(key);
    if (dom) return dom;
    thread.timeline.querySelector('.side-empty')?.remove();
    dom = new TurnView(prompt, { artifactReader: this.artifactReader, ...this.presentation(thread.parent) });
    dom.turn.dataset.sideTurn = key; thread.timeline.append(dom.turn); thread.turns.set(key, dom);
    thread.observer.observe(dom.turn); thread.scheduleScroll();
    return dom;
  }
  async submit(thread) {
    const prompt = thread.input.value.trim();
    if (!prompt || thread.active || thread.starting || thread.unconfirmed || thread.disposed) return;
    if (thread.uncertain) { thread.status.textContent = '上次启动结果未确认，请关闭并重开旁支查看，不能直接重复提交。'; return; }
    thread.starting = true; thread.error = ''; thread.following = true;
    const key = requestId(), dom = this.turn(thread, key, prompt);
    thread.input.value = ''; thread.status.textContent = '正在启动…'; thread.send.disabled = true;
    try {
      const response = await thread.api.start(thread.parent.id, this.windowId, prompt, key);
      if (thread.disposed) return;
      const view = new RunView(response.run_id);
      await this.watch(thread, response.run_id, dom, view);
    } catch (error) {
      thread.uncertain = !error.status; thread.error = error?.message || '启动失败';
      dom.fail(`启动未完成：${thread.error}`);
    } finally { thread.starting = false; this.sync(thread); }
  }
  async watch(thread, runId, dom, view) {
    const client = thread.client;
    if (thread.disposed) return;
    if (!client) throw new Error('Agent 连接不可用。');
    thread.active = runId; this.sync(thread);
    try {
      const snapshot = await client.snapshot(runId);
      view.apply({ kind: 'snapshot', reason: 'initial', snapshot }); dom.update(view.state);
      if (view.state.outcome) return;
      const subscription = client.subscribe(runId, {
        after: view.state.seq,
        onFrame: frame => { if (thread.disposed) return; view.apply(frame); dom.update(view.state); if (view.state.outcome) { subscription.close(); this.sync(thread); } },
      });
      thread.subscriptions.add(subscription);
      try {
        await subscription.closed;
        if (!thread.disposed && !view.state.outcome) {
          const snapshot = await client.snapshot(runId); view.apply({ kind: 'snapshot', reason: 'source_resync', snapshot }); dom.update(view.state);
          if (!view.state.outcome) throw new Error('事件流结束但任务尚未结算。');
        }
      } finally { thread.subscriptions.delete(subscription); }
    } catch (error) {
      if (!thread.disposed && !view.state.outcome) {
        thread.unconfirmed = runId; thread.error = '连接中断，请重新连接确认任务状态。';
        dom.connectionLost(thread.error, async () => { thread.unconfirmed = null; thread.error = ''; await this.watch(thread, runId, dom, view); });
      }
    } finally {
      if (thread.active === runId) thread.active = null;
      this.sync(thread);
    }
  }
  sync(thread) {
    const busy = Boolean(thread.active || thread.starting || thread.unconfirmed);
    thread.stop.hidden = !busy; thread.send.disabled = busy || !this.api.configured; thread.input.disabled = busy;
    thread.status.textContent = thread.unconfirmed ? thread.error : busy ? '正在工作' : thread.error || '';
  }
  disposeThread(thread, removeRemote) {
    if (thread.closing) return thread.closing;
    thread.disposed = true; thread.observer?.disconnect(); if (thread.scrollFrame) cancelAnimationFrame(thread.scrollFrame);
    for (const view of thread.turns.values()) view.dispose();
    for (const sub of thread.subscriptions) sub.close();
    thread.subscriptions.clear();
    thread.closing = (async () => {
      try {
        await thread.ready?.catch(() => {});
        if (removeRemote) await thread.api.close(thread.parent.id, this.windowId);
        // UI disposal is observation cleanup, not task cancellation.
      } finally { thread.api.clear(); }
    })();
    return thread.closing;
  }
}
