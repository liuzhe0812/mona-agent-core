// Safe token-to-DOM presentation. Content identity, not mutated DOM equality, controls reuse.
import { parseMarkdown, markdown } from './vendor/content/parser.mjs';
import { katex, purify } from './vendor/content/core.mjs';
import { element as el, button, contentAction, copyText, downloadContent, previewDialog, closeContentPreviews, zoomPreview, safeWebUrl, appendPlain, csvText } from './content-dom.mjs';
import { diagramView } from './diagram-view.mjs';
import { paneIcon } from './pane-icons.mjs';
import { uiRegistry } from './ui/registry.mjs';

const states = new WeakMap(), codeStates = new WeakMap();
const IMAGE_TYPES = new Set(['image/png', 'image/jpeg', 'image/webp', 'image/gif']);
let nextMessage = 0, highlightModule, parserWorker, parseId = 0;
const parseCalls = new Map();
function attr(token, key) { return token.attrs?.find(([name]) => name === key)?.[1] || ''; }
function cleanup(node) { closeContentPreviews(node); node.dispose?.(); for (const child of node.querySelectorAll?.('[data-content-owned]') || []) child.dispose?.(); }
export function disposeMessage(target) {
  closeContentPreviews(target);
  const state = states.get(target); if (state) { state.disposed = true; state.request++; state.controller.abort(); state.pendingParse = null; for (const block of state.blocks) cleanup(block.node); states.delete(target); }
}
function largeParse(source, signal) {
  if (signal?.aborted) return Promise.reject(new DOMException('正文已关闭。', 'AbortError'));
  if (typeof Worker === 'undefined') return Promise.resolve(parseMarkdown(source));
  if (parseCalls.size >= 16) return Promise.reject(new Error('正在排版其他消息，请稍后重试。'));
  if (!parserWorker) {
    parserWorker = new Worker(new URL('./markdown-worker.mjs', import.meta.url), { type: 'module' });
    parserWorker.onmessage = ({ data }) => { const item = parseCalls.get(data.id); if (!item) return; item.cleanup(); data.error ? item.reject(new Error(data.error)) : item.resolve(data.tokens); };
    parserWorker.onerror = () => { parserWorker?.terminate(); parserWorker = null; for (const item of [...parseCalls.values()]) { item.cleanup(); item.reject(new Error('正文排版组件未能加载，已保留原文。')); } parseCalls.clear(); };
  }
  return new Promise((resolve, reject) => {
    const id = ++parseId, cleanup = () => { clearTimeout(timer); parseCalls.delete(id); signal?.removeEventListener('abort', aborted); };
    const aborted = () => { cleanup(); reject(new DOMException('正文已关闭。', 'AbortError')); };
    const timer = setTimeout(() => { parserWorker?.terminate(); parserWorker = null; for (const item of [...parseCalls.values()]) { item.cleanup(); item.reject(new Error('正文排版超时，已保留原文。')); } }, 5000);
    parseCalls.set(id, { resolve, reject, timer, cleanup }); signal?.addEventListener('abort', aborted, { once: true });
    try { parserWorker.postMessage({ id, source }); } catch (error) { cleanup(); reject(error); }
  });
}
export function renderMath(source, display = false) {
  const root = el(display ? 'div' : 'span', display ? 'message-math-block' : 'message-math');
  root.setAttribute('aria-label', String(source));
  try {
    if (String(source).length > 8192) throw new Error('公式过长');
    katex.render(String(source), root, { displayMode: display, throwOnError: true, trust: false, strict: 'ignore', maxExpand: 1000, maxSize: 20, output: 'htmlAndMathml' });
  } catch { root.classList.add('math-unavailable'); root.append(el('code', '', source)); root.dataset.tooltip = '公式无法排版，显示原文'; }
  return root;
}
export function renderMermaid(source, options = {}) { return diagramView(String(source || ''), options); }
function normalLanguage(value) { const raw = String(value || '').trim().split(/\s+/)[0].toLowerCase(); return ({ rs: 'rust', py: 'python', js: 'javascript', ts: 'typescript', sh: 'bash', ps1: 'powershell' })[raw] || raw || 'text'; }
export function codeBlock(source, language = '', options = {}) {
  const root = el('section', 'message-code-block'), head = el('div', 'message-code-head'), actions = el('div', 'message-code-actions');
  const label = el('span', 'message-code-language'), pre = el('pre', 'message-code'), code = el('code'); pre.append(code);
  const state = { root, label, code, pre, source: '', language: '', version: 0, disposed: false, expanded: false, userExpanded: false, options };
  const copy = contentAction('复制代码', 'copy', () => { void copyText(state.source, copy); }, 'code-copy');
  const wrap = contentAction('换行', 'wrap', () => { root.classList.toggle('is-wrapped'); wrap.setAttribute('aria-pressed', String(root.classList.contains('is-wrapped'))); }, 'code-wrap'); wrap.setAttribute('aria-pressed', 'false');
  const expand = contentAction('展开代码', 'expand', () => { state.userExpanded = true; state.expanded = !state.expanded; root.classList.toggle('is-collapsed', !state.expanded); expand.setAttribute('aria-label', state.expanded ? '收起代码' : '展开代码'); expand.dataset.tooltip = expand.getAttribute('aria-label'); expand.setAttribute('aria-expanded', String(state.expanded)); }, 'code-expand'); state.expand = expand;
  const save = contentAction('下载代码', 'download', () => downloadContent(state.source, 'text/plain;charset=utf-8', 'code.' + (({ rust: 'rs', python: 'py', javascript: 'js', typescript: 'ts', bash: 'sh' })[state.language] || 'txt')), 'code-download');
  actions.append(expand, wrap, copy, save); head.append(paneIcon('file'), label, actions); root.append(head, pre); root.dataset.contentOwned = 'code';
  root.dispose = () => { state.disposed = true; state.version++; }; codeStates.set(root, state); updateCodeBlock(root, source, language, options); return root;
}
export function updateCodeBlock(root, source, language, options = {}) {
  const state = codeStates.get(root); if (!state) return;
  const value = String(source ?? ''), lang = normalLanguage(language), changed = value !== state.source || lang !== state.language;
  if (changed) { state.source = value; state.language = lang; state.version++; state.highlighted = null; appendPlain(state.code, value); }
  state.options = options; state.label.textContent = lang === 'text' ? '代码' : lang;
  const long = value.split('\n').length > 18 || value.length > 2400; state.expand.hidden = !long;
  if (!state.userExpanded) { state.expanded = !long; root.classList.toggle('is-collapsed', long); }
  if (options.streaming || state.disposed || value.length > 128 * 1024 || lang === 'text') return;
  if (state.highlighted === value + '\0' + lang) return;
  const version = state.version; highlightModule ||= import('./vendor/content/highlight.mjs').catch(error => { highlightModule = null; throw error; });
  highlightModule.then(({ highlight }) => {
    if (state.disposed || version !== state.version || state.options.streaming) return;
    const selection = window.getSelection(); if (selection && !selection.isCollapsed && selection.containsNode(state.code, true)) return;
    const result = highlight(value, lang);
    if (result) state.code.replaceChildren(purify.sanitize(result, { RETURN_DOM_FRAGMENT: true, ALLOWED_TAGS: ['span'], ALLOWED_ATTR: ['class'] }));
    state.highlighted = value + '\0' + lang;
  }).catch(() => { /* Highlighting is optional; the complete source remains readable. */ });
}
function imageView(source, alt, options) {
  const root = el('figure', 'message-media'); root.dataset.contentOwned = 'image';
  const note = el('p', 'muted-note'), image = el('img'); image.alt = alt || '图片'; image.loading = 'lazy'; image.decoding = 'async'; image.referrerPolicy = 'no-referrer';
  const controller = new AbortController(); let disposed = false, url, attempt = 0, loading = false;
  const trigger = button('', () => {
    if (image.hidden || !image.naturalWidth) return;
    const gallery = root.closest('.message-image-gallery');
    const images = gallery ? [...gallery.querySelectorAll('.message-image-trigger img')].filter(item => !item.hidden && item.naturalWidth) : [image];
    let index = Math.max(0, images.indexOf(image));
    const preview = zoomPreview(images[index].cloneNode(true), { stepZoom: true });
    const download = contentAction('下载图片', 'download', () => {
      const current = images[index], kind = /^data:image\/(png|jpeg|webp|gif);base64,/i.exec(current.src)?.[1];
      if (!kind) return;
      const name = (current.alt || 'image').replace(/[^\p{L}\p{N}._-]/gu, '_').slice(0, 60) || 'image';
      const link = el('a'); link.href = current.src; link.download = `${name}.${kind === 'jpeg' ? 'jpg' : kind}`; document.body.append(link); link.click(); link.remove();
    });
    let change;
    if (images.length > 1) {
      const nav = el('div', 'message-image-gallery-nav'), count = el('span', '', `${index + 1} / ${images.length}`);
      const previous = contentAction('上一张', 'previous', () => change(-1));
      const next = contentAction('下一张', 'next', () => change(1));
      nav.append(previous, count, next); preview.prepend(nav);
      change = direction => { index = (index + direction + images.length) % images.length; preview.querySelector('.content-zoom-area').replaceChildren(images[index].cloneNode(true)); count.textContent = `${index + 1} / ${images.length}`; dialog.querySelector('.content-preview-header strong').textContent = images[index].alt || '图片'; download.hidden = !/^data:image\/(?:png|jpeg|webp|gif);base64,/i.test(images[index].src); };
    }
    const dialog = previewDialog(images[index].alt || '图片', preview, { owner: root });
    if (dialog) { dialog.classList.add('image-preview-dialog'); const header = dialog.querySelector('.content-preview-header'), close = header.querySelector('button'); close.replaceChildren(paneIcon('close')); close.setAttribute('aria-label', '关闭图片'); download.hidden = !/^data:image\/(?:png|jpeg|webp|gif);base64,/i.test(images[index].src); header.insertBefore(download, close); }
    if (dialog && change) dialog.addEventListener('keydown', event => { if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') { event.preventDefault(); change(event.key === 'ArrowLeft' ? -1 : 1); } });
  }, 'message-image-trigger image-preview-open');
  trigger.setAttribute('aria-label', '查看大图'); trigger.setAttribute('aria-disabled', 'true');
  const placeholder = el('span', 'message-image-placeholder'); placeholder.append(paneIcon('image'));
  trigger.append(placeholder, image); root.append(trigger, note); image.hidden = true;
  const show = src => { if (disposed) return; placeholder.replaceChildren(paneIcon('image')); image.src = src; image.hidden = false; placeholder.hidden = false; trigger.dataset.state = 'loading'; note.textContent = ''; };
  image.addEventListener('load', () => { image.hidden = false; placeholder.hidden = true; trigger.dataset.state = 'ready'; trigger.setAttribute('aria-disabled', 'false'); note.textContent = ''; root.dispatchEvent(new CustomEvent('mona:content-resized', { bubbles: true })); });
  image.addEventListener('error', () => { note.textContent = '图片无法显示，请检查文件或地址。'; image.hidden = true; placeholder.replaceChildren(paneIcon('imageOff')); placeholder.hidden = false; trigger.dataset.state = 'error'; trigger.setAttribute('aria-disabled', 'true'); });
  const retry = button('重试加载图片', () => { void loadLocal(); }, 'text-button image-retry'); retry.hidden = true; root.append(retry);
  root.dispose = () => { disposed = true; attempt++; controller.abort(); closeContentPreviews(root); if (url) URL.revokeObjectURL(url); image.removeAttribute('src'); };
  const inline = /^data:(image\/(?:png|jpeg|webp|gif));base64,([A-Za-z0-9+/]+=*)$/.exec(source);
  if (inline && source.length <= 6 * 1024 * 1024) show(source);
  else if (/^https:\/\//i.test(source) && safeWebUrl(source)) {
    const load = button('加载远程图片', () => { load.remove(); show(safeWebUrl(source)); }); note.textContent = '远程图片需确认后加载'; root.append(load);
  } else if (options.readMedia && source && (!/^[a-z][a-z0-9+.-]*:/i.test(source) || /^[a-z]:[\\/]|^file:\/\//i.test(source)) && !source.startsWith('//')) {
    void loadLocal();
  } else { note.textContent = '当前图片没有可安全读取的来源。'; placeholder.replaceChildren(paneIcon('imageOff')); trigger.dataset.state = 'error'; }
  async function loadLocal() {
    if (disposed || loading || !options.readMedia) return;
    loading = true; retry.hidden = true; trigger.dataset.state = 'loading'; placeholder.replaceChildren(paneIcon('image')); const generation = ++attempt; note.textContent = '正在读取图片…';
    try {
      await Promise.resolve();
      if (disposed || options.signal?.aborted) throw new DOMException('预览已关闭。', 'AbortError');
      const page = await options.readMedia(source, { signal: controller.signal });
      if (disposed || generation !== attempt || options.signal?.aborted) return;
      if (!IMAGE_TYPES.has(page.media_type) || typeof page.base64 !== 'string' || page.base64.length > 6 * 1024 * 1024 || !/^[A-Za-z0-9+/]+=*$/.test(page.base64)) throw new Error('图片格式或大小不支持。');
      show(`data:${page.media_type};base64,${page.base64}`);
    } catch (error) { if (!disposed && generation === attempt && error.name !== 'AbortError') { note.textContent = error.message; placeholder.replaceChildren(paneIcon('imageOff')); trigger.dataset.state = 'error'; retry.hidden = false; } }
    finally { if (generation === attempt) loading = false; }
  }
  return root;
}
function linkNode(href, options, token) {
  const web = safeWebUrl(href);
  if (web) { const link = el('a'); link.href = web; link.target = '_blank'; link.rel = 'noopener noreferrer'; return link; }
  if (href.startsWith('#') && options.root) {
    const link = el('a'); link.href = href;
    link.addEventListener('click', event => {
      event.preventDefault(); let id; try { id = decodeURIComponent(href.slice(1)); } catch { return; }
      const target = options.root.querySelector(`[data-markdown-anchor="${CSS.escape(id)}"],[id="${CSS.escape(id)}"]`);
      target?.scrollIntoView({ block: 'center' });
    }); return link;
  }
  if (options.openFile && href && (!/^(?:[a-z][a-z0-9+.-]*:|\/\/)/i.test(href) || /^[a-z]:[\\/]|^file:\/\//i.test(href))) {
    const link = button('', () => {
      link.disabled = true;
      Promise.resolve().then(() => {
        if (options.signal?.aborted || !link.isConnected) throw new DOMException('文件引用已失效。', 'AbortError');
        return options.openFile(href);
      }).catch(error => { if (options.signal?.aborted || !link.isConnected || error.name === 'AbortError') return; options.onError?.(error.message); link.dataset.tooltip = error.message; }).finally(() => { link.disabled = false; });
    }, 'message-file-link'); link.dataset.filePath = href; link.dataset.tooltip = href; link.append(paneIcon('file')); return link;
  }
  return el('span', 'message-unavailable-link');
}
function renderInline(parent, tokens, options, task = false) {
  const stack = [parent]; let first = true;
  for (const token of tokens || []) {
    let root = stack.at(-1);
    if (token.type === 'text') {
      const match = task && first && /^\[([ xX])\]\s+(.*)$/s.exec(token.content);
      if (match) { const checkbox = el('input', 'message-task-checkbox'); checkbox.type = 'checkbox'; checkbox.disabled = true; checkbox.checked = match[1].toLowerCase() === 'x'; checkbox.setAttribute('aria-label', checkbox.checked ? '已完成' : '未完成'); root.append(checkbox, document.createTextNode(match[2])); }
      else root.append(document.createTextNode(token.content)); first = false; continue;
    }
    first = false;
    if (token.type === 'image') { root.append(imageView(attr(token, 'src'), token.content, options)); continue; }
    if (token.type === 'mona_math_inline') { root.append(renderMath(token.content)); continue; }
    if (token.type === 'code_inline') { root.append(el('code', '', token.content)); continue; }
    if (token.type === 'softbreak') { root.append(document.createTextNode('\n')); continue; }
    if (token.type === 'hardbreak') { root.append(el('br')); continue; }
    if (token.type === 'footnote_ref') {
      const id = token.meta.id + 1, reference = el('sup', 'message-footnote'); const a = el('a', '', String(id)); a.href = `#${options.identity}-footnote-${id}`;
      a.addEventListener('click', event => { event.preventDefault(); options.root.querySelector(`#${options.identity}-footnote-${id}`)?.scrollIntoView({ block: 'center' }); }); reference.append(a); root.append(reference); continue;
    }
    if (token.nesting === -1) { if (stack.length > 1) stack.pop(); continue; }
    if (token.nesting === 1) {
      const allowed = new Set(['em', 'strong', 's', 'a']);
      if (!allowed.has(token.tag)) { stack.push(root); continue; }
      const child = token.tag === 'a' ? linkNode(attr(token, 'href'), options, token) : el(token.tag === 's' ? 'del' : token.tag);
      root.append(child); stack.push(child); continue;
    }
    if (token.content) root.append(document.createTextNode(token.content));
  }
}
export function appendInline(parent, value, options = {}) { renderInline(parent, markdown.parseInline(String(value), {})[0]?.children, options); }
function tableFrame() {
  const root = el('div', 'message-table-frame'), toolbar = el('div', 'message-table-actions'), wrap = el('div', 'message-table'), table = el('table');
  root.dataset.contentOwned = 'table';
  const rows = () => [...table.rows].map(row => [...row.cells].map(cell => cell.textContent.trim()));
  const copy = contentAction('复制表格', 'copy', () => {
    const values = rows(), escape = text => text.replace(/\\/g, '\\\\').replace(/\|/g, '\\|').replace(/\n/g, ' ');
    const formatted = [values[0] || [], (values[0] || []).map(() => '---'), ...values.slice(1)];
    void copyText(formatted.map(row => '| ' + row.map(escape).join(' | ') + ' |').join('\n'), copy);
  });
  const save = contentAction('下载 CSV', 'download', () => downloadContent(csvText(rows()), 'text/csv;charset=utf-8', 'table.csv'));
  const expand = contentAction('放大表格', 'expand', () => { const view = el('div', 'message-table'); view.append(table.cloneNode(true)); previewDialog('表格', view, { owner: root }); });
  const wide = contentAction('扩展横向滚动', 'horizontalExpand', () => { const expanded = root.classList.toggle('is-expanded'); wide.setAttribute('aria-pressed', String(expanded)); wide.setAttribute('aria-label', expanded ? '收起横向滚动' : '扩展横向滚动'); wide.dataset.tooltip = wide.getAttribute('aria-label'); update(); }, 'message-table-wide'); wide.hidden = true; wide.setAttribute('aria-pressed', 'false');
  const update = () => {
    if (!root.isConnected) return;
    const maximum = Math.max(0, wrap.scrollWidth - wrap.clientWidth), overflowing = maximum > 1;
    root.classList.toggle('has-left-edge', overflowing && wrap.scrollLeft > 1);
    root.classList.toggle('has-right-edge', overflowing && wrap.scrollLeft < maximum - 1);
    wide.hidden = !root.classList.contains('is-expanded') && (!overflowing || (root.closest('.workspace')?.clientWidth || 0) <= root.clientWidth + 80);
  };
  wrap.addEventListener('scroll', update, { passive: true });
  const size = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(update);
  if (size) { size.observe(wrap); size.observe(table); }
  root.dispose = () => size?.disconnect();
  toolbar.append(copy, save, expand, wide); toolbar.setAttribute('aria-label', '表格操作'); wrap.append(table); root.append(toolbar, wrap); requestAnimationFrame(update); return { root, table };
}
function groups(tokens) {
  const result = []; let current = [], level = 0;
  for (const token of tokens) { current.push(token); level += token.nesting; if (level === 0) { result.push(current); current = []; } }
  if (current.length) result.push(current); return result;
}
const imageLine = /^\s*!\[[^\]]*\]\([^\n]+\)\s*$/;
const fenceLine = /^( {0,3})(`{3,}|~{3,})(.*)$/;
function groupImageLines(source) {
  const lines = source.split('\n'), result = []; let fence = '', length = 0;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i], marker = fenceLine.exec(line);
    if (marker) {
      if (fence === marker[2][0] && marker[2].length >= length && !marker[3].trim()) { fence = ''; length = 0; }
      else if (!fence && (marker[2][0] === '~' || !marker[3].includes('`'))) { fence = marker[2][0]; length = marker[2].length; }
      result.push(line); continue;
    }
    result.push(line);
    if (fence || !imageLine.test(line)) continue;
    let next = i + 1; while (next < lines.length && !lines[next].trim()) next++;
    if (next > i + 1 && next < lines.length && imageLine.test(lines[next])) i = next - 1;
  }
  return result.join('\n');
}
function buildBlock(tokens, options) {
  const first = tokens[0];
  if (first.type === 'fence' || first.type === 'code_block') {
    const language = first.info?.trim().split(/\s+/)[0] || 'text';
    if (language === 'mermaid') { const node = renderMermaid(first.content, options); node.dataset.contentOwned = 'diagram'; return node; }
    if (['math', 'latex', 'tex'].includes(language)) return renderMath(first.content, true);
    return codeBlock(first.content.replace(/\n$/, ''), language, options);
  }
  if (first.type === 'mona_math_block') return options.streaming && !first.meta?.complete ? el('pre', 'message-code', first.content) : renderMath(first.content, true);
  const holder = el('div'), stack = [holder];
  for (const token of tokens) {
    const parent = stack.at(-1);
    if (token.type === 'inline') {
      const meaningful = (token.children || []).filter(child => child.type !== 'text' || child.content.trim());
      if (parent.tagName === 'P' && meaningful.filter(child => child.type === 'image').length >= 2 && meaningful.every(child => ['image', 'softbreak', 'hardbreak'].includes(child.type))) parent.classList.add('message-image-gallery');
      renderInline(parent, token.children, options, Boolean(parent.closest('li'))); continue;
    }
    if (token.type === 'fence' || token.type === 'code_block' || token.type === 'mona_math_block') { parent.append(buildBlock([token], options)); continue; }
    if (token.type === 'footnote_block_open') { const list = el('ol', 'message-footnotes'); parent.append(list); stack.push(list); continue; }
    if (token.type === 'footnote_open') { const item = el('li'); item.id = `${options.identity}-footnote-${token.meta.id + 1}`; parent.append(item); stack.push(item); continue; }
    if (token.type === 'footnote_anchor') continue;
    if (token.nesting === -1) { if (stack.length > 1) stack.pop(); continue; }
    if (token.nesting === 1) {
      if (token.type === 'table_open') { const table = tableFrame(); parent.append(table.root); stack.push(table.table); continue; }
      const tags = new Set(['p', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'ul', 'ol', 'li', 'blockquote', 'thead', 'tbody', 'tr', 'th', 'td']);
      if (!tags.has(token.tag)) { stack.push(parent); continue; }
      const child = el(token.tag); if (token.hidden) child.className = 'message-loose-paragraph';
      if (token.tag === 'ol') { const start = Number(attr(token, 'start')); if (Number.isInteger(start) && start > 1) child.start = start; }
      const alignment = /^text-align:(left|center|right)$/.exec(attr(token, 'style')); if (alignment) child.classList.add('align-' + alignment[1]);
      parent.append(child); stack.push(child); continue;
    }
    if (token.type === 'hr') parent.append(el('hr', 'message-rule'));
    else if (token.content) parent.append(document.createTextNode(token.content));
  }
  if (holder.childNodes.length === 1) return holder.firstChild; holder.className = 'message-block'; return holder;
}
function patchPlain(old, fresh) {
  if (old.nodeType !== fresh.nodeType || old.nodeName !== fresh.nodeName) return false;
  if (old.nodeType === 3) { const value = fresh.data; if (value.startsWith(old.data)) old.appendData(value.slice(old.data.length)); else old.replaceData(0, old.length, value); return true; }
  // An interactive descendant does not invalidate its entire paragraph. Keep unchanged
  // inline code/emphasis/links in place, but never reconcile through a resource owner.
  if (old.matches?.('button,img,math,input,[data-content-owned]') || fresh.matches?.('button,img,math,input,[data-content-owned]')) {
    if (old.matches?.('.message-file-link') && fresh.matches?.('.message-file-link') && old.dataset.filePath === fresh.dataset.filePath && old.textContent === fresh.textContent) return true;
    return old.isEqualNode(fresh);
  }
  if (old.attributes?.length !== fresh.attributes?.length || [...(old.attributes || [])].some(a => fresh.getAttribute(a.name) !== a.value)) return false;
  const previous = [...old.childNodes], next = [...fresh.childNodes];
  for (let i = 0; i < next.length; i++) { if (!previous[i]) old.append(next[i]); else if (!patchPlain(previous[i], next[i])) { cleanup(previous[i]); previous[i].replaceWith(next[i]); } }
  for (let i = next.length; i < previous.length; i++) { cleanup(previous[i]); previous[i].remove(); } return true;
}
export function renderMessage(target, value, options = {}) {
  const source = String(value ?? ''), scope = options.contextKey || '';
  const displaySource = groupImageLines(source);
  let state = states.get(target);
  if (state && state.scope !== scope) { disposeMessage(target); target.replaceChildren(); state = null; }
  if (!state) { state = { identity: `message-${++nextMessage}`, request: 0, blocks: [], scope, disposed: false, controller: new AbortController() }; states.set(target, state); }
  if (state.source === source && state.streaming === Boolean(options.streaming)) return state.ready || Promise.resolve();
  state.source = source; state.streaming = Boolean(options.streaming); const request = ++state.request;
  const context = { ...options, root: target, identity: state.identity, signal: state.controller.signal };
  target.dataset.rendering = 'true';
  const commit = tokens => {
    if (state.disposed || request !== state.request) return;
    const parsed = groups(tokens); if (parsed.length > 4000 || tokens.length > 50000) throw new Error('正文结构过于复杂，显示原文。');
    const old = state.blocks, next = [];
    parsed.forEach((group, index) => {
      const signature = JSON.stringify(group), previous = old[index], first = group[0];
      if (previous?.signature === signature && !(previous.incompleteMath && !options.streaming)) {
        if (!options.streaming) {
          for (const node of [previous.node, ...(previous.node.querySelectorAll?.('[data-content-owned]') || [])]) {
            node.complete?.(); const code = codeStates.get(node); if (code) updateCodeBlock(node, code.source, code.language, options);
          }
        }
        next.push(previous); return;
      }
      if (previous && codeStates.has(previous.node) && ['fence', 'code_block'].includes(first.type) && first.info !== 'mermaid' && !['math', 'tex', 'latex'].includes(first.info)) {
        updateCodeBlock(previous.node, first.content.replace(/\n$/, ''), first.info, options); next.push({ node: previous.node, signature }); return;
      }
      let node = buildBlock(group, context);
      if (previous && patchPlain(previous.node, node)) { cleanup(node); node = previous.node; }
      else if (previous) cleanup(previous.node);
      next.push({ node, signature, incompleteMath: options.streaming && group.some(token => token.type === 'mona_math_block' && !token.meta?.complete) });
    });
    const retained = new Set(next.map(block => block.node)); for (const block of old) if (!retained.has(block.node)) { cleanup(block.node); block.node.remove(); }
    next.forEach((block, index) => { if (target.childNodes[index] !== block.node) target.insertBefore(block.node, target.childNodes[index] || null); });
    while (target.childNodes.length > next.length) target.lastChild.remove(); state.blocks = next;
    const anchors = new Map();
    for (const heading of target.querySelectorAll('h1,h2,h3,h4,h5,h6')) {
      const base = heading.textContent.normalize('NFC').toLocaleLowerCase().replace(/[^\p{L}\p{N}\s_-]/gu, '').trim().replace(/\s+/g, '-') || 'section';
      const count = anchors.get(base) || 0; anchors.set(base, count + 1); heading.dataset.markdownAnchor = base + (count ? '-' + count : '');
    }
    target.dataset.rendering = 'false'; target.dispatchEvent(new CustomEvent('mona:content-resized', { bubbles: true }));
  };
  const failed = error => {
    if (state.disposed || request !== state.request) return;
    for (const block of state.blocks) cleanup(block.node); state.blocks = [];
    const raw = el('pre', 'message-raw-fallback', source), note = el('p', 'muted-note', '排版暂不可用，原文已保留。'); note.dataset.tooltip = String(error.message || error).slice(0, 300); target.replaceChildren(note, raw);
    const retry = button('重新排版', () => { state.source = undefined; void renderMessage(target, source, options); }); target.insertBefore(retry, raw);
    target.dataset.rendering = 'false'; target.dispatchEvent(new CustomEvent('mona:content-resized', { bubbles: true }));
  };
  if (source.length > 32768 && typeof Worker !== 'undefined') {
    // One outstanding parse per message; intermediate streaming snapshots are replaced, not queued.
    state.pendingParse = { source: displaySource, commit, failed };
    if (!state.parsing) {
      state.parsing = (async () => {
        while (state.pendingParse && !state.disposed) {
          const job = state.pendingParse; state.pendingParse = null;
          try { job.commit(await largeParse(job.source, state.controller.signal)); } catch (error) { job.failed(error); }
        }
      })().finally(() => { state.parsing = null; });
    }
    state.ready = state.parsing;
  } else { state.pendingParse = null; try { commit(parseMarkdown(displaySource)); } catch (error) { failed(error); } state.ready = Promise.resolve(); }
  return state.ready;
}

export function artifactViewer(artifact, { runId = '', artifactReader = null, label = '完整结果', openArtifact } = {}) {
  const root = el('section', 'artifact-viewer'); root.dataset.contentOwned = 'artifact'; let disposed = false, offset = 0;
  root.dispose = () => { disposed = true; };
  const uri = String(artifact?.uri || ''), head = el('div', 'artifact-viewer-head'), actions = el('div', 'artifact-viewer-actions');
  head.append(el('strong', '', label), el('span', 'muted-note', `${Number(artifact?.bytes || 0).toLocaleString()} 字节`));
  const copy = button('复制引用', () => { void copyText(uri, copy); }); actions.append(copy); head.append(actions); root.append(head);
  if (openArtifact) actions.append(button('预览', () => { Promise.resolve(openArtifact(artifact)).catch(error => { note.textContent = error.message; }); }));
  const note = el('p', 'muted-note'), output = el('pre', 'activity-panel-output artifact-output'); output.hidden = true;
  if (artifactReader?.supports?.(uri)) {
    const read = button('读取内容', async () => {
      read.disabled = true; note.textContent = '正在读取…';
      try {
        const page = await artifactReader.readPage(runId, uri, offset); if (disposed) return;
        if (typeof page.text !== 'string' || !Number.isSafeInteger(page.next_offset) || page.next_offset < offset || (!page.eof && page.next_offset === offset)) throw new Error('归档分页响应无效。');
        output.hidden = false; output.append(document.createTextNode(page.text)); offset = page.next_offset;
        const capped = offset >= 1024 * 1024; read.textContent = page.eof ? '已读取完整内容' : capped ? '预览已达 1 MiB' : '读取下一页'; read.disabled = page.eof || capped;
        note.textContent = `已读取 ${offset.toLocaleString()} / ${Number(page.total_bytes).toLocaleString()} 字节。`;
      } catch (error) { if (!disposed) { note.textContent = error.message || '读取失败，请重试。'; read.disabled = false; } }
    }); root.append(note, read, output);
  } else { note.textContent = '当前宿主未开放此资源的读取能力。'; root.append(note); }
  return root;
}
export function richContentBlock(blocks, options = {}) {
  const root = el('div', 'rich-content');
  for (const block of Array.isArray(blocks) ? blocks : []) {
    if (block?.type === 'text') { const text = el('div', 'message-text'); void renderMessage(text, block.text || '', options); root.append(text); }
    else if (block?.type === 'image') {
      const source = block.source?.kind === 'base64' && IMAGE_TYPES.has(block.media_type) ? `data:${block.media_type};base64,${block.source.data}` : '';
      root.append(imageView(source, '工具返回的图片', options));
    } else if (block?.type === 'resource') {
      const card = el('div', 'message-resource'); card.append(el('strong', '', block.name || '资源'), el('span', 'muted-note', block.media_type || 'application/octet-stream'));
      if (block.reference) card.append(artifactViewer(block.reference, { ...options, label: block.name || '资源内容' })); root.append(card);
    }
  }
  root.dataset.contentOwned = 'rich'; root.dispose = () => { for (const text of root.querySelectorAll('.message-text')) disposeMessage(text); for (const node of root.querySelectorAll('[data-content-owned]')) node.dispose?.(); }; return root;
}
export function structuredDetailsBlock(details, options = {}) {
  const root = el('div', 'structured-details'); root.dataset.contentOwned = 'details';
  for (const [key, detail] of Object.entries(details || {})) {
    const renderer = uiRegistry.resolve('tool.view', key);
    const rendered = renderer ? uiRegistry.invoke(() => renderer(detail, options)) : null;
    if (rendered instanceof HTMLElement) root.append(rendered);
    else { const item = el('details', 'structured-json'); item.append(el('summary', '', key), el('pre', 'activity-panel-result', JSON.stringify(detail, null, 2))); root.append(item); }
  }
  root.dispose = () => { for (const node of root.querySelectorAll('[data-content-owned]')) node.dispose?.(); };
  return root.childElementCount ? root : null;
}
