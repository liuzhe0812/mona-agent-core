import { purify } from './vendor/content/core.mjs';
import { element, contentAction, copyText, previewDialog, closeContentPreviews, zoomPreview, downloadContent } from './content-dom.mjs';
import { paneIcon } from './pane-icons.mjs';
const queue = []; let service, busy = false, next = 0;
const themeViews = new Set(); let themeObserver;
function observeTheme(callback) {
  if (!themeObserver) { themeObserver = new MutationObserver(() => { for (const view of themeViews) view(); }); themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] }); }
  themeViews.add(callback); return () => { themeViews.delete(callback); if (!themeViews.size) { themeObserver?.disconnect(); themeObserver = null; } };
}
function resetService() { service?.frame.remove(); service?.stop(); service = null; }
function init() {
  if (service) return service;
  const nonce = crypto.randomUUID(), frame = element('iframe', 'mona-diagram-engine'); frame.setAttribute('sandbox', 'allow-scripts'); frame.setAttribute('aria-hidden', 'true'); frame.tabIndex = -1; frame.title = '图表排版'; frame.src = `/apps/web/diagram-renderer.html#${nonce}`;
  let readyResolve, readyReject, current; const ready = new Promise((yes, no) => { readyResolve = yes; readyReject = no; });
  const timeout = setTimeout(() => readyReject(new Error('图表组件加载超时，请重试。')), 15000);
  const receive = event => {
    if (event.source !== frame.contentWindow || event.data?.nonce !== nonce) return;
    const data = event.data;
    if (data.type === 'ready') { clearTimeout(timeout); readyResolve(); }
    if (current && data.id === current.id) { const call = current; current = null; clearTimeout(call.timer); data.type === 'result' ? call.resolve(data.svg) : call.reject(new Error(data.message || '图表解析失败。')); }
  };
  window.addEventListener('message', receive); document.body.append(frame);
  service = { frame, ready, stop() { clearTimeout(timeout); window.removeEventListener('message', receive); if (current) { clearTimeout(current.timer); current.reject(new Error('图表排版已停止。')); current = null; } },
    async render(source, dark) {
      await ready;
      return new Promise((resolve, reject) => { const id = ++next; current = { id, resolve, reject, timer: setTimeout(() => { current = null; reject(new Error('图表排版超时，保留源码。')); resetService(); }, 8000) };
        frame.contentWindow.postMessage({ type: 'render', nonce, id, source, dark }, '*'); });
    } };
  return service;
}
async function drain() {
  if (busy) return; busy = true;
  try { while (queue.length) {
    const task = queue.shift(); if (task.signal.aborted) { task.reject(new DOMException('已取消', 'AbortError')); continue; }
    try { task.resolve(await init().render(task.source, task.dark)); } catch (error) { resetService(); task.reject(error); }
  } } finally { busy = false; }
}
function render(source, signal) {
  if (source.length > 32768) return Promise.reject(new Error('图表超过 32 KiB 上限，可复制源码查看。'));
  if (queue.length >= 16) return Promise.reject(new Error('正在排版其他图表，请稍后重试。'));
  return new Promise((resolve, reject) => { queue.push({ source, signal, dark: document.documentElement.dataset.theme === 'dark', resolve, reject }); void drain(); });
}
export function diagramView(source, { streaming = false } = {}) {
  const root = element('section', 'message-diagram'), header = element('div', 'message-code-head'), label = element('span', 'message-code-language', 'mermaid'), actions = element('div', 'message-code-actions');
  const code = element('pre', 'message-code', source), body = element('div', 'message-mermaid-wrap'), note = element('p', 'muted-note');
  const controller = new AbortController(); let url, loading = false, ready = false, complete = !streaming, visible = typeof IntersectionObserver === 'undefined', renderedTheme;
  const visibility = typeof IntersectionObserver === 'undefined' ? null : new IntersectionObserver(entries => {
    visible = entries.some(entry => entry.isIntersecting); if (visible && complete && (!ready || renderedTheme !== document.documentElement.dataset.theme)) void load();
  }, { rootMargin: '800px' }); visibility?.observe(root);
  const stopTheme = observeTheme(() => { if (complete && visible && renderedTheme !== document.documentElement.dataset.theme) void load(); });
  const copy = contentAction('复制图表源码', 'copy', () => { void copyText(source, copy); });
  const mode = contentAction('查看源码', 'file', () => { const showCode = code.hidden; code.hidden = !showCode; body.hidden = showCode; mode.setAttribute('aria-label', showCode ? '查看图表' : '查看源码'); mode.dataset.tooltip = mode.getAttribute('aria-label'); }); mode.hidden = true;
  const large = contentAction('放大图表', 'expand', () => previewDialog('图表', zoomPreview(body.firstChild.cloneNode(true)), { owner: root })); large.hidden = true;
  const save = contentAction('下载 SVG', 'download', () => { if (root.svg) downloadContent(root.svg, 'image/svg+xml', 'diagram.svg'); }); save.hidden = true;
  const retry = contentAction('重新渲染', 'refresh', () => { void load(); }); retry.hidden = true;
  actions.append(mode, copy, large, save, retry); header.append(paneIcon('image'), label, actions); root.append(header, note, body, code);
  async function load() {
    if (loading || controller.signal.aborted) return;
    loading = true; retry.hidden = true; note.textContent = '正在排版图表…';
    const selectedSource = ready && !code.hidden, theme = document.documentElement.dataset.theme;
    try {
      const svg = await render(source, controller.signal); if (controller.signal.aborted) return;
      if (typeof svg !== 'string' || svg.length > 2 * 1024 * 1024) throw new Error('图表输出超限。');
      // SVG is displayed in image mode, never injected into the application DOM.
      const clean = purify.sanitize(svg, { USE_PROFILES: { svg: true, svgFilters: true }, FORBID_TAGS: ['script', 'foreignObject', 'a', 'image', 'use'], FORBID_ATTR: ['onload', 'onclick'] });
      const doc = new DOMParser().parseFromString(clean, 'image/svg+xml');
      if (doc.querySelector('parsererror')) throw new Error('图表输出无效。');
      for (const style of doc.querySelectorAll('style')) if (/@import|@font-face|url\(\s*[^#]/i.test(style.textContent)) style.remove();
      for (const node of doc.querySelectorAll('[style]')) if (/url\(\s*[^#]|expression\(/i.test(node.getAttribute('style'))) node.removeAttribute('style');
      root.svg = new XMLSerializer().serializeToString(doc.documentElement);
      if (url) URL.revokeObjectURL(url); url = URL.createObjectURL(new Blob([root.svg], { type: 'image/svg+xml' }));
      const image = element('img', 'message-mermaid'); image.alt = 'Mermaid 图表'; image.src = url; image.addEventListener('dblclick', () => large.click());
      body.replaceChildren(image); code.hidden = !selectedSource; body.hidden = selectedSource; note.textContent = ''; ready = true; renderedTheme = theme; mode.hidden = large.hidden = save.hidden = false; mode.setAttribute('aria-label', selectedSource ? '查看图表' : '查看源码'); mode.dataset.tooltip = mode.getAttribute('aria-label');
      root.dispatchEvent(new CustomEvent('mona:content-resized', { bubbles: true }));
    } catch (error) { if (error.name !== 'AbortError') { note.textContent = '图表未能显示，源码已保留：' + error.message; code.hidden = false; body.hidden = true; retry.hidden = false; } }
    finally { loading = false; if (ready && visible && renderedTheme === theme && renderedTheme !== document.documentElement.dataset.theme) void load(); }
  }
  root.dispose = () => { closeContentPreviews(root); controller.abort(); visibility?.disconnect(); stopTheme(); if (url) URL.revokeObjectURL(url); };
  root.complete = () => { complete = true; if (!ready && !loading && visible) void load(); };
  body.hidden = true;
  if (streaming) note.textContent = '图表生成中，结束后显示。';
  else if (visible) void load();
  else note.textContent = '进入阅读区域后显示图表。';
  return root;
}
