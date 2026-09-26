import { artifactViewer, renderMessage, richContentBlock, structuredDetailsBlock, disposeMessage } from './content-renderer.mjs';
import { button, copyText, appendPlain } from './content-dom.mjs';
import { fileReferenceCards } from './file-reference-cards.mjs';
import { ProcessScroll } from './process-scroll.mjs';
import { paneIcon } from './pane-icons.mjs';
import { workSummary, canFoldTurn, category } from './conversation-policy.mjs';

// Presentation only: all work items and outcomes come from the shared RunView.
const states = { pending: '等待', running: '执行中', completed: '完成', failed: '失败', cancelled: '已取消', denied: '已拒绝', skipped: '已跳过', unknown: '状态未知' };
export const statusText = state => states[state] || state;
export const outcomeText = outcome => ({ completed: '任务完成', failed: '任务失败', cancelled: '任务已取消', timed_out: '任务超时', limited: '达到运行限制' })[outcome.status] || outcome.status;
const REDACTED_ERROR_MESSAGE = 'run stopped; inspect trusted host diagnostics for details';
const ERROR_HINTS = {
  model_transport: '模型请求没有正常结束，请检查供应商地址、网络连接，或供应商是否提前关闭了响应流。',
  model_protocol: '模型响应格式异常，请检查供应商接口和模型协议。',
  model_authentication: '模型服务拒绝了凭据，请检查 API Key 与账号权限。',
  model_quota: '模型额度或账单不足，请检查供应商余额与套餐。',
  model_rate_limit: '模型服务正在限流，请稍后重试或降低请求频率。',
  model_server: '供应商服务暂时不可用，请稍后重试。',
  model_context_window: '请求超出模型上下文上限，请减少上下文或改用更大窗口的模型。',
  model_request: '供应商拒绝了这次请求，请检查模型名称、接口路径和请求参数。',
  configuration: '运行配置有误，请检查模型设置、工作区和 Agent 组件配置。',
  deadline: '模型请求超时，请检查网络或调整超时设置。',
  limit: '已达到运行限制，请减少任务规模或调整限制配置。',
};

export function outcomeErrorText(error) {
  if (!error || error.message !== REDACTED_ERROR_MESSAGE) return error?.message || '';
  return ERROR_HINTS[error.code] || '任务执行失败，请查看可信宿主诊断。';
}

const text = (tag, className, value = '') => {
  const node = document.createElement(tag);
  node.className = className;
  node.textContent = value;
  return node;
};

function messageCopyButton(label, readText) {
  const action = button('', () => { void copyText(readText(), action); }, 'message-icon-action');
  action.setAttribute('aria-label', label); action.dataset.tooltip = label;
  action.append(paneIcon('copy'));
  const success = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  success.setAttribute('viewBox', '0 0 24 24'); success.setAttribute('class', 'line-icon copy-success-icon'); success.setAttribute('aria-hidden', 'true');
  const check = document.createElementNS(success.namespaceURI, 'path'); check.setAttribute('d', 'm5 12 4 4L19 6'); success.append(check); action.append(success);
  return action;
}

export function durationText(ms) {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  return seconds < 60 ? `${seconds} 秒` : `${Math.floor(seconds / 60)} 分 ${seconds % 60} 秒`;
}

// Task rows carry a relative stamp; anything older than a month falls back to a date.
export function relativeTime(value, now = Date.now()) {
  const at = Number(value);
  if (!Number.isFinite(at) || at <= 0) return '';
  const seconds = Math.floor((now - at) / 1000);
  if (seconds < 60) return '刚刚';
  if (seconds < 3600) return `${Math.floor(seconds / 60)}分钟`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}小时`;
  const days = Math.floor(seconds / 86400);
  return days < 30 ? `${days}天` : new Date(at).toLocaleDateString();
}

export function partitionItems(state) {
  const items = state.items;
  const last = items.at(-1);
  // Protocol v2 has no reasoning channel. Intermediate messages stay labelled as messages.
  const final = last?.content.kind === 'agent_message' && (state.outcome?.status === 'completed' || !state.outcome) ? last : null;
  const output = state.outcome?.output;
  const answer = output != null ? output : final?.content.text || '';
  // Deduplicate only the terminal message actually represented by the final output.
  const exclude = final && (output == null || final.content.text === output) ? final.id : null;
  // Each model call opens a message item, so a tool-only response leaves one empty: it must not become a row.
  const visible = item => item.content.kind !== 'agent_message' || Boolean(item.content.text?.trim());
  return { process: items.filter(item => item.id !== exclude && visible(item)), answer, truncated: exclude && final.content.truncated };
}

// Presentation only: consecutive tool activity shares a group; ordinary narration
// remains a readable flow block, never a reasoning label or a generic tool card.
const ROW_LABELS = { shell: '终端', read: '读取', search: '搜索', write: '写入' };
// Thin line icons follow the shared .line-icon stroke contract.
const ICON_PATHS = {
  shell: ['M4 5.5h16v13H4z', 'm7.5 10 2.5 2-2.5 2', 'M13 14h3.5'],
  read: ['M6.5 3.5h7L18 8v12.5H6.5z', 'M13.5 3.5V8H18', 'M9.5 12.5h5', 'M9.5 16h5'],
  search: ['M11 4.5a6.5 6.5 0 1 1 0 13 6.5 6.5 0 0 1 0-13Z', 'm15.8 15.8 4.2 4.2'],
  write: ['M4.5 19.5h15', 'M14.5 4.5 18 8l-9 9H5.5v-3.5z'],
  message: ['M4.5 6.5h15v9.5h-10l-4.5 4v-4H4.5z'],
  tool: ['M6 6h12v12H6z', 'M9.5 9.5h5v5h-5z', 'M12 3v3M12 18v3M3 12h3M18 12h3'],
};
const SVG_NS = 'http://www.w3.org/2000/svg';
function iconNode(kind) {
  const svg = document.createElementNS(SVG_NS, 'svg');
  svg.setAttribute('class', 'line-icon');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('aria-hidden', 'true');
  for (const definition of ICON_PATHS[kind] || ICON_PATHS.tool) {
    const path = document.createElementNS(SVG_NS, 'path');
    path.setAttribute('d', definition);
    svg.append(path);
  }
  return svg;
}
const STATE_PRIORITY = ['failed', 'denied', 'unknown', 'cancelled', 'running', 'pending', 'skipped', 'completed'];
const toolKind = category;

function argumentsOf(content) {
  return content.arguments && typeof content.arguments === 'object' ? content.arguments : null;
}

// Terminal output carries ANSI control sequences (color, cursor); the desktop agent strips them
// for shell results and so does this view, which only shows plain text.
const ANSI_SEQUENCE = /\x1b\[[0-?]*[ -/]*[@-~]/g;

// A message can start with a blank line; the preview shows the first line that has content.
function firstLine(value) {
  return String(value || '').split('\n').map(line => line.trim()).find(Boolean) || '';
}

// The one-line row preview is plain text, so markdown markers are removed from it.
function previewLine(value) {
  return firstLine(value)
    .replace(/^#{1,6}\s+/, '')
    .replace(/^(?:[-*+]|\d+[.)])\s+/, '')
    .replace(/\[([^\]]*)\]\([^)\s]+\)/g, '$1')
    .replace(/[*`~]/g, '');
}

// Preparation is not dispatch: never parse partial JSON or expose guessed paths.
function previewOf(content) {
  const args = argumentsOf(content);
  const value = args ? args.command ?? args.cmd ?? args.path ?? args.file_path ?? args.query ?? args.pattern : '';
  if (typeof value === 'string' && value) return value;
  return '';
}

function commandText(content, kind) {
  if (kind !== 'shell') return content.arguments_text || (content.arguments != null ? JSON.stringify(content.arguments, null, 2) : '');
  const args = argumentsOf(content);
  const command = args ? args.command ?? args.cmd : null;
  return typeof command === 'string' && command ? command : previewOf(content);
}

// Consecutive work stays together across model steps. A visible reply is a real boundary.
export function itemBlocks(process) {
  const blocks = [];
  for (const item of process) {
    const work = item.content.kind === 'tool_call', last = blocks.at(-1);
    if (work && last?.key) last.items.push(item);
    else blocks.push({ key: work ? `work-${item.id}` : null, items: [item] });
  }
  return blocks;
}

function groupState(items) {
  const seen = new Set(items.map(item => item.state));
  return STATE_PRIORITY.find(state => seen.has(state)) || 'unknown';
}

// Reuse existing nodes so expansion state survives incremental updates.
function syncChildren(container, nodes) {
  nodes.forEach((node, index) => {
    if (container.children[index] !== node) container.insertBefore(node, container.children[index] || null);
  });
  while (container.children.length > nodes.length) container.lastElementChild.remove();
}

// A row with nothing to show stays a plain line instead of an expandable step.
function hasDetail(item) {
  const content = item.content;
  if (content.kind === 'tool_call' && item.state === 'pending') return false;
  if (content.kind !== 'tool_call') return Boolean(content.text);
  return Boolean(content.arguments_text || content.arguments != null || content.output || content.result
    || (content.details && Object.keys(content.details).length));
}

function createItemEntry(item) {
  const node = text('details', 'activity-item');
  node.dataset.itemId = item.id;
  const summary = text('summary', 'activity-summary');
  const entry = {
    node, summary,
    icon: text('span', 'activity-icon'),
    name: text('span', 'activity-name'),
    preview: text('span', 'activity-preview'),
    state: text('span', 'activity-state'),
    body: text('div', 'activity-body'),
  };
  entry.icon.setAttribute('aria-hidden', 'true');
  summary.addEventListener('click', (event) => { if (summary.classList.contains('is-static')) event.preventDefault(); });
  summary.append(entry.icon, entry.name, entry.preview, entry.state, text('span', 'chevron', '›'));
  node.append(summary, entry.body);
  return entry;
}

function createGroupEntry() {
  const node = text('details', 'activity-group');
  const summary = text('summary', 'activity-group-summary');
  const group = {
    node, summary,
    icon: text('span', 'activity-icon'),
    name: text('span', 'activity-group-name'),
    state: text('span', 'activity-group-state'),
    body: text('div', 'activity-group-body'),
    content: text('div', 'activity-group-content'),
  };
  const leading = text('span', 'activity-leading');
  const chevron = text('span', 'chevron', '›'); chevron.setAttribute('aria-hidden', 'true');
  group.icon.setAttribute('aria-hidden', 'true'); leading.append(group.icon, chevron);
  summary.append(leading, group.name, group.state);
  group.body.id = `process-body-${crypto.randomUUID()}`; group.body.setAttribute('role', 'region'); group.body.setAttribute('aria-label', '工具执行记录');
  summary.setAttribute('aria-controls', group.body.id); group.body.append(group.content);
  node.append(summary, group.body); group.scroll = new ProcessScroll(node, group.body, group.content);
  group.titleMinimum = Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--motion-fast')) || 150;
  node.addEventListener('toggle', event => { if (event.target === node) summary.setAttribute('aria-expanded', String(node.open)); });
  group.dispose = () => { clearTimeout(group.titleTimer); group.scroll.dispose(); };
  return group;
}

// Only a live title is rate-limited; failures and settled titles publish immediately.
function presentGroup(group) {
  const items = group.items, active = [...items].reverse().find(item => ['running', 'pending'].includes(item.state));
  const status = groupState(items), label = workSummary(items, { running: Boolean(active), detail: true });
  const kind = toolKind((active || items[0]).content.name);
  const publish = () => {
    const desired = group.desiredTitle; group.titleTimer = null;
    group.name.textContent = desired.label; group.summary.dataset.tooltip = desired.label;
    if (group.iconKind !== desired.kind) { group.icon.replaceChildren(iconNode(desired.kind)); group.iconKind = desired.kind; }
    group.displayedAt = performance.now();
  };
  group.desiredTitle = { label, kind };
  if (label !== group.name.textContent || kind !== group.iconKind) {
    const wait = group.displayedAt == null ? 0 : group.titleMinimum - (performance.now() - group.displayedAt);
    if (!active || wait <= 0 || ['failed', 'denied', 'unknown', 'cancelled'].includes(status)) { clearTimeout(group.titleTimer); publish(); }
    else if (!group.titleTimer) group.titleTimer = setTimeout(publish, wait);
  } else { clearTimeout(group.titleTimer); group.titleTimer = null; }
  group.node.className = `activity-group state-${status}`;
  group.state.textContent = ['completed', 'running', 'pending'].includes(status) ? '' : statusText(status);
  group.state.hidden = !group.state.textContent;
}

// Tool panels are mounted lazily and updated by field; log/JSON/artifact state survives streaming.
function updateToolPanel(entry) {
  const { item, runId, artifactReader, options } = entry.latestTool;
  const content = item.content, kind = toolKind(content.name);
  const clean = value => typeof value === 'string' ? (kind === 'shell' ? value.replace(ANSI_SEQUENCE, '') : value) : '';
  let parts = entry.parts;
  if (!parts) {
    const root = text('div', 'activity-panel'), command = text('div', 'activity-panel-code'), path = text('div', 'tool-file-target');
    const log = text('pre', 'activity-panel-output'), result = text('pre', 'activity-panel-result'), media = text('div', 'tool-result-media'), artifacts = text('div', 'tool-result-artifacts'), details = text('div', 'tool-result-details'), notice = text('p', 'muted-note');
    const follow = button('跟随最新输出', () => { log.scrollTop = log.scrollHeight; parts.following = true; follow.hidden = true; }); follow.hidden = true;
    root.append(path, command, log, follow, result, media, artifacts, details, notice);
    parts = { root, path, command, log, follow, result, media, artifacts, details, notice, following: true, disposed: false,
      dispose() { this.disposed = true; for (const holder of [media, artifacts, details]) for (const child of holder.children) child.dispose?.(); } };
    log.addEventListener('scroll', () => { parts.following = log.scrollHeight - log.scrollTop - log.clientHeight < 12; follow.hidden = parts.following; }, { passive: true });
    entry.parts = parts; entry.body.replaceChildren(root);
  }
  const command = commandText(content, kind); appendPlain(parts.command, (kind === 'shell' ? '$ ' : '') + command); parts.command.hidden = !command;
  const args = argumentsOf(content), file = ['read', 'write'].includes(kind) && (args?.path || args?.file_path);
  if (file !== parts.file || parts.scope !== options.contextKey) {
    parts.file = file; parts.scope = options.contextKey; parts.path.replaceChildren();
    if (typeof file === 'string' && options.openFile) parts.path.append(button(file, () => { Promise.resolve().then(() => options.openFile(file)).catch(error => options.onError?.(error.message)); }, 'message-file-link'));
  }
  const output = clean(content.output); const following = parts.following;
  appendPlain(parts.log, output); parts.log.hidden = !output;
  if (following) queueMicrotask(() => { if (!parts.disposed && parts.following) parts.log.scrollTop = parts.log.scrollHeight; });
  const resultText = clean(content.result?.content); const rich = Array.isArray(content.result?.blocks) && content.result.blocks.length;
  parts.result.hidden = Boolean(rich) || !resultText.trim() || resultText.trim() === output.trim(); if (!parts.result.hidden) appendPlain(parts.result, resultText);
  for (const [key, holder, value, factory] of [
    ['rich', parts.media, content.result?.blocks, () => richContentBlock(content.result.blocks, { ...options, runId, artifactReader })],
    ['artifact', parts.artifacts, content.result?.artifact, () => artifactViewer(content.result.artifact, { ...options, runId, artifactReader })],
    ['details', parts.details, content.details, () => structuredDetailsBlock(content.details, options)],
  ]) {
    const signature = JSON.stringify(value || null);
    if (signature === parts[key + 'Signature']) continue;
    parts[key + 'Signature'] = signature; for (const child of holder.children) child.dispose?.(); holder.replaceChildren();
    if (value) { const node = factory(); if (node) holder.append(node); }
  }
  parts.notice.textContent = [content.arguments_truncated ? '参数显示已截断。' : '', content.output_truncated ? '日志显示已截断。' : '', content.result?.truncated ? (content.result.redacted ? '部分媒体来源未开放给界面。' : '结果预览已截断。') : ''].filter(Boolean).join(' ');
  parts.notice.hidden = !parts.notice.textContent;
}

function updateMessagePanel(entry, item, options) {
  if (!entry.messagePanel) {
    entry.messagePanel = text('div', 'activity-message-panel');
    entry.messageText = text('div', 'message-text');
    entry.messageTruncated = text('p', 'muted-note', '消息过长，已截断显示。');
    entry.messagePanel.append(entry.messageText, entry.messageTruncated);
    entry.body.replaceChildren(entry.messagePanel);
  }
  void renderMessage(entry.messageText, item.content.text || '等待回复…', { ...options, streaming: item.state === 'running' });
  entry.messageTruncated.hidden = !item.content.truncated;
}

function updateItem(entry, item, runId, artifactReader, options) {
  const content = item.content, tool = content.kind === 'tool_call';
  const kind = tool ? toolKind(content.name) : 'message';
  const preview = tool ? previewOf(content) : previewLine(content.text);
  entry.node.className = `activity-item state-${item.state} kind-${kind}`;
  entry.icon.className = `activity-icon icon-${kind}`;
  if (entry.iconKind !== kind) { entry.icon.replaceChildren(iconNode(kind)); entry.iconKind = kind; }
  entry.name.textContent = tool ? ROW_LABELS[kind] || content.name || '工具' : '模型回复';
  entry.preview.textContent = item.state === 'pending' && tool ? (['write', 'edit'].includes(content.name) && content.arguments_text ? `正在准备内容 ${Math.ceil(content.arguments_text.length / 1024)} KB` : '正在准备') : preview;
  entry.preview.hidden = !entry.preview.textContent;
  entry.preview.classList.toggle('is-command', tool);
  // Completed rows stay silent like the reference; every other state must remain visible.
  entry.state.textContent = item.state === 'completed' ? '' : statusText(item.state);
  entry.state.hidden = !entry.state.textContent;
  entry.summary.dataset.tooltip = preview || (tool ? content.name || '工具' : '展开模型回复');
  entry.summary.classList.toggle('is-static', !hasDetail(item));
  const signature = JSON.stringify([content, item.state, options.contextKey]);
  if (signature === entry.signature) return;
  entry.signature = signature;
  if (tool) {
    entry.latestTool = { item, runId, artifactReader, options };
    if (!entry.toolMounted) {
      entry.toolMounted = true;
      entry.node.addEventListener('toggle', () => { if (entry.node.open && entry.latestTool) updateToolPanel(entry); });
    }
    if (entry.node.open) updateToolPanel(entry);
  } else { entry.node.open = true; updateMessagePanel(entry, item, options); }
}

export class TurnView {
  constructor(userText, { artifactReader, ...presentation } = {}) {
    this.presentation = presentation; this.userText = userText; this.disposed = false; this.streaming = true;
    this.startedAt = performance.now();
    this.endedAt = null;
    this.entries = new Map();
    this.groups = new Map();
    this.processAutoOpened = false;
    this.artifactReader = artifactReader;
    this.renderedAnswer = undefined;
    this.renderedTruncated = false;
    this.pendingAnswer = '';
    this.pendingTruncated = false;
    this.answerRenderTimer = 0;
    this.lastAnswerRenderAt = 0;
    this.turn = text('article', 'turn');
    this.process = text('details', 'execution-process');
    this.summary = text('summary', 'execution-summary');
    this.clock = text('span', 'execution-clock', '正在启动…');
    this.clock.dataset.tooltip = '本页面从发起请求到收到结束状态的用时';
    this.summary.append(this.clock, text('span', 'chevron', '›'));
    this.items = text('div', 'activity-list');
    this.process.append(this.summary, this.items);
    this.answer = text('div', 'message-text');
    this.answerTruncated = text('p', 'muted-note message-truncated-note', '消息过长，已截断显示。'); this.answerTruncated.hidden = true;
    this.footer = text('div', 'run-footer');
    this.footer.setAttribute('role', 'status');
    this.pruned = text('p', 'muted-note');
    this.pruned.hidden = true;
    const body = text('div', 'assistant-body');
    body.append(this.process, this.pruned, this.answer, this.answerTruncated, this.footer);
    const user = text('div', 'user-message', userText); user.dataset.conversationSelectable = 'user';
    const userActions = text('div', 'message-actions user-actions');
    userActions.append(messageCopyButton('复制消息', () => this.userText));
    if (userText.length > 900 || userText.split('\n').length > 10) {
      user.classList.add('is-collapsed');
      const expand = button('展开全文', () => { const collapsed = user.classList.toggle('is-collapsed'); expand.textContent = collapsed ? '展开全文' : '收起'; expand.setAttribute('aria-expanded', String(!collapsed)); }); expand.setAttribute('aria-expanded', 'false'); userActions.prepend(expand);
    }
    this.answer.dataset.conversationSelectable = 'assistant';
    this.answerActions = text('div', 'message-actions assistant-actions'); this.answerActions.hidden = true;
    this.answerActions.append(messageCopyButton('复制回答', () => this.copyAnswerText || ''));
    this.fileCards = text('div', 'conversation-file-references');
    this.timestamp = text('time', 'message-timestamp'); this.timestamp.hidden = true; this.answerActions.append(this.timestamp);
    body.append(this.fileCards, this.answerActions); this.turn.append(user, userActions, body);
    this.answer.addEventListener('mona:content-resized', () => this.refreshFileCards());
    this.process.addEventListener('toggle', event => { if (event.target === this.process) this.summary.setAttribute('aria-expanded', String(this.process.open)); });
    this.process.addEventListener('click', event => {
      if (event.target.closest('summary') !== this.summary) return;
      if (!this.foldable) { event.preventDefault(); return; }
      this.processUserToggled = true; this.processChoice = !this.process.open;
    });
  }

  setSupplemental(value = true) { this.hasSupplemental = value; if (this.lastState) this.applyDisplayPolicy(); }
  applyDisplayPolicy() {
    const state = this.lastState;
    this.foldable = canFoldTurn(state, this.hasSupplemental);
    this.process.open = !this.foldable || Boolean(this.processChoice);
    this.summary.classList.toggle('is-static', !this.foldable);
    this.turn.dataset.ending = state.outcome?.status || 'running';
    this.summary.setAttribute('aria-expanded', String(this.process.open));
    this.summary.setAttribute('aria-disabled', String(!this.foldable));
    for (const group of this.groups.values()) {
      presentGroup(group);
      group.node.open = Boolean(group.manualOpen);
      group.summary.hidden = false;
      group.summary.setAttribute('aria-expanded', String(group.node.open));
      group.scroll.mode(!group.summary.hidden, this.streaming);
    }
  }

  prepareSearch(query) {
    if (!query || this.disposed) return;
    const needle = query.toLocaleLowerCase();
    for (const entry of this.entries.values()) {
      if (!entry.latestTool) continue;
      const content = entry.latestTool.item.content;
      const values = [content.output, content.result?.content, ...(content.result?.blocks || []).filter(block => block.type === 'text').map(block => block.text)];
      if (values.some(value => typeof value === 'string' && value.toLocaleLowerCase().includes(needle))) updateToolPanel(entry);
    }
  }

  setTimestamp(value) {
    if (!Number.isFinite(value) || value <= 0) return;
    const date = new Date(value); this.timestamp.dateTime = date.toISOString(); this.timestamp.textContent = date.toLocaleString(undefined, { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }); this.timestamp.hidden = false;
  }
  refreshFileCards() {
    if (this.disposed || this.streaming || !this.presentation.describeFile) return;
    const paths = [...new Set([...(this.writtenPaths || []), ...[...this.answer.querySelectorAll('.message-file-link')].map(link => link.dataset.filePath)])]
      .filter(path => typeof path === 'string' && /\.(?:pdf|docx|xlsx|pptx|csv|md|txt|png|jpg|jpeg|webp|gif|html)(?:#L\d+(?:-L?\d+)?)?$/i.test(path)).slice(0, 8);
    const signature = JSON.stringify([this.presentation.contextKey, paths]); if (signature === this.cardSignature) return;
    this.cardSignature = signature; this.cardView?.dispose(); this.fileCards.replaceChildren();
    if (paths.length) { this.cardView = fileReferenceCards(paths, this.presentation); this.fileCards.append(this.cardView); }
  }
  bindPresentation(options) {
    const changed = this.presentation.contextKey !== options.contextKey; this.presentation = options;
    if (changed) { this.cardView?.dispose(); this.cardSignature = null; this.fileCards.replaceChildren(); }
    if (changed) { disposeMessage(this.answer); this.renderedAnswer = undefined; for (const entry of this.entries.values()) { entry.signature = null; entry.parts?.dispose?.(); entry.parts = null; } }
  }
  dispose() {
    this.disposed = true; if (this.answerRenderTimer) clearTimeout(this.answerRenderTimer); this.answerRenderTimer = 0;
    this.cardView?.dispose();
    disposeMessage(this.answer);
    for (const group of this.groups.values()) group.dispose(); for (const entry of this.entries.values()) { if (entry.messageText) disposeMessage(entry.messageText); entry.parts?.dispose?.(); }
    this.entries.clear(); this.groups.clear();
  }

  tick() {
    const status = this.lastState?.outcome?.status;
    const label = this.disconnected ? '连接中断 · 状态待确认' : this.startFailed ? '任务未启动' : status && status !== 'completed' ? outcomeText({ status }) : `${this.endedAt == null ? '正在工作' : '用时'} ${durationText((this.endedAt ?? performance.now()) - this.startedAt)}`;
    this.clock.textContent = label;
    this.summary.setAttribute('aria-label', label);
  }

  flushAnswer() {
    if (this.answerRenderTimer) clearTimeout(this.answerRenderTimer);
    this.answerRenderTimer = 0;
    if (this.disposed) return;
    if (this.pendingAnswer === this.renderedAnswer && this.pendingTruncated === this.renderedTruncated && this.renderedStreaming === this.streaming) return;
    this.answerReady = renderMessage(this.answer, this.pendingAnswer, { ...this.presentation, streaming: this.streaming });
    this.renderedStreaming = this.streaming;
    this.answerTruncated.hidden = !this.pendingTruncated;
    this.renderedAnswer = this.pendingAnswer;
    this.renderedTruncated = this.pendingTruncated;
    this.lastAnswerRenderAt = performance.now();
  }

  queueAnswer(answer, truncated, immediate = false) {
    this.pendingAnswer = answer;
    this.pendingTruncated = Boolean(truncated);
    const elapsed = performance.now() - this.lastAnswerRenderAt;
    if (immediate || !answer || this.renderedAnswer === undefined || elapsed >= 90) {
      this.flushAnswer();
      return;
    }
    if (!this.answerRenderTimer) this.answerRenderTimer = setTimeout(() => this.flushAnswer(), Math.max(1, 90 - elapsed));
  }

  connectionLost(message, reconnect) {
    this.disconnected = true; this.flushAnswer(); this.tick();
    this.footer.className = 'run-footer outcome-unknown'; this.footer.replaceChildren(text('span', '', message));
    const retry = button('重新连接', async () => {
      retry.disabled = true;
      try { await reconnect(); }
      catch (error) { this.footer.firstChild.textContent = `仍未确认运行状态：${error.message}`; }
      finally { if (!this.disposed) retry.disabled = false; }
    }); this.footer.append(retry);
  }

  fail(message) {
    this.streaming = false; this.flushAnswer();
    this.turn.classList.remove('is-live-turn');
    this.startFailed = true; this.processChoice = true;
    this.endedAt ??= performance.now();
    this.tick();
    this.process.classList.remove('is-running');
    this.process.open = true;
    this.footer.className = 'run-footer outcome-failed';
    this.footer.textContent = message;
  }

  update(state) {
    if (this.disposed) return;
    this.streaming = !state.outcome;
    this.lastState = state;
    this.disconnected = false; this.startFailed = false;
    this.turn.classList.toggle('is-live-turn', this.streaming);
    this.writtenPaths = state.items.filter(item => item.content.kind === 'tool_call' && ['write', 'edit'].includes(item.content.name) && item.state === 'completed').map(item => argumentsOf(item.content)?.path).filter(Boolean);
    const { process, answer, truncated } = partitionItems(state);
    this.lastProcess = process;
    this.copyAnswerText = answer;
    this.answerActions.hidden = !state.outcome || !this.copyAnswerText;
    const blocks = itemBlocks(process);
    // Prepending a historical prefix may extend a group; keep existing seats and disclosure state.
    const claimed = new Set();
    for (const block of blocks) if (block.key) {
      const previous = [...this.groups].find(([key, group]) => !claimed.has(key) && block.items.some(item => group.members?.has(item.id)));
      if (previous) block.key = previous[0];
      claimed.add(block.key);
    }
    const ids = new Set(process.map(item => item.id));
    for (const [id, entry] of this.entries) if (!ids.has(id)) { if (entry.messageText) disposeMessage(entry.messageText); entry.parts?.dispose?.(); entry.node.remove(); this.entries.delete(id); }
    const liveGroups = new Set(blocks.map(block => block.key).filter(Boolean));
    for (const [key, group] of this.groups) if (!liveGroups.has(key)) { group.dispose(); group.node.remove(); this.groups.delete(key); }
    const topLevel = [];
    for (const block of blocks) {
      const entries = block.items.map(item => {
        let entry = this.entries.get(item.id);
        if (!entry) { entry = createItemEntry(item); this.entries.set(item.id, entry); }
        updateItem(entry, item, state.run_id, this.artifactReader, this.presentation);
        return entry;
      });
      if (!block.key) { topLevel.push(entries[0].node); continue; }
      let group = this.groups.get(block.key);
      if (!group) {
        group = createGroupEntry();
        // A live step opens so streamed calls stay visible; a replayed finished run stays collapsed.
        group.manualOpen = false;
        group.summary.addEventListener('click', () => { group.manualOpen = !group.node.open; if (group.manualOpen) group.scroll.initialize(); });
        this.groups.set(block.key, group);
      }
      group.members = new Set(block.items.map(item => item.id)); group.items = block.items;
      const anchor = group.scroll.capture();
      syncChildren(group.content, entries.map(entry => entry.node)); group.scroll.updated(anchor);
      topLevel.push(group.node);
    }
    syncChildren(this.items, topLevel);
    this.process.hidden = state.outcome != null && !process.length;
    this.process.classList.toggle('is-running', !state.outcome);
    if (!state.outcome && process.length && !this.processAutoOpened) {
      this.process.open = true;
      this.processAutoOpened = true;
    }
    this.pruned.hidden = !state.pruned_items;
    this.pruned.textContent = `较早的 ${state.pruned_items} 个显示项已从视图裁剪。`;
    this.queueAnswer(answer, truncated, Boolean(state.outcome));
    this.answer.hidden = !answer;
    if (state.outcome) {
      if (!this.completed && !this.processUserToggled) this.process.open = false;
      this.completed = true;
      this.endedAt ??= performance.now();
      this.footer.className = `run-footer outcome-${state.outcome.status}`;
      this.footer.textContent = state.outcome.status === 'completed' ? '' : outcomeText(state.outcome);
      const errorText = outcomeErrorText(state.outcome.error);
      if (errorText) this.footer.append(text('span', 'error-detail', ` · ${errorText}`));
    } else this.footer.textContent = '';
    this.applyDisplayPolicy(); this.refreshFileCards(); this.tick();
  }
}
