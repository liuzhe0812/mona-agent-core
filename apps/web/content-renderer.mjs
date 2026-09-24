// Safe conversation content renderer. Model/tool text is never interpreted as HTML or script.
const MATH_NS = 'http://www.w3.org/1998/Math/MathML';
const SVG_NS = 'http://www.w3.org/2000/svg';
const SAFE_LINK = /^(?:https?:|mailto:)/i;
const IMAGE_TYPES = new Set(['image/png', 'image/jpeg', 'image/webp', 'image/gif']);
const ESCAPABLE = new Set('\\`*_[\]{}()#+-.!|~$<>'.split(''));

function el(tag, className = '', value) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (value !== undefined) node.textContent = value;
  return node;
}

function svg(tag, className = '') {
  const node = document.createElementNS(SVG_NS, tag);
  if (className) node.setAttribute('class', className);
  return node;
}

function appendTextWithUrls(parent, value) {
  const pattern = /https?:\/\/[^\s<>()]+/gi;
  let end = 0;
  for (const match of value.matchAll(pattern)) {
    parent.append(document.createTextNode(value.slice(end, match.index)));
    let href = match[0], suffix = '';
    while (/[.,;:!?]$/.test(href)) { suffix = href.at(-1) + suffix; href = href.slice(0, -1); }
    const anchor = el('a', '', href);
    anchor.href = href; anchor.target = '_blank'; anchor.rel = 'noopener noreferrer';
    parent.append(anchor, document.createTextNode(suffix));
    end = match.index + match[0].length;
  }
  parent.append(document.createTextNode(value.slice(end)));
}

function unescapedIndex(value, marker, start) {
  let index = value.indexOf(marker, start);
  while (index >= 0) {
    let slashes = 0;
    for (let at = index - 1; at >= 0 && value[at] === '\\'; at -= 1) slashes += 1;
    if (slashes % 2 === 0) return index;
    index = value.indexOf(marker, index + marker.length);
  }
  return -1;
}

function linkToken(value, at) {
  if (value[at] !== '[') return null;
  const close = unescapedIndex(value, ']', at + 1);
  if (close < 0 || value[close + 1] !== '(') return null;
  const end = unescapedIndex(value, ')', close + 2);
  if (end < 0) return null;
  const href = value.slice(close + 2, end).trim();
  if (!href || /\s/.test(href)) return null;
  return { label: value.slice(at + 1, close), href, end: end + 1 };
}

export function appendInline(parent, value) {
  value = String(value ?? '');
  let index = 0, plain = 0;
  const flush = end => { if (end > plain) appendTextWithUrls(parent, value.slice(plain, end)); };
  while (index < value.length) {
    if (value[index] === '\\' && index + 1 < value.length && ESCAPABLE.has(value[index + 1])) {
      flush(index); parent.append(document.createTextNode(value[index + 1])); index += 2; plain = index; continue;
    }
    if (value[index] === '`') {
      const end = unescapedIndex(value, '`', index + 1);
      if (end > index + 1) {
        flush(index); parent.append(el('code', '', value.slice(index + 1, end))); index = end + 1; plain = index; continue;
      }
    }
    if (value.startsWith('**', index)) {
      const end = unescapedIndex(value, '**', index + 2);
      if (end > index + 2) {
        flush(index); const strong = el('strong'); appendInline(strong, value.slice(index + 2, end)); parent.append(strong);
        index = end + 2; plain = index; continue;
      }
    }
    if (value.startsWith('~~', index)) {
      const end = unescapedIndex(value, '~~', index + 2);
      if (end > index + 2) {
        flush(index); const del = el('del'); appendInline(del, value.slice(index + 2, end)); parent.append(del);
        index = end + 2; plain = index; continue;
      }
    }
    if (value[index] === '*' || value[index] === '_') {
      const marker = value[index], end = unescapedIndex(value, marker, index + 1);
      if (end > index + 1) {
        flush(index); const em = el('em'); appendInline(em, value.slice(index + 1, end)); parent.append(em);
        index = end + 1; plain = index; continue;
      }
    }
    if (value[index] === '[') {
      const token = linkToken(value, index);
      if (token) {
        flush(index);
        if (!SAFE_LINK.test(token.href)) appendTextWithUrls(parent, value.slice(index, token.end));
        else {
          const anchor = el('a'); appendInline(anchor, token.label || token.href);
          anchor.href = token.href; anchor.target = '_blank'; anchor.rel = 'noopener noreferrer'; parent.append(anchor);
        }
        index = token.end; plain = index; continue;
      }
    }
    if (value[index] === '<') {
      const end = value.indexOf('>', index + 1);
      const href = end > 0 ? value.slice(index + 1, end) : '';
      if (end > index + 1 && SAFE_LINK.test(href) && !/\s/.test(href)) {
        flush(index); const anchor = el('a', '', href); anchor.href = href; anchor.target = '_blank'; anchor.rel = 'noopener noreferrer';
        parent.append(anchor); index = end + 1; plain = index; continue;
      }
    }
    if (value[index] === '$' && value[index + 1] !== '$') {
      const end = unescapedIndex(value, '$', index + 1);
      if (end > index + 1 && !value.slice(index + 1, end).includes('\n')) {
        flush(index); parent.append(renderMath(value.slice(index + 1, end), false));
        index = end + 1; plain = index; continue;
      }
    }
    index += 1;
  }
  flush(value.length);
}

function splitTableRow(line) {
  const source = line.trim().replace(/^\|/, '').replace(/\|$/, '');
  const cells = []; let current = '';
  for (let index = 0; index < source.length; index += 1) {
    if (source[index] === '\\' && source[index + 1] === '|') { current += '|'; index += 1; continue; }
    if (source[index] === '|') { cells.push(current.trim()); current = ''; continue; }
    current += source[index];
  }
  cells.push(current.trim());
  return cells;
}

function tableDelimiter(line) {
  const cells = splitTableRow(line);
  return cells.length > 1 && cells.every(cell => /^:?-{3,}:?$/.test(cell.replace(/\s/g, '')));
}

function tableAlignments(line) {
  return splitTableRow(line).map(cell => {
    const normalized = cell.replace(/\s/g, '');
    const left = normalized.startsWith(':'), right = normalized.endsWith(':');
    return left && right ? 'center' : right ? 'right' : left ? 'left' : '';
  });
}

function inlineBlock(tag, content) {
  const block = el(tag); appendInline(block, content); return block;
}

function tableBlock(head, rows, alignments) {
  const wrap = el('div', 'message-table'), table = document.createElement('table');
  const headRow = document.createElement('tr');
  head.forEach((cell, column) => {
    const th = inlineBlock('th', cell);
    if (alignments[column]) th.className = `align-${alignments[column]}`;
    headRow.append(th);
  });
  const thead = document.createElement('thead'); thead.append(headRow); table.append(thead);
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
  wrap.append(table); return wrap;
}

const KEYWORDS = {
  javascript: new Set('break case catch class const continue debugger default delete do else export extends finally for function if import in instanceof let new return static super switch this throw try typeof var void while with yield async await of true false null undefined'.split(' ')),
  typescript: new Set('abstract any as assert asserts bigint boolean break case catch class const constructor continue declare default delete do else enum export extends false finally for from function get if implements import in infer instanceof interface is keyof let module namespace never new null number object of override private protected public readonly require return satisfies set static string super switch symbol this throw true try type typeof undefined unknown var void while with yield async await'.split(' ')),
  rust: new Set('as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while'.split(' ')),
  python: new Set('and as assert async await break class continue def del elif else except False finally for from global if import in is lambda None nonlocal not or pass raise return True try while with yield'.split(' ')),
  shell: new Set('case do done elif else esac fi for function if in then until while select time coproc'.split(' ')),
  powershell: new Set('begin break catch class continue data define do dynamicparam else elseif end enum exit filter finally for foreach from function hidden if in inlineScript parallel param process return sequence switch throw trap try until using var while workflow'.toLowerCase().split(' ')),
};
const LANG_ALIASES = { js:'javascript', jsx:'javascript', mjs:'javascript', cjs:'javascript', ts:'typescript', tsx:'typescript', py:'python', rs:'rust', sh:'shell', bash:'shell', zsh:'shell', ps1:'powershell', pwsh:'powershell', json:'json', html:'markup', xml:'markup', svg:'markup', css:'css', diff:'diff', patch:'diff' };
function normalizeLanguage(value) {
  const raw = String(value || '').toLowerCase().replace(/^language-/, '');
  return LANG_ALIASES[raw] || raw || 'text';
}

function tokenSpan(kind, value) { return el('span', `token-${kind}`, value); }
function highlightDiff(codeNode, code) {
  const lines = code.split('\n');
  lines.forEach((line, index) => {
    const kind = line.startsWith('+++') || line.startsWith('---') || line.startsWith('@@') ? 'meta'
      : line.startsWith('+') ? 'add' : line.startsWith('-') ? 'delete' : '';
    codeNode.append(kind ? tokenSpan(kind, line) : document.createTextNode(line));
    if (index < lines.length - 1) codeNode.append(document.createTextNode('\n'));
  });
}

function highlightMarkup(codeNode, code) {
  const pattern = /<!--[\s\S]*?-->|<\/?[A-Za-z][^>]*>/g;
  let end = 0;
  for (const match of code.matchAll(pattern)) {
    codeNode.append(document.createTextNode(code.slice(end, match.index)));
    codeNode.append(tokenSpan(match[0].startsWith('<!--') ? 'comment' : 'keyword', match[0]));
    end = match.index + match[0].length;
  }
  codeNode.append(document.createTextNode(code.slice(end)));
}

function highlightGeneric(codeNode, code, language) {
  if (language === 'diff') { highlightDiff(codeNode, code); return; }
  if (language === 'markup') { highlightMarkup(codeNode, code); return; }
  const hashComments = language === 'python' || language === 'shell' || language === 'powershell';
  const pattern = hashComments
    ? /#[^\n]*|\/\*[\s\S]*?\*\/|\/\/[^\n]*|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`|\b\d+(?:\.\d+)?\b|\b[A-Za-z_$][\w$]*\b/g
    : /\/\*[\s\S]*?\*\/|\/\/[^\n]*|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`|\b\d+(?:\.\d+)?\b|\b[A-Za-z_$][\w$]*\b/g;
  const keywords = KEYWORDS[language] || new Set();
  let end = 0;
  for (const match of code.matchAll(pattern)) {
    codeNode.append(document.createTextNode(code.slice(end, match.index)));
    const token = match[0];
    let kind = '';
    if (token.startsWith('//') || token.startsWith('/*') || (hashComments && token.startsWith('#'))) kind = 'comment';
    else if (/^["'`]/.test(token)) kind = 'string';
    else if (/^\d/.test(token)) kind = 'number';
    else if (keywords.has(language === 'powershell' ? token.toLowerCase() : token)) kind = 'keyword';
    codeNode.append(kind ? tokenSpan(kind, token) : document.createTextNode(token));
    end = match.index + token.length;
  }
  codeNode.append(document.createTextNode(code.slice(end)));
}

async function copyText(value, button) {
  try {
    if (navigator.clipboard?.writeText) await navigator.clipboard.writeText(value);
    else {
      const area = el('textarea'); area.value = value; area.setAttribute('readonly', ''); area.className = 'copy-fallback';
      document.body.append(area); area.select();
      if (!document.execCommand?.('copy')) throw new Error('copy unsupported');
      area.remove();
    }
    const before = button.textContent; button.textContent = '已复制';
    setTimeout(() => { if (button.isConnected) button.textContent = before; }, 1200);
  } catch {
    button.textContent = '复制失败';
  }
}

export function codeBlock(code, language = '') {
  const normalized = normalizeLanguage(language);
  const block = el('div', 'message-code-block');
  const head = el('div', 'message-code-head');
  const label = el('span', 'message-code-language', normalized === 'text' ? '代码' : normalized);
  const actions = el('div', 'message-code-actions');
  const copy = el('button', 'text-button code-copy', '复制'); copy.type = 'button';
  copy.addEventListener('click', () => { void copyText(code, copy); });
  actions.append(copy); head.append(label, actions);
  const pre = el('pre', 'message-code'), codeNode = el('code');
  highlightGeneric(codeNode, code, normalized); pre.append(codeNode);
  if (code.split('\n').length > 18 || code.length > 2400) {
    block.classList.add('is-collapsed');
    const expand = el('button', 'text-button code-expand', '展开'); expand.type = 'button';
    expand.addEventListener('click', () => {
      const collapsed = block.classList.toggle('is-collapsed');
      expand.textContent = collapsed ? '展开' : '收起';
    });
    actions.prepend(expand);
  }
  block.append(head, pre); return block;
}

const GREEK = { alpha:'α', beta:'β', gamma:'γ', delta:'δ', epsilon:'ε', zeta:'ζ', eta:'η', theta:'θ', iota:'ι', kappa:'κ', lambda:'λ', mu:'μ', nu:'ν', xi:'ξ', pi:'π', rho:'ρ', sigma:'σ', tau:'τ', phi:'φ', chi:'χ', psi:'ψ', omega:'ω', Gamma:'Γ', Delta:'Δ', Theta:'Θ', Lambda:'Λ', Xi:'Ξ', Pi:'Π', Sigma:'Σ', Phi:'Φ', Psi:'Ψ', Omega:'Ω' };
const MATH_OPS = { times:'×', cdot:'·', div:'÷', pm:'±', mp:'∓', le:'≤', leq:'≤', ge:'≥', geq:'≥', ne:'≠', neq:'≠', approx:'≈', sim:'∼', in:'∈', notin:'∉', to:'→', rightarrow:'→', leftarrow:'←', leftrightarrow:'↔', infinity:'∞', infty:'∞', partial:'∂', nabla:'∇', sum:'∑', prod:'∏', int:'∫' };

function mathNode(tag, value) {
  const node = document.createElementNS(MATH_NS, tag);
  if (value !== undefined) node.textContent = value;
  return node;
}
class MathParser {
  constructor(source) { this.source = source; this.index = 0; }
  skip() { while (/\s/.test(this.source[this.index] || '')) this.index += 1; }
  command() {
    this.index += 1;
    const match = /^[A-Za-z]+/.exec(this.source.slice(this.index));
    if (!match) return this.source[this.index++] || '';
    this.index += match[0].length; return match[0];
  }
  group() {
    this.skip();
    if (this.source[this.index] !== '{') return this.atom();
    this.index += 1; const row = this.row('}'); if (this.source[this.index] === '}') this.index += 1; return row;
  }
  textGroup() {
    this.skip(); if (this.source[this.index] !== '{') return '';
    let depth = 0, output = '';
    for (this.index += 1; this.index < this.source.length; this.index += 1) {
      const char = this.source[this.index];
      if (char === '{') { depth += 1; output += char; }
      else if (char === '}' && depth > 0) { depth -= 1; output += char; }
      else if (char === '}') { this.index += 1; break; }
      else output += char;
    }
    return output;
  }
  atom() {
    this.skip();
    const char = this.source[this.index];
    if (!char) return mathNode('mrow');
    let base;
    if (char === '{') base = this.group();
    else if (char === '\\') {
      const name = this.command();
      if (name === 'frac') { const node = mathNode('mfrac'); node.append(this.group(), this.group()); base = node; }
      else if (name === 'sqrt') { const node = mathNode('msqrt'); node.append(this.group()); base = node; }
      else if (name === 'text' || name === 'mathrm' || name === 'operatorname') base = mathNode('mtext', this.textGroup());
      else if (name === 'left' || name === 'right') base = this.atom();
      else if (GREEK[name]) base = mathNode('mi', GREEK[name]);
      else if (MATH_OPS[name]) base = mathNode('mo', MATH_OPS[name]);
      else base = mathNode('mtext', `\\${name}`);
    } else if (/\d/.test(char)) {
      const match = /^\d+(?:\.\d+)?/.exec(this.source.slice(this.index)); this.index += match[0].length; base = mathNode('mn', match[0]);
    } else if (/[A-Za-z]/.test(char)) {
      const match = /^[A-Za-z]+/.exec(this.source.slice(this.index)); this.index += match[0].length; base = mathNode('mi', match[0]);
    } else {
      this.index += 1; base = mathNode(/[+\-=<>()[\],]/.test(char) ? 'mo' : 'mi', char);
    }
    let sub = null, sup = null;
    for (;;) {
      this.skip(); const script = this.source[this.index];
      if (script !== '^' && script !== '_') break;
      this.index += 1; const value = this.group();
      if (script === '^') sup = value; else sub = value;
    }
    if (sub && sup) { const node = mathNode('msubsup'); node.append(base, sub, sup); return node; }
    if (sub) { const node = mathNode('msub'); node.append(base, sub); return node; }
    if (sup) { const node = mathNode('msup'); node.append(base, sup); return node; }
    return base;
  }
  row(stop = '') {
    const row = mathNode('mrow');
    while (this.index < this.source.length && (!stop || this.source[this.index] !== stop)) row.append(this.atom());
    return row;
  }
}
export function renderMath(source, display = false) {
  const math = mathNode('math'); math.classList.add('message-math');
  math.setAttribute('display', display ? 'block' : 'inline');
  math.setAttribute('aria-label', String(source || ''));
  try { math.append(new MathParser(String(source || '')).row()); }
  catch { math.append(mathNode('mtext', String(source || ''))); }
  return math;
}

function parseMermaidNode(token) {
  token = token.trim();
  const match = /^([A-Za-z0-9_.-]+)(?:\[([^\]]*)\]|\(([^)]*)\)|\{([^}]*)\})?$/.exec(token);
  if (!match) return null;
  return { id: match[1], label: match[2] ?? match[3] ?? match[4] ?? match[1], shape: match[4] != null ? 'diamond' : match[3] != null ? 'round' : 'box' };
}
function mermaidEdge(line) {
  const patterns = [
    /^(.+?)\s*--\>\|([^|]+)\|\s*(.+)$/,
    /^(.+?)\s*--\s*([^>-]+?)\s*--\>\s*(.+)$/,
    /^(.+?)\s*(-->|==>|-\.->|---)\s*(.+)$/,
  ];
  for (const [index, pattern] of patterns.entries()) {
    const match = pattern.exec(line);
    if (!match) continue;
    if (index < 2) return { from: parseMermaidNode(match[1]), to: parseMermaidNode(match[3]), label: match[2].trim() };
    return { from: parseMermaidNode(match[1]), to: parseMermaidNode(match[3]), label: '' };
  }
  return null;
}
export function renderMermaid(source) {
  if (String(source || '').length > 64 * 1024) {
    const fallback = codeBlock(source, 'mermaid');
    fallback.prepend(el('p', 'muted-note', 'Mermaid 内容过长，已按源码显示。'));
    return fallback;
  }
  const lines = String(source || '').split(/\r?\n/).map(line => line.trim()).filter(line => line && !line.startsWith('%%'));
  const header = /^(?:flowchart|graph)\s+(TD|TB|BT|LR|RL)\b/i.exec(lines[0] || '');
  if (!header) {
    const fallback = codeBlock(source, 'mermaid'); fallback.classList.add('message-diagram-unsupported');
    fallback.prepend(el('p', 'muted-note', '当前内置 Mermaid 渲染器支持 flowchart / graph。')); return fallback;
  }
  const direction = header[1].toUpperCase();
  const nodes = new Map(), edges = [];
  const remember = item => { if (item && !nodes.has(item.id)) nodes.set(item.id, item); else if (item?.label && nodes.get(item.id)?.label === item.id) nodes.set(item.id, item); };
  for (const raw of lines.slice(1, 401)) {
    for (const part of raw.split(';').map(value => value.trim()).filter(Boolean)) {
      const edge = mermaidEdge(part);
      if (edge?.from && edge?.to) { remember(edge.from); remember(edge.to); edges.push({ from: edge.from.id, to: edge.to.id, label: edge.label }); continue; }
      remember(parseMermaidNode(part));
    }
  }
  if (nodes.size > 120 || edges.length > 240) {
    const fallback = codeBlock(source, 'mermaid');
    fallback.prepend(el('p', 'muted-note', 'Mermaid 图过于复杂，已按源码显示。'));
    return fallback;
  }
  if (!nodes.size) return codeBlock(source, 'mermaid');
  const ids = [...nodes.keys()], rank = new Map(ids.map(id => [id, 0]));
  for (let pass = 0; pass < ids.length; pass += 1) for (const edge of edges) {
    rank.set(edge.to, Math.min(ids.length - 1, Math.max(rank.get(edge.to) || 0, (rank.get(edge.from) || 0) + 1)));
  }
  const groups = new Map();
  for (const id of ids) { const value = rank.get(id) || 0; if (!groups.has(value)) groups.set(value, []); groups.get(value).push(id); }
  const horizontal = direction === 'LR' || direction === 'RL', nodeW = 150, nodeH = 44, rankGap = 70, laneGap = 24;
  const maxRank = Math.max(...groups.keys()), maxLane = Math.max(...[...groups.values()].map(group => group.length));
  const width = horizontal ? (maxRank + 1) * (nodeW + rankGap) - rankGap + 40 : maxLane * (nodeW + laneGap) - laneGap + 40;
  const height = horizontal ? maxLane * (nodeH + laneGap) - laneGap + 40 : (maxRank + 1) * (nodeH + rankGap) - rankGap + 40;
  const positions = new Map();
  for (const [r, group] of groups) group.forEach((id, lane) => {
    let x = horizontal ? 20 + r * (nodeW + rankGap) : 20 + lane * (nodeW + laneGap);
    let y = horizontal ? 20 + lane * (nodeH + laneGap) : 20 + r * (nodeH + rankGap);
    if (direction === 'RL') x = width - nodeW - x;
    if (direction === 'BT') y = height - nodeH - y;
    positions.set(id, { x, y });
  });
  const wrap = el('div', 'message-mermaid-wrap'), diagram = svg('svg', 'message-mermaid');
  diagram.setAttribute('viewBox', `0 0 ${width} ${height}`);
  diagram.setAttribute('role', 'img'); diagram.setAttribute('aria-label', 'Mermaid 流程图');
  const edgesLayer = svg('g', 'mermaid-edges'), nodesLayer = svg('g', 'mermaid-nodes');
  for (const edge of edges) {
    const a = positions.get(edge.from), b = positions.get(edge.to); if (!a || !b) continue;
    const x1 = a.x + nodeW / 2, y1 = a.y + nodeH / 2, x2 = b.x + nodeW / 2, y2 = b.y + nodeH / 2;
    const line = svg('line', 'mermaid-edge'); line.setAttribute('x1', x1); line.setAttribute('y1', y1); line.setAttribute('x2', x2); line.setAttribute('y2', y2); edgesLayer.append(line);
    const angle = Math.atan2(y2-y1, x2-x1), size = 7;
    const arrow = svg('polygon', 'mermaid-arrow');
    arrow.setAttribute('points', `${x2},${y2} ${x2-size*Math.cos(angle-.5)},${y2-size*Math.sin(angle-.5)} ${x2-size*Math.cos(angle+.5)},${y2-size*Math.sin(angle+.5)}`);
    edgesLayer.append(arrow);
    if (edge.label) { const label = svg('text', 'mermaid-edge-label'); label.setAttribute('x', (x1+x2)/2); label.setAttribute('y', (y1+y2)/2 - 5); label.textContent = edge.label; edgesLayer.append(label); }
  }
  for (const [id, item] of nodes) {
    const pos = positions.get(id), group = svg('g', 'mermaid-node');
    if (item.shape === 'diamond') {
      const shape = svg('polygon', 'mermaid-node-shape'); shape.setAttribute('points', `${pos.x+nodeW/2},${pos.y} ${pos.x+nodeW},${pos.y+nodeH/2} ${pos.x+nodeW/2},${pos.y+nodeH} ${pos.x},${pos.y+nodeH/2}`); group.append(shape);
    } else {
      const rect = svg('rect', 'mermaid-node-shape'); rect.setAttribute('x', pos.x); rect.setAttribute('y', pos.y); rect.setAttribute('width', nodeW); rect.setAttribute('height', nodeH); rect.setAttribute('rx', item.shape === 'round' ? 22 : 8); group.append(rect);
    }
    const label = svg('text', 'mermaid-node-label'); label.setAttribute('x', pos.x + nodeW/2); label.setAttribute('y', pos.y + nodeH/2); label.textContent = item.label.slice(0, 80); group.append(label); nodesLayer.append(group);
  }
  diagram.append(edgesLayer, nodesLayer); wrap.append(diagram); return wrap;
}

function reconcileChildren(target, staging) {
  const current = [...target.childNodes], next = [...staging.childNodes];
  let prefix = 0;
  while (prefix < current.length && prefix < next.length && current[prefix].isEqualNode(next[prefix])) prefix += 1;
  while (target.childNodes.length > prefix) target.lastChild.remove();
  for (let index = prefix; index < next.length; index += 1) target.append(next[index]);
}

function appendListItem(li, content) {
  const task = /^\[([ xX])\]\s+(.*)$/.exec(content);
  if (!task) { appendInline(li, content); return; }
  const checkbox = document.createElement('input'); checkbox.type = 'checkbox'; checkbox.disabled = true; checkbox.checked = task[1].toLowerCase() === 'x';
  checkbox.className = 'message-task-checkbox'; checkbox.setAttribute('aria-label', checkbox.checked ? '已完成' : '未完成');
  li.append(checkbox); appendInline(li, task[2]);
}

function renderBlocks(root, value) {
  const lines = String(value ?? '').split('\n');
  const lists = []; let paragraph = null;
  const reset = () => { paragraph = null; lists.length = 0; };
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    const fence = /^\s*(```|~~~)\s*([^\s]*)?.*$/.exec(line);
    if (fence) {
      const body = [], marker = fence[1], language = fence[2] || '';
      index += 1;
      while (index < lines.length && !new RegExp(`^\\s*${marker[0]}{${marker.length},}\\s*$`).test(lines[index])) { body.push(lines[index]); index += 1; }
      const code = body.join('\n');
      if (/^(?:mermaid)$/i.test(language)) root.append(renderMermaid(code));
      else if (/^(?:math|latex|tex)$/i.test(language)) { const wrap = el('div', 'message-math-block'); wrap.append(renderMath(code, true)); root.append(wrap); }
      else root.append(codeBlock(code, language));
      reset(); continue;
    }
    if (/^\s*\$\$/.test(line)) {
      let math = line.replace(/^\s*\$\$\s*/, ''), closed = /\$\$\s*$/.test(math);
      if (closed) math = math.replace(/\s*\$\$\s*$/, '');
      else {
        const body = [math]; index += 1;
        while (index < lines.length && !/\$\$\s*$/.test(lines[index])) { body.push(lines[index]); index += 1; }
        if (index < lines.length) body.push(lines[index].replace(/\$\$\s*$/, ''));
        math = body.join('\n');
      }
      const wrap = el('div', 'message-math-block'); wrap.append(renderMath(math, true)); root.append(wrap); reset(); continue;
    }
    if (!line.trim()) { paragraph = null; continue; }
    if (/^\s*>/.test(line)) {
      const quoteLines = [];
      while (index < lines.length && /^\s*>/.test(lines[index])) { quoteLines.push(lines[index].replace(/^\s*>\s?/, '')); index += 1; }
      index -= 1; const quote = document.createElement('blockquote'); renderBlocks(quote, quoteLines.join('\n')); root.append(quote); reset(); continue;
    }
    const heading = /^(#{1,6})\s+(.*?)(?:\s+#+)?\s*$/.exec(line);
    if (heading) { root.append(inlineBlock(`h${heading[1].length}`, heading[2])); reset(); continue; }
    const underline = /^\s*(=+|-+)\s*$/.exec(line);
    if (paragraph && underline && !paragraph.querySelector('br')) {
      paragraph.replaceWith(inlineBlock(underline[1][0] === '=' ? 'h1' : 'h2', paragraph.textContent)); reset(); continue;
    }
    if (/^\s*(?:-{3,}|\*{3,}|_{3,})\s*$/.test(line)) { root.append(el('hr', 'message-rule')); reset(); continue; }
    const item = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/.exec(line);
    if (item) {
      const indent = item[1].replace(/\t/g, '  ').length, tag = /\d/.test(item[2]) ? 'ol' : 'ul';
      while (lists.length && lists.at(-1).indent > indent) lists.pop();
      let level = lists.at(-1);
      if (!level || (level.indent < indent && level.item)) {
        const nested = document.createElement(tag); if (level) level.item.append(nested); else root.append(nested);
        level = { node:nested, indent, tag, item:null }; lists.push(level);
      } else if (level.tag !== tag) {
        const sibling = document.createElement(tag); level.node.after(sibling); level = { node:sibling, indent, tag, item:null }; lists[lists.length - 1] = level;
      }
      if (tag === 'ol' && !level.node.childElementCount) { const start = Number.parseInt(item[2], 10); if (start > 1) level.node.start = start; }
      const li = document.createElement('li'); appendListItem(li, item[3]); level.node.append(li); level.item = li; paragraph = null; continue;
    }
    const indent = /^\s*/.exec(line)[0].replace(/\t/g, '  ').length;
    if (lists.length && indent > lists.at(-1).indent && lists.at(-1).item) {
      lists.at(-1).item.append(document.createElement('br')); appendInline(lists.at(-1).item, line.trim()); continue;
    }
    if (line.includes('|') && index + 1 < lines.length && tableDelimiter(lines[index + 1])) {
      const alignments = tableAlignments(lines[index + 1]), head = splitTableRow(line); index += 2; const rows = [];
      while (index < lines.length && lines[index].trim() && lines[index].includes('|')) { rows.push(splitTableRow(lines[index])); index += 1; }
      index -= 1; root.append(tableBlock(head, rows, alignments)); reset(); continue;
    }
    lists.length = 0;
    if (!paragraph) { paragraph = document.createElement('p'); root.append(paragraph); }
    else paragraph.append(document.createElement('br'));
    appendInline(paragraph, line);
  }
}

export function renderMessage(target, value) {
  const staging = document.createElement('div'); renderBlocks(staging, value); reconcileChildren(target, staging);
}

function imageBlock(block) {
  const figure = el('figure', 'message-media');
  const type = String(block.media_type || '');
  if (!IMAGE_TYPES.has(type)) { figure.append(el('p', 'muted-note', `不支持的图片类型：${type || 'unknown'}`)); return figure; }
  const img = document.createElement('img'); img.alt = '工具返回的图片'; img.loading = 'lazy'; img.decoding = 'async';
  const source = block.source || {};
  if (source.kind === 'base64' && typeof source.data === 'string' && /^[A-Za-z0-9+/]+=*$/.test(source.data)) {
    img.src = `data:${type};base64,${source.data}`; figure.append(img); return figure;
  }
  if (source.kind === 'redacted') {
    figure.append(el('p', 'muted-note', '远程图片地址已由宿主隐藏；当前没有可安全显示的内联图片数据。'));
    return figure;
  }
  figure.append(el('p', 'muted-note', '图片数据不可用。')); return figure;
}

export function artifactViewer(artifact, { runId = '', artifactReader = null, label = '完整结果' } = {}) {
  const block = el('div', 'artifact-viewer');
  const bytes = Number(artifact?.bytes) || 0, uri = String(artifact?.uri || '');
  const scheme = /^([a-z][a-z0-9+.-]*):/i.exec(uri)?.[1] || 'artifact';
  const head = el('div', 'artifact-viewer-head');
  head.append(el('strong', '', label), el('span', 'muted-note', bytes ? `${bytes.toLocaleString()} 字节 · ${scheme}` : scheme));
  const actions = el('div', 'artifact-viewer-actions');
  const copy = el('button', 'text-button', '复制引用'); copy.type = 'button'; copy.addEventListener('click', () => { void copyText(uri, copy); }); actions.append(copy); head.append(actions); block.append(head);
  const output = el('pre', 'activity-panel-output artifact-output'); output.hidden = true;
  if (artifactReader?.supports?.(uri)) {
    const note = el('p', 'muted-note', '内容保存在受控宿主归档中。');
    const read = el('button', 'text-button', '读取内容'); read.type = 'button'; let offset = 0;
    read.addEventListener('click', async () => {
      read.disabled = true; note.textContent = '正在读取…';
      try {
        const page = await artifactReader.readPage(runId, uri, offset);
        output.hidden = false; output.append(document.createTextNode(page.text)); offset = page.next_offset;
        read.textContent = page.eof ? '已读取完整内容' : '读取下一页'; read.disabled = page.eof;
        note.textContent = `已读取 ${offset.toLocaleString()} / ${Number(page.total_bytes).toLocaleString()} 字节。`;
      } catch (error) { read.disabled = false; note.textContent = error?.message || '读取内容失败。'; }
    });
    block.append(note, read, output);
  } else block.append(el('p', 'muted-note', '当前宿主未提供此引用的浏览器读取方式；引用本身不会被页面直接打开。'));
  return block;
}

function resourceBlock(block, options) {
  const card = el('div', 'message-resource');
  const title = el('strong', '', block.name || '资源');
  const bytes = Number(block.bytes) || 0;
  const meta = el('span', 'muted-note', `${block.media_type || 'application/octet-stream'}${bytes ? ` · ${bytes.toLocaleString()} 字节` : ''}`);
  card.append(title, meta);
  if (block.reference) card.append(artifactViewer(block.reference, { ...options, label: block.name || '资源内容' }));
  return card;
}

export function richContentBlock(blocks, options = {}) {
  const root = el('div', 'rich-content');
  for (const block of Array.isArray(blocks) ? blocks : []) {
    if (block?.type === 'text') { const body = el('div', 'message-text'); renderMessage(body, block.text || ''); root.append(body); }
    else if (block?.type === 'image') root.append(imageBlock(block));
    else if (block?.type === 'resource') root.append(resourceBlock(block, options));
  }
  return root;
}

function diffView(detail) {
  const wrap = el('div', 'structured-diff');
  const head = el('div', 'structured-detail-head');
  head.append(el('strong', '', detail.path || '文件变更'));
  if (Number.isInteger(detail.firstChangedLine)) head.append(el('span', 'muted-note', `第 ${detail.firstChangedLine} 行附近`));
  wrap.append(head);
  if (typeof detail.diff === 'string' && detail.diff) {
    const pre = el('pre', 'diff-view');
    const lines = detail.diff.split('\n');
    lines.forEach((line, index) => {
      const kind = line.startsWith('+++') || line.startsWith('---') || line.startsWith('@@') ? 'meta'
        : line.startsWith('+') ? 'add' : line.startsWith('-') ? 'delete' : '';
      pre.append(kind ? tokenSpan(kind, line) : document.createTextNode(line));
      if (index < lines.length - 1) pre.append(document.createTextNode('\n'));
    });
    wrap.append(pre);
  }
  if (detail.diffTruncated) wrap.append(el('p', 'muted-note', 'Diff 过长，当前只显示受限预览。'));
  return wrap;
}

function jsonDetail(label, value) {
  const details = el('details', 'structured-json'), summary = el('summary', '', label);
  const pre = el('pre', 'activity-panel-result');
  try { pre.textContent = JSON.stringify(value, null, 2); } catch { pre.textContent = String(value); }
  details.append(summary, pre); return details;
}

export function structuredDetailsBlock(details) {
  const root = el('div', 'structured-details'), consumed = new Set();
  const codingDiff = details?.['coding.diff'];
  if (codingDiff && typeof codingDiff === 'object') { root.append(diffView(codingDiff)); consumed.add('coding.diff'); }
  if (details && typeof details === 'object') for (const [key, value] of Object.entries(details)) {
    if (!consumed.has(key)) root.append(jsonDetail(key, value));
  }
  return root.childElementCount ? root : null;
}
