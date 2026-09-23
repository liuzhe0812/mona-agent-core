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
  configuration: '模型配置有误，请检查供应商地址、模型名称和密钥。',
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

// Deliberately small Markdown subset. Every node is built through the DOM, so model output is never HTML.
const INLINE_PATTERN = /(`[^`\n]+`|\*\*[^*\n]+\*\*|~~[^~\n]+~~|\*[^*\n]+\*|\[[^\]\n]*\]\([^)\s]+\))/g;
const SAFE_LINK = /^(?:https?:|mailto:)/i;
const TABLE_DELIMITER = /^\s*\|?[\s:|-]+\|[\s:|-]*$/;

function appendInline(parent, value) {
  let end = 0;
  for (const match of value.matchAll(INLINE_PATTERN)) {
    parent.append(document.createTextNode(value.slice(end, match.index)));
    const token = match[0];
    if (token.startsWith('`')) parent.append(text('code', '', token.slice(1, -1)));
    else if (token.startsWith('**')) parent.append(text('strong', '', token.slice(2, -2)));
    else if (token.startsWith('~~')) parent.append(text('del', '', token.slice(2, -2)));
    else if (token.startsWith('*')) parent.append(text('em', '', token.slice(1, -1)));
    else {
      const [, label, href] = /^\[([^\]]*)\]\(([^)\s]+)\)$/.exec(token);
      if (!SAFE_LINK.test(href)) parent.append(document.createTextNode(token));
      else {
        const anchor = text('a', '', label || href);
        anchor.href = href;
        anchor.rel = 'noopener noreferrer';
        anchor.target = '_blank';
        parent.append(anchor);
      }
    }
    end = match.index + token.length;
  }
  parent.append(document.createTextNode(value.slice(end)));
}

function inlineBlock(tag, content) {
  const element = text(tag, '');
  appendInline(element, content);
  return element;
}

function splitRow(line) {
  return line.trim().replace(/^\|/, '').replace(/\|$/, '').split('|').map(cell => cell.trim());
}

function tableAlignments(delimiter) {
  return splitRow(delimiter).map(cell => {
    const left = cell.startsWith(':');
    const right = cell.endsWith(':');
    return left && right ? 'center' : right ? 'right' : left ? 'left' : '';
  });
}

function tableBlock(head, rows, alignments) {
  const wrap = text('div', 'message-table');
  const table = document.createElement('table');
  const headRow = document.createElement('tr');
  head.forEach((cell, column) => {
    const th = inlineBlock('th', cell);
    if (alignments[column]) th.className = `align-${alignments[column]}`;
    headRow.append(th);
  });
  const thead = document.createElement('thead');
  thead.append(headRow);
  table.append(thead);
  if (rows.length) {
    const tbody = document.createElement('tbody');
    for (const row of rows) {
      const tr = document.createElement('tr');
      head.forEach((_, column) => {
        const td = inlineBlock('td', row[column] ?? '');
        if (alignments[column]) td.className = `align-${alignments[column]}`;
        tr.append(td);
      });
      tbody.append(tr);
    }
    table.append(tbody);
  }
  wrap.append(table);
  return wrap;
}

export function renderMessage(node, value) {
  node.replaceChildren();
  const lines = String(value ?? '').split('\n');
  const lists = [];
  let paragraph = null, quote = null, quoteParagraph = null;
  const reset = () => { paragraph = null; quote = null; quoteParagraph = null; lists.length = 0; };

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    if (/^\s*(?:```|~~~)/.test(line)) {
      const body = [];
      index += 1;
      while (index < lines.length && !/^\s*(?:```|~~~)\s*$/.test(lines[index])) { body.push(lines[index]); index += 1; }
      const pre = text('pre', 'message-code');
      pre.append(text('code', '', body.length ? `${body.join('\n')}\n` : ''));
      node.append(pre);
      reset();
      continue;
    }
    // A blank line ends the open paragraph and quote but keeps a list going, so loose lists stay one list.
    if (!line.trim()) { paragraph = null; quote = null; quoteParagraph = null; continue; }

    const quoted = /^\s*>\s?(.*)$/.exec(line);
    if (quoted) {
      if (!quote) { quote = text('blockquote', ''); quoteParagraph = null; lists.length = 0; node.append(quote); }
      if (quoted[1].trim()) {
        if (!quoteParagraph) { quoteParagraph = document.createElement('p'); quote.append(quoteParagraph); }
        else quoteParagraph.append(document.createElement('br'));
        appendInline(quoteParagraph, quoted[1]);
      } else quoteParagraph = null;
      paragraph = null;
      continue;
    }

    const heading = /^(#{1,6})\s+(.*?)(?:\s+#+)?\s*$/.exec(line);
    if (heading) { node.append(inlineBlock(`h${heading[1].length}`, heading[2])); reset(); continue; }

    const underline = /^\s*(=+|-+)\s*$/.exec(line);
    if (paragraph && underline && !paragraph.querySelector('br')) {
      paragraph.replaceWith(inlineBlock(underline[1][0] === '=' ? 'h1' : 'h2', paragraph.textContent));
      reset();
      continue;
    }
    if (/^\s*(?:-{3,}|\*{3,}|_{3,})\s*$/.test(line)) { node.append(text('hr', 'message-rule')); reset(); continue; }

    const item = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/.exec(line);
    if (item) {
      const indent = item[1].replace(/\t/g, '  ').length;
      const tag = /\d/.test(item[2]) ? 'ol' : 'ul';
      while (lists.length && lists.at(-1).indent > indent) lists.pop();
      let level = lists.at(-1);
      if (!level || (level.indent < indent && level.item)) {
        const nested = document.createElement(tag);
        if (level) level.item.append(nested);
        else node.append(nested);
        level = { node: nested, indent, tag, item: null };
        lists.push(level);
      } else if (level.tag !== tag) {
        const sibling = document.createElement(tag);
        level.node.after(sibling);
        level = { node: sibling, indent, tag, item: null };
        lists[lists.length - 1] = level;
      }
      if (tag === 'ol' && !level.node.childElementCount) {
        const start = Number.parseInt(item[2], 10);
        if (start > 1) level.node.start = start;
      }
      const li = document.createElement('li');
      appendInline(li, item[3]);
      level.node.append(li);
      level.item = li;
      paragraph = null; quote = null; quoteParagraph = null;
      continue;
    }
    // Continuation lines of an indented list item belong to that item.
    const indent = /^\s*/.exec(line)[0].replace(/\t/g, '  ').length;
    if (lists.length && indent > lists.at(-1).indent && lists.at(-1).item) {
      lists.at(-1).item.append(document.createElement('br'));
      appendInline(lists.at(-1).item, line.trim());
      continue;
    }

    if (line.includes('|') && index + 1 < lines.length && TABLE_DELIMITER.test(lines[index + 1]) && lines[index + 1].includes('-')) {
      const alignments = tableAlignments(lines[index + 1]);
      const head = splitRow(line);
      index += 2;
      const rows = [];
      while (index < lines.length && lines[index].trim() && lines[index].includes('|')) { rows.push(splitRow(lines[index])); index += 1; }
      index -= 1;
      node.append(tableBlock(head, rows, alignments));
      reset();
      continue;
    }

    lists.length = 0; quote = null; quoteParagraph = null;
    if (!paragraph) { paragraph = document.createElement('p'); node.append(paragraph); }
    else paragraph.append(document.createElement('br'));
    appendInline(paragraph, line);
  }
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

// Presentation only: rows stay one line tall and a model step that issues several
// tool calls becomes one nested group, mirroring the reference execution process.
// Rows are labelled with the tool category, as in the reference process list.
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
const STATE_PRIORITY = ['failed', 'denied', 'unknown', 'cancelled', 'running', 'pending', 'completed'];
const KIND_PATTERNS = [
  ['shell', /exec|shell|terminal|command|bash|powershell/i],
  ['search', /search|grep|find|glob/i],
  ['read', /read|list|cat|open|view/i],
  ['write', /write|edit|patch|create|apply/i],
];

function toolKind(name) {
  return KIND_PATTERNS.find(([, pattern]) => pattern.test(String(name || '')))?.[0] || 'tool';
}

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

// Streamed arguments are partial JSON; the first known field is still worth showing.
function previewOf(content) {
  const args = argumentsOf(content);
  const value = args ? args.command ?? args.cmd ?? args.path ?? args.file_path ?? args.query ?? args.pattern : '';
  if (typeof value === 'string' && value) return value;
  const partial = /"(?:command|cmd|path|file_path|query|pattern)"\s*:\s*"([^"]*)/.exec(content.arguments_text || '');
  return partial ? partial[1].replace(/\\n/g, ' ').replace(/\\"/g, '"') : '';
}

function commandText(content, kind) {
  if (kind !== 'shell') return content.arguments_text || (content.arguments != null ? JSON.stringify(content.arguments, null, 2) : '');
  const args = argumentsOf(content);
  const command = args ? args.command ?? args.cmd : null;
  return typeof command === 'string' && command ? command : previewOf(content);
}

// Same-step tool items belong to one model call and read as one collapsed block.
export function itemBlocks(process) {
  const blocks = [];
  for (const item of process) {
    const key = item.content.kind === 'tool_call' && Number.isInteger(item.step) ? `step-${item.step}` : null;
    const last = blocks.at(-1);
    if (key && last?.key === key) last.items.push(item);
    else blocks.push({ key, items: [item] });
  }
  return blocks.map(block => (block.items.length > 1 ? block : { key: null, items: block.items }));
}

function groupState(items) {
  const seen = new Set(items.map(item => item.state));
  return STATE_PRIORITY.find(state => seen.has(state)) || 'unknown';
}

function groupTitle(items) {
  const kinds = items.map(item => toolKind(item.content.name));
  const uniform = kinds.every(kind => kind === kinds[0]) ? kinds[0] : 'tool';
  return ROW_LABELS[uniform] || '工具';
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
  if (content.kind !== 'tool_call') return Boolean(content.text);
  return Boolean(content.arguments_text || content.arguments != null || content.output || content.result);
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
    count: text('span', 'activity-group-count'),
    state: text('span', 'activity-group-state'),
    body: text('div', 'activity-group-body'),
  };
  group.icon.setAttribute('aria-hidden', 'true');
  summary.append(group.icon, group.name, group.count, group.state, text('span', 'chevron', '›'));
  node.append(summary, group.body);
  return group;
}

// Expanded tool detail: tool name, command or arguments, log, bounded result and the outcome mark.
function artifactBlock(artifact, runId, readArtifact) {
  const block = text('div', 'activity-panel-artifact');
  if (!readArtifact || !String(artifact.uri || '').startsWith('spill:')) {
    block.append(text('p', 'muted-note', `完整结果位置：${artifact.uri}`));
    return block;
  }
  const output = text('pre', 'activity-panel-output', ''); output.hidden = true;
  const note = text('p', 'muted-note', `完整结果 ${artifact.bytes} 字节，保存在宿主私有归档中。`);
  const button = text('button', 'text-button', '读取完整结果'); button.type = 'button';
  let offset = 0;
  button.addEventListener('click', async () => {
    button.disabled = true; note.textContent = '正在读取完整结果…';
    try {
      const page = await readArtifact(runId, artifact.uri, offset);
      output.hidden = false; output.append(document.createTextNode(page.text));
      offset = page.next_offset;
      button.textContent = page.eof ? '已读取完整结果' : '读取下一页';
      button.disabled = page.eof;
      note.textContent = `已读取 ${offset} / ${page.total_bytes} 字节。`;
    } catch (error) {
      button.disabled = false; note.textContent = error?.message || '读取完整结果失败。';
    }
  });
  block.append(note, button, output);
  return block;
}

function toolPanel(item, runId, readArtifact) {
  const content = item.content;
  const kind = toolKind(content.name);
  const clean = value => (kind === 'shell' && typeof value === 'string' ? value.replace(ANSI_SEQUENCE, '') : value);
  const output = clean(content.output) || '';
  const panel = text('div', 'activity-panel');
  const command = commandText(content, kind);
  if (command) {
    const code = text('div', 'activity-panel-code');
    if (kind === 'shell') code.append(text('span', 'activity-panel-prompt', '$ '));
    code.append(document.createTextNode(command));
    panel.append(code);
  }
  if (content.arguments_truncated) panel.append(text('p', 'muted-note', '参数过长，已截断显示。'));
  if (output) panel.append(text('pre', 'activity-panel-output', output));
  if (content.output_truncated) panel.append(text('p', 'muted-note', '执行输出过长，已截断显示。'));
  if (content.result) {
    const resultText = clean(content.result.content) || '';
    // The bounded result and the log preview are separate protocol fields, but repeating one text twice hides nothing.
    if (resultText.trim() && resultText.trim() !== output.trim()) panel.append(text('pre', 'activity-panel-result', resultText));
    if (content.result.truncated) panel.append(text('p', 'muted-note', '结果过长，已截断显示。'));
    if (content.result.artifact) panel.append(artifactBlock(content.result.artifact, runId, readArtifact));
  }
  if (!panel.childElementCount) panel.append(text('p', 'muted-note', '尚无执行结果。'));
  return panel;
}

function messagePanel(item) {
  const panel = text('div', 'activity-panel');
  const body = text('div', 'message-text');
  renderMessage(body, item.content.text || '等待回复…');
  panel.append(body);
  if (item.content.truncated) panel.append(text('p', 'muted-note', '消息过长，已截断显示。'));
  return panel;
}

function updateItem(entry, item, runId, readArtifact) {
  const content = item.content, tool = content.kind === 'tool_call';
  const kind = tool ? toolKind(content.name) : 'message';
  const preview = tool ? previewOf(content) : previewLine(content.text);
  entry.node.className = `activity-item state-${item.state} kind-${kind}`;
  entry.icon.className = `activity-icon icon-${kind}`;
  if (entry.iconKind !== kind) { entry.icon.replaceChildren(iconNode(kind)); entry.iconKind = kind; }
  entry.name.textContent = tool ? ROW_LABELS[kind] || content.name || '工具' : '模型回复';
  entry.preview.textContent = preview;
  entry.preview.hidden = !preview;
  entry.preview.classList.toggle('is-command', tool);
  // Completed rows stay silent like the reference; every other state must remain visible.
  entry.state.textContent = item.state === 'completed' ? '' : statusText(item.state);
  entry.state.hidden = !entry.state.textContent;
  entry.summary.dataset.tooltip = preview || (tool ? content.name || '工具' : '展开模型回复');
  entry.summary.classList.toggle('is-static', !hasDetail(item));
  const signature = JSON.stringify(content);
  if (signature === entry.signature) return;
  entry.signature = signature;
  entry.body.replaceChildren(tool ? toolPanel(item, runId, readArtifact) : messagePanel(item));
}

export class TurnView {
  constructor(userText, { readArtifact } = {}) {
    this.startedAt = performance.now();
    this.endedAt = null;
    this.entries = new Map();
    this.groups = new Map();
    this.processAutoOpened = false;
    this.readArtifact = readArtifact;
    this.turn = text('article', 'turn');
    this.process = text('details', 'execution-process');
    this.summary = text('summary', 'execution-summary');
    this.clock = text('span', 'execution-clock', '正在启动…');
    this.clock.dataset.tooltip = '本页面从发起请求到收到结束状态的用时';
    this.summary.append(this.clock, text('span', 'chevron', '›'));
    this.items = text('div', 'activity-list');
    this.process.append(this.summary, this.items);
    this.answer = text('div', 'message-text');
    this.footer = text('div', 'run-footer');
    this.footer.setAttribute('role', 'status');
    this.pruned = text('p', 'muted-note');
    this.pruned.hidden = true;
    const body = text('div', 'assistant-body');
    body.append(this.process, this.pruned, this.answer, this.footer);
    this.turn.append(text('div', 'user-message', userText), body);
  }

  tick() {
    this.clock.textContent = `${this.endedAt == null ? '正在工作' : '已工作'} ${durationText((this.endedAt ?? performance.now()) - this.startedAt)}`;
  }

  fail(message) {
    this.endedAt ??= performance.now();
    this.tick();
    this.process.classList.remove('is-running');
    this.process.open = false;
    this.footer.className = 'run-footer outcome-failed';
    this.footer.textContent = message;
  }

  update(state) {
    const { process, answer, truncated } = partitionItems(state);
    const blocks = itemBlocks(process);
    const ids = new Set(process.map(item => item.id));
    for (const [id, entry] of this.entries) if (!ids.has(id)) { entry.node.remove(); this.entries.delete(id); }
    const liveGroups = new Set(blocks.map(block => block.key).filter(Boolean));
    for (const [key, group] of this.groups) if (!liveGroups.has(key)) { group.node.remove(); this.groups.delete(key); }
    const topLevel = [];
    for (const block of blocks) {
      const entries = block.items.map(item => {
        let entry = this.entries.get(item.id);
        if (!entry) { entry = createItemEntry(item); this.entries.set(item.id, entry); }
        updateItem(entry, item, state.run_id, this.readArtifact);
        return entry;
      });
      if (!block.key) { topLevel.push(entries[0].node); continue; }
      let group = this.groups.get(block.key);
      if (!group) {
        group = createGroupEntry();
        // A live step opens so streamed calls stay visible; a replayed finished run stays collapsed.
        group.node.open = !state.outcome;
        this.groups.set(block.key, group);
      }
      const status = groupState(block.items);
      const kind = toolKind(block.items[0].content.name);
      const uniform = block.items.every(item => toolKind(item.content.name) === kind) ? kind : 'tool';
      group.node.className = `activity-group state-${status}`;
      group.icon.className = `activity-icon icon-${uniform}`;
      if (group.iconKind !== uniform) { group.icon.replaceChildren(iconNode(uniform)); group.iconKind = uniform; }
      group.name.textContent = groupTitle(block.items);
      group.count.textContent = String(block.items.length);
      group.state.textContent = status === 'completed' ? '' : statusText(status);
      group.state.hidden = !group.state.textContent;
      syncChildren(group.body, entries.map(entry => entry.node));
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
    if (answer !== this.lastAnswer || truncated !== this.lastTruncated) {
      renderMessage(this.answer, answer);
      if (truncated) this.answer.append(text('p', 'muted-note', '消息过长，已截断显示。'));
      this.lastAnswer = answer; this.lastTruncated = truncated;
    }
    this.answer.hidden = !answer;
    if (state.outcome) {
      this.process.open = false;
      this.endedAt ??= performance.now();
      this.footer.className = `run-footer outcome-${state.outcome.status}`;
      this.footer.textContent = state.outcome.status === 'completed' ? '' : outcomeText(state.outcome);
      const errorText = outcomeErrorText(state.outcome.error);
      if (errorText) this.footer.append(text('span', 'error-detail', ` · ${errorText}`));
    } else this.footer.textContent = '';
    this.tick();
  }
}
