import { UiHost } from './ui/host.mjs';
import { UiShell } from './ui/shell.mjs';
import { uiRegistry } from './ui/registry.mjs';
import { catalog } from './ui/catalog.mjs';
import { RunView, requestId } from '../../packages/client/src/index.mjs';
import { HttpAgentClient } from '../../packages/client/src/http.mjs';
import { TauriAgentClient } from '../../packages/client/src/tauri.mjs';
import { TurnView } from './run-view.mjs';
import { ConversationUI } from './sessions-ui.mjs';
import { RightPane } from './right-pane.mjs';
import { NavigationLayout, ComposerLayout } from './shell-layout.mjs';
import { ConversationControls } from './conversation-controls.mjs';
import './tooltip.mjs';

const $ = (selector, root = document) => root.querySelector(selector);
const timeline = $('#timeline');
const welcome = $('#welcome');
const prompt = $('#prompt');
const send = $('#send');
const cancel = $('#cancel');
const dialog = $('#connection-dialog');
const form = $('#connection-form');
const transport = $('#transport');
const endpoint = $('#endpoint');
const token = $('#token');
const connectionError = $('#connection-error');
const connectButton = $('#connect');
const httpFields = $('#http-fields');
const main = $('#main');
const chatSidebar = $('#chat-sidebar');
const chatWorkspace = $('#chat-workspace');
const settingsPage = $('#settings-page');
const shell = $('.shell');

let client = null;
let active = null;
let renderFrame = 0;
let workTimer = null;
let starting = false;
let ui = null;
let connectionGeneration = 0;
let desktopProduct = false;
let desktopOpenWith = [];
const rightPane = new RightPane();
const navigationLayout = new NavigationLayout({ pane: rightPane });
const composerLayout = new ComposerLayout();
let conversationControls = null;
const turnViews = new Set();
const conversations = new ConversationUI({
  busy: () => starting || Boolean(active && !active.done) || Boolean(ui?.locks.size),
  changed: () => {
    setBusy(Boolean(active && !active.done)); ui?.emit('session', conversations.selected);
  },
  findProject: id => ui?.service('workspace')?.projectState?.projects.find(p => p.id === id),
  createTurn,
  beforeHistoryChange: () => conversationControls?.beforeChange(),
  afterHistoryChange: anchor => conversationControls?.afterChange(anchor),
  historyOpened: id => conversationControls?.activate(id),
  historyBefore: id => conversationControls?.historyBefore(id),
  forgetReading: id => { conversationControls?.memory.delete(id); if (conversationControls?.scope === id) { conversationControls.scope = ''; conversationControls.anchor = null; } },
  openSettings: () => showSettings(),
  clear() {
    active?.subscription?.close(); clearInterval(workTimer);
    if (renderFrame) cancelAnimationFrame(renderFrame);
    conversationControls?.clear();
    ui?.emit('cleared');
    for (const view of turnViews) view.dispose(); turnViews.clear();
    renderFrame = 0; active = null; timeline.replaceChildren(); welcome.hidden = false;
  },
  async live(runId, dom) {
    const snapshot = await client.snapshot(runId);
    const view = new RunView(runId);
    view.apply({ kind: 'snapshot', reason: 'initial', snapshot });
    void subscribeRun(runId, view, dom);
  },
});

const slots = new UiShell(uiRegistry, rightPane, message => conversations.notice(message, true));
ui = new UiHost({ catalog, registry: uiRegistry, onError: message => conversations.notice(message, true), shell: {
  session: () => conversations.selected,
  project: () => conversations.project,
  selectProject: project => conversations.selectProject(project),
  projectsRendered: projects => conversations.setProjects(projects),
  showMoreProject: id => conversations.showMore(id),
  desktopMode: () => desktopProduct,
  desktopOpenWith: () => desktopOpenWith,
  openDesktopWorkspace: (sessionId, application) => {
    if (!desktopProduct || !desktopOpenWith.includes(application)) throw new Error('当前连接没有系统打开方式。');
    const native = globalThis.__MONA_TAURI__ ?? globalThis.__TAURI__?.core;
    return native.invoke('desktop_open_workspace', { sessionId, application });
  },
  pickDesktopProject: (requestId, revision) => {
    if (!desktopProduct) throw new Error('当前连接没有桌面项目选择能力。');
    const native = globalThis.__MONA_TAURI__ ?? globalThis.__TAURI__?.core;
    return native.invoke('desktop_pick_project', { requestId, revision });
  },
  async projectsChanged(state) {
    if (conversations.project && !state?.projects.some(p => p.id === conversations.project.id)) { conversations.project = null; conversations.title(); }
    await conversations.refreshList();
  },
  client: () => client,
  presentation: session => ui.presentation(session),
  pane: owner => slots.ownedPane(owner), releaseOwner: owner => slots.releaseOwner(owner),
  ensureSession: () => conversations.ensureSession(),
  refreshSessionHeader: id => conversations.refreshHeader(id),
  focusPrompt: () => prompt.focus(),
  draft: () => prompt.value,
  busyChanged: () => setBusy(Boolean(active && !active.done)),
  showSettings,
  async openSession(id) {
    if (starting || conversations.loading || (active && !active.done)) throw new Error('请先等待当前任务结束再打开历史会话。');
    await conversations.open(id); hideSettings();
  },
  async submitText(value, sessionId = null) {
    if (sessionId && conversations.selected?.id !== sessionId) throw new Error('会话已切换，未向其他会话发送任务。');
    if (starting || conversations.loading || (active && !active.done)) throw new Error('当前任务尚未结束。');
    if (prompt.value.trim()) throw new Error('输入框已有草稿，请发送或保留草稿后继续。');
    prompt.value = value; await submitPrompt();
  },
}});
conversationControls = new ConversationControls({
  main, timeline,
  hasOlder: () => conversations.nextBefore != null,
  loadOlder: () => conversations.loadOlder(),
  prepareSearch: query => { for (const turn of turnViews) turn.prepareSearch(query); },
  onQuote: reference => {
    const quote = reference.text.split('\n').map(line => '> ' + line).join('\n');
    prompt.value += `${prompt.value ? '\n\n' : ''}${quote}\n\n`;
    prompt.dispatchEvent(new Event('input', { bubbles: true })); prompt.focus();
  },
  onSide: reference => ui.service('side')?.open(`引用当前对话：\n${reference.text.split('\n').map(line => '> ' + line).join('\n')}\n\n`, { draft: true }),
  canSide: () => Boolean(conversations.selected && ui.service('side')?.available() && matchMedia('(min-width: 901px)').matches),
});
document.addEventListener('mona:ui-error', event => conversations.notice(String(event.detail || 'UI 模块错误'), true));

function clearConnection() {
  connectionGeneration++; client = null; desktopProduct = false; desktopOpenWith = []; conversations.clear();
  conversationControls?.memory.clear(); ui.disconnect(); rightPane.clear(); setBusy(false);
}

function showSettings(section = 'components') {
  closeSidebar();
  chatSidebar.hidden = true;
  chatWorkspace.hidden = true;
  settingsPage.hidden = false;
  rightPane.settingsVisibility(true); slots.show(section);
}

function hideSettings() {
  settingsPage.hidden = true;
  rightPane.settingsVisibility(false); slots.hide();
  chatSidebar.hidden = false;
  chatWorkspace.hidden = false;
  prompt.focus();
}

function setBusy(busy) {
  composerLayout.schedule();
  send.disabled = starting || !client || !prompt.value.trim() || !conversations.canSend || Boolean(ui?.locks.size);
  conversations.controls(); ui?.emit('busy', Boolean(starting || busy || conversations.loading));
  cancel.hidden = !busy;
  send.dataset.action = busy ? 'append' : 'send';
  send.dataset.tooltip = busy ? '向当前任务追加指令' : '发送任务';
  send.setAttribute('aria-label', send.dataset.tooltip);
}

function text(tag, className, value) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  node.textContent = value;
  return node;
}

function createTurn(userText, options = {}) {
  welcome.hidden = true;
  const session = options.session || conversations.selected;
  const dom = new TurnView(userText, ui.presentation(session));
  if (options.turn?.id) dom.turn.dataset.turnId = options.turn.id;
  if (session?.id) dom.turn.dataset.sessionId = session.id;
  turnViews.add(dom);
  timeline.insertBefore(dom.turn, options.before || null);
  if (!options.history) conversationControls?.jumpBottom();
  return dom;
}

function renderActive() {
  renderFrame = 0;
  if (!active) return;
  const state = active.view.state;
  active.dom.update(state);
  ui.emit('run', state);
  if (state.outcome) {
    if (!active.done) {
      active.done = true;
      active.subscription?.close();
      clearInterval(workTimer);
      void conversations.settled();
    }
    setBusy(false);
  } else setBusy(true);
  conversationControls?.schedule();
}

function scheduleRender() {
  if (renderFrame) return;
  renderFrame = requestAnimationFrame(renderActive);
}

async function subscribeRun(runId, view, dom) {
  const run = { runId, view, dom, subscription: null, done: false };
  active = run;
  clearInterval(workTimer);
  scheduleRender();
  if (view.state.outcome) return;
  const subscription = client.subscribe(runId, {
    after: view.state.seq,
    onFrame(frame) {
      if (active !== run) return;
      view.apply(frame);
      scheduleRender();
    },
  });
  run.subscription = subscription;
  workTimer = setInterval(() => dom.tick(), 1000);
  try {
    await subscription.closed;
    if (active !== run) return;
    if (!view.state.outcome) {
      const snapshot = await client.snapshot(runId);
      if (active !== run) return;
      view.apply({ kind: 'snapshot', reason: 'source_resync', snapshot });
      if (!snapshot.outcome) throw new Error('事件流提前结束，请刷新历史重新连接');
      scheduleRender();
    }
  } catch (error) {
    if (active !== run) return;
    clearInterval(workTimer);
    run.disconnected = true;
    dom.connectionLost('实时连接已中断，任务可能仍在执行；重新连接只读取状态，不会重复执行工具。', async () => {
      if (active !== run) return;
      let snapshot;
      try { snapshot = await client.snapshot(runId); }
      catch (error) {
        if (active === run && conversations.persistent && conversations.selected && (error.status === 404 || error.code === 'not_found')) {
          const id = conversations.selected.id; active = null; setBusy(false); conversations.notice('实时任务已不在宿主中，正在读取已保存记录；不会重新执行。'); await conversations.open(id, null); return;
        }
        throw error;
      }
      if (active !== run) return;
      view.apply({ kind: 'snapshot', reason: 'source_resync', snapshot }); run.disconnected = false;
      conversations.notice(''); await subscribeRun(runId, view, dom);
    });
    conversations.notice('实时连接已中断。请在本轮选择“重新连接”确认状态；取消请求不代表已回滚。', true);
    setBusy(true);
  }
}

async function submitPrompt() {
  const value = prompt.value.trim();
  if (!value || !client || starting || !conversations.canSend || ui.locks.size) return;
  try {
    if (await ui.command(value)) { prompt.value = ''; setBusy(Boolean(active && !active.done)); return; }
  } catch (error) { conversations.notice(error?.message || '操作未完成。', true); return; }
  prompt.value = '';
  setBusy(Boolean(active && !active.done));
  if (active && !active.done) {
    const note = text('div', 'user-message supplement', value);
    active.dom.turn.append(note);
    active.dom.setSupplemental(true);
    try {
      await client.input(active.runId, { request_id: requestId(), text: value });
    } catch (error) {
      note.append(text('span', 'error-detail', `发送失败：${error?.message || error}`));
    }
    return;
  }

  if (!conversations.persistent) $('#thread-title').textContent = value.slice(0, 26) || '临时任务';
  const dom = createTurn(value);
  starting = true;
  setBusy(false);
  try {
    const request = { request_id: requestId(), prompt: value };
    const response = conversations.persistent ? await conversations.submit(value) : await client.start(request);
    if (conversations.selected) {
      dom.bindPresentation(ui.presentation(conversations.selected));
      dom.turn.dataset.sessionId = conversations.selected.id;
      conversationControls?.activate(conversations.selected.id, false);
    }
    dom.turn.dataset.turnId ||= response.turn?.id || response.run_id || request.request_id;
    starting = false;
    if (conversations.persistent && (response.reused || !response.live || !response.run_id)) {
      await conversations.open(response.session.id);
    } else {
      const view = new RunView(response.run_id);
      void subscribeRun(response.run_id, view, dom);
    }
  } catch (error) {
    starting = false;
    dom.fail(`启动失败：${error?.message || error}`);
    if (!prompt.value) prompt.value = value;
    setBusy(false);
  }
}

async function connectHttp(baseUrl, bearer, { desktop = false, openWith = [] } = {}) {
  clearConnection();
  desktopProduct = desktop;
  desktopOpenWith = desktop ? openWith : [];
  const generation = connectionGeneration;
  const candidate = new HttpAgentClient({
    baseUrl,
    token: bearer,
    fetch: globalThis.fetch.bind(globalThis),
    maxEventBytes: 16 * 1024 * 1024,
  });
  const response = await fetch(baseUrl.replace(/\/$/, '') + '/v1/info', {
    headers: { Authorization: `Bearer ${bearer}` },
    credentials: 'omit',
    redirect: 'error',
    cache: 'no-store',
    signal: AbortSignal.timeout(12000),
  });
  if (!response.ok) throw new Error(`连接验证失败：HTTP ${response.status}`);
  const info = await response.json();
  if (info.protocol_version !== 2 || info.stream !== 'sse') throw new Error('服务未提供 Stream v2 / SSE。');
  if (generation !== connectionGeneration) return;
  client = candidate;
  await ui.connect(baseUrl, bearer);
  if (generation !== connectionGeneration) return;
  setBusy(false);
  await conversations.configure(baseUrl, bearer, { projectsEnabled: ui.service('workspace')?.settings?.projects_enabled === true });

}

async function connectTauri() {
  clearConnection();
  desktopProduct = false;
  const native = globalThis.__MONA_TAURI__ ?? globalThis.__TAURI__?.core;
  if (!native?.invoke || !native?.Channel) throw new Error('当前页面不在已配置的 Tauri 宿主中。');
  client = new TauriAgentClient({ invoke: native.invoke, Channel: native.Channel });
  // Product management requires a host adapter; native run transport remains independent.
  conversations.ephemeral('当前 Tauri 宿主尚未装配会话管理；此连接仅支持临时任务。正式 Web 宿主支持本地保存。');
  setBusy(false);
}

async function connectDesktop() {
  const native = globalThis.__MONA_TAURI__ ?? globalThis.__TAURI__?.core;
  const config = await native.invoke('desktop_config');
  if (typeof config?.endpoint !== 'string' || typeof config?.token !== 'string') throw new Error('桌面宿主连接信息无效。');
  endpoint.value = config.endpoint;
  const openWith = Array.isArray(config.open_with) ? [...new Set(config.open_with.filter(id => ['explorer', 'terminal'].includes(id)))] : [];
  await connectHttp(config.endpoint, config.token, { desktop: true, openWith });
}

async function autoConnect() {
  const native = globalThis.__MONA_TAURI__ ?? globalThis.__TAURI__?.core;
  if (native?.invoke && new URLSearchParams(location.search).get('monaDesktop') === '1') {
    const choice = document.createElement('option'); choice.value = 'desktop'; choice.textContent = '桌面宿主';
    transport.replaceChildren(choice); transport.value = 'desktop'; httpFields.hidden = true;
    try {
      await connectDesktop();
    } catch (error) {
      clearConnection(); desktopProduct = false;
      connectionError.textContent = `桌面宿主连接失败：${error?.message || error}`;
      if (!dialog.open) dialog.showModal();
    }
    return;
  }
  if (native?.invoke && native?.Channel) {
    await connectTauri();
    return;
  }
  try {
    const response = await fetch('/__mona_dev_config__.json', { cache: 'no-store', credentials: 'same-origin' });
    if (!response.ok) throw new Error('no dev config');
    const config = await response.json();
    endpoint.value = config.endpoint;
    await connectHttp(config.endpoint, config.token);
  } catch {
    clearConnection();
  }
}

transport.addEventListener('change', () => { httpFields.hidden = transport.value !== 'http'; });
form.addEventListener('submit', async (event) => {
  event.preventDefault();
  connectionError.textContent = '';
  connectButton.disabled = true;
  try {
    if (starting || conversations.loading || (active && !active.done)) throw new Error('会话正在加载、保存或执行，请等待完成；运行中的任务需先停止并等待最终状态。');
    if (transport.value === 'desktop') await connectDesktop();
    else if (transport.value === 'tauri') await connectTauri();
    else await connectHttp(endpoint.value.trim().replace(/\/$/, ''), token.value);
    token.value = '';
    dialog.close();
  } catch (error) {
    connectionError.textContent = error?.message || String(error);
  } finally {
    connectButton.disabled = false;
  }
});

send.addEventListener('click', submitPrompt);
prompt.addEventListener('input', () => setBusy(Boolean(active && !active.done)));
prompt.addEventListener('keydown', (event) => {
  if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
    event.preventDefault();
    void submitPrompt();
  }
});
cancel.addEventListener('click', async () => {
  if (!active || active.done || !client) return;
  cancel.disabled = true;
  try {
    await client.cancel(active.runId);
  } catch (error) {
    active.dom.footer.textContent = `取消信号发送失败：${error?.message || error}`;
  } finally {
    cancel.disabled = false;
  }
});
$('#new-chat').addEventListener('click', () => {
  if (starting || (active && !active.done)) {
    alert('请先停止当前任务并等待最终状态。');
    return;
  }
  if (!conversations.newChat()) return;
  closeSidebar();
  prompt.focus();
});
$('#settings-button').addEventListener('click', () => showSettings());
function closeSidebar() { navigationLayout.close(); }
document.addEventListener('keydown', event => {
  if (!event.defaultPrevented && (event.ctrlKey || event.metaKey) && !event.altKey && event.key.toLowerCase() === 'n' && !document.querySelector('dialog[open]') && !chatWorkspace.inert) {
    event.preventDefault(); $('#new-chat').click(); closeSidebar();
  }
});
$('#settings-back').addEventListener('click', hideSettings);
for (const [dialogElement, canDismiss] of [
  [dialog, () => !connectButton.disabled],
]) {
  for (const button of dialogElement.querySelectorAll('button[value="cancel"]')) {
    button.addEventListener('click', (event) => {
      event.preventDefault();
      if (canDismiss()) dialogElement.close('cancel');
    });
  }
  dialogElement.addEventListener('cancel', (event) => {
    if (!canDismiss()) event.preventDefault();
  });
  dialogElement.addEventListener('click', (event) => {
    if (event.target !== dialogElement || !canDismiss()) return;
    const rect = dialogElement.getBoundingClientRect();
    const inside = event.clientX >= rect.left && event.clientX <= rect.right
      && event.clientY >= rect.top && event.clientY <= rect.bottom;
    if (!inside) dialogElement.close('cancel');
  });
}

setBusy(false);
await ui.init();
void autoConnect();
