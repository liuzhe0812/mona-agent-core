import { renderMessage, codeBlock } from './content-renderer.mjs';
import { panelButton, copyPanelText } from './pane-icons.mjs';
const node = (tag, className, text = '') => { const n = document.createElement(tag); n.className = className; n.textContent = text; return n; };
const ext = path => path.split('.').at(-1).toLowerCase();
const languages = { rs: 'rust', js: 'javascript', mjs: 'javascript', ts: 'typescript', py: 'python', sh: 'shell', ps1: 'powershell', html: 'html', json: 'json', css: 'css', diff: 'diff' };
const mime = { pdf: 'application/pdf', mp3: 'audio/mpeg', wav: 'audio/wav', ogg: 'audio/ogg', m4a: 'audio/mp4', mp4: 'video/mp4', webm: 'video/webm' };
let sheetsWasm;
async function wasmBytes() {
  sheetsWasm ||= fetch('/apps/web/vendor/office/sheets.wasm', { credentials: 'omit', redirect: 'error', signal: AbortSignal.timeout(30000) }).then(response => { if (!response.ok) throw new Error('工作表组件加载失败。'); return response.arrayBuffer(); }).catch(error => { sheetsWasm = null; throw error; });
  return sheetsWasm;
}
export class FilePreview {
  constructor({ api, target, path, read }) {
    this.api = api; this.target = target; this.path = path; this.read = read; this.generation = 0; this.text = ''; this.revision = null; this.next = 0; this.mode = 'preview'; this.zoom = 100; this.urls = new Set(); this.abort = new AbortController();
    this.root = node('section', 'right-file-preview'); this.toolbar = node('div', 'right-file-toolbar');
    this.crumbs = node('span', 'file-breadcrumbs', path.split('/').join(' › ')); this.crumbs.dataset.tooltip = path;
    this.toggle = panelButton('源码', () => { this.mode = this.mode === 'preview' ? 'source' : 'preview'; this.render(); });
    this.wrap = panelButton('自动换行', () => { this.root.classList.toggle('is-wrapped'); this.wrap.setAttribute('aria-pressed', String(this.root.classList.contains('is-wrapped'))); });
    this.copy = panelButton('复制', () => { void copyPanelText(this.text, this.copy); });
    const pathButton = panelButton('复制路径', () => { void copyPanelText(path, pathButton); });
    this.downloadButton = panelButton('下载', () => { void this.download(); });
    this.refreshButton = panelButton('刷新', () => { void this.load(true); });
    this.toolbar.append(this.crumbs, this.toggle, this.wrap, this.copy, pathButton, this.downloadButton, this.refreshButton);
    this.content = node('div', 'right-file-content'); this.meta = node('p', 'right-file-meta'); this.meta.setAttribute('role', 'status');
    this.more = panelButton('读取下一页', () => { void this.load(false); }, 'text-button right-file-more'); this.more.hidden = true;
    this.root.append(this.toolbar, this.meta, this.content, this.more);
  }
  objectUrl(bytes, type) { const url = URL.createObjectURL(new Blob([bytes], { type })); this.urls.add(url); return url; }
  revoke() { this.cleanupDocument?.(); this.cleanupDocument = null; for (const url of this.urls) URL.revokeObjectURL(url); this.urls.clear(); }
  close() { this.generation++; this.abort.abort(); this.revoke(); }
  async load(reset = false) {
    if (this.loading && !reset) return;
    if (reset) { this.abort.abort(); this.abort = new AbortController(); this.revision = null; this.text = ''; this.next = 0; this.revoke(); this.content.replaceChildren(); }
    const generation = ++this.generation; this.loading = true; this.more.disabled = true; this.meta.textContent = '正在读取…';
    try {
      const page = await this.read(this.target, this.path, { offset: reset ? 0 : this.next, revision: reset ? undefined : this.revision, signal: this.abort.signal });
      if (generation !== this.generation) return;
      this.revision = page.revision; this.next = page.next_offset; this.page = page;
      if (page.kind === 'text') this.text += page.text;
      this.render();
      this.more.hidden = page.eof || page.kind !== 'text' || this.next >= 1024 * 1024;
      this.meta.textContent = `${page.bytes.toLocaleString()} 字节${!page.eof ? ` · 已读取 ${page.next_offset.toLocaleString()} 字节` : ''}`;
      if (!page.eof && this.next >= 1024 * 1024) this.meta.textContent += ' · 预览已达 1 MiB，可下载完整文件';
      if (page.kind === 'binary') await this.renderBinary(generation);
    } catch (error) { if (generation === this.generation && error.name !== 'AbortError') { this.meta.textContent = error.message; this.more.hidden = true; } }
    finally { if (generation === this.generation) { this.loading = false; this.more.disabled = false; } }
  }
  render() {
    if (!this.page) return;
    const extension = ext(this.path), previewable = ['md', 'markdown', 'html', 'htm', 'svg'].includes(extension);
    this.toggle.hidden = !previewable || this.page.kind !== 'text'; this.toggle.textContent = this.mode === 'preview' ? '源码' : '预览';
    this.copy.hidden = this.wrap.hidden = this.page.kind !== 'text'; this.content.replaceChildren();
    if (this.page.kind === 'image') {
      const image = node('img', 'workspace-image'); image.alt = this.path; image.src = `data:${this.page.media_type};base64,${this.page.base64}`;
      image.addEventListener('error', () => { this.meta.textContent = '图片无法解码。'; });
      const controls = node('div', 'image-controls');
      for (const [label, zoom] of [['50%', 50], ['适应', 100], ['150%', 150], ['200%', 200]]) controls.append(panelButton(label, () => { image.className = `workspace-image image-zoom-${zoom}`; }));
      this.content.append(controls, image); return;
    }
    if (this.page.kind !== 'text') { this.content.append(node('p', 'workspace-empty', '正在检查文件预览…')); return; }
    if (previewable && this.mode === 'preview') {
      if (['md', 'markdown'].includes(extension)) { const body = node('article', 'workspace-markdown-preview message-text'); renderMessage(body, this.text); this.content.append(body); }
      else {
        const frame = document.createElement('iframe'); frame.className = 'static-document-preview'; frame.title = this.path; frame.setAttribute('sandbox', ''); frame.referrerPolicy = 'no-referrer';
        // Script-free, network-free local rendering. A model cannot relax this parent-supplied CSP.
        frame.srcdoc = `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; base-uri 'none'; form-action 'none'"><meta charset="utf-8">` + this.text;
        this.content.append(frame);
      }
      return;
    }
    const code = node('div', 'workspace-source-preview message-text');
    // Use the same highlighter as conversation code, not a second Markdown implementation.
    code.append(codeBlock(this.text, languages[extension] || extension));
    code.querySelector('.message-code-block')?.classList.remove('is-collapsed');
    code.querySelector('.message-code-head')?.remove();
    const gutter = node('div', 'file-line-numbers'); gutter.setAttribute('aria-hidden', 'true');
    this.text.split('\n').forEach((_, index) => gutter.append(node('div', '', String(index + 1)))); code.prepend(gutter);
    this.content.append(code);
  }
  async renderBinary(generation) {
    const extension = ext(this.path), type = mime[extension];
    if (['docx', 'xlsx', 'pptx'].includes(extension)) { await this.renderOffice(extension, generation); return; }
    if (!type) { this.content.replaceChildren(node('p', 'workspace-empty', '此格式暂未安装预览器，可下载后打开。')); return; }
    const bytes = await this.api.bytes(this.target, this.path, this.revision, this.abort.signal);
    if (generation !== this.generation) return;
    if (extension === 'pdf' && new TextDecoder().decode(bytes.slice(0, 5)) !== '%PDF-') throw new Error('文件不是有效的 PDF。');
    const url = this.objectUrl(bytes, type);
    if (extension === 'pdf') {
      const frame = document.createElement('iframe'); frame.title = this.path; frame.className = 'static-document-preview'; frame.src = url; this.content.replaceChildren(frame);
    } else {
      const media = document.createElement(type.startsWith('audio') ? 'audio' : 'video'); media.controls = true; media.preload = 'metadata'; media.src = url;
      media.addEventListener('error', () => { this.meta.textContent = '浏览器不支持此媒体编码，可下载后打开。'; }); this.content.replaceChildren(media);
    }
  }
  async renderOffice(kind, generation) {
    if (this.page.bytes > 8 * 1024 * 1024) throw new Error('Office 文件超过 8 MiB 预览限制，可下载后打开。');
    const [bytes, wasm] = await Promise.all([this.api.bytes(this.target, this.path, this.revision, this.abort.signal), kind === 'xlsx' ? wasmBytes() : null]);
    if (generation !== this.generation) return;
    const nonce = crypto.randomUUID(); const frame = document.createElement('iframe'); frame.className = 'static-document-preview office-document-preview';
    frame.title = this.path; frame.referrerPolicy = 'no-referrer'; frame.setAttribute('sandbox', 'allow-scripts'); frame.src = `/apps/web/document-preview.html#${nonce}`;
    await new Promise((resolve, reject) => {
      let settled = false;
      const cleanup = () => { clearTimeout(timer); window.removeEventListener('message', receive); };
      const finish = error => { if (settled) return; settled = true; cleanup(); error ? reject(error) : resolve(); };
      const receive = event => {
        if (event.source !== frame.contentWindow || event.data?.nonce !== nonce || generation !== this.generation) return;
        if (event.data.type === 'ready') frame.contentWindow.postMessage({ type: 'document', nonce, kind, buffer: bytes.buffer, wasm, name: this.path, dark: document.documentElement.dataset.theme === 'dark' }, '*');
        else if (event.data.type === 'rendered') { frame.dataset.rendered = 'true'; finish(); }
        else if (event.data.type === 'error') finish(new Error(String(event.data.value).slice(0, 300)));
      };
      const timer = setTimeout(() => { frame.remove(); finish(new Error('文档预览超时，可下载后打开。')); }, 40000);
      this.cleanupDocument = () => { frame.remove(); finish(new DOMException('预览已关闭。', 'AbortError')); };
      window.addEventListener('message', receive); this.content.replaceChildren(frame);
    });
  }
  async download() {
    const generation = this.generation; this.downloadButton.disabled = true;
    try {
      const bytes = await this.api.bytes(this.target, this.path, this.revision, this.abort.signal);
      if (generation !== this.generation) return;
      const url = this.objectUrl(bytes, 'application/octet-stream'), anchor = document.createElement('a'); anchor.href = url; anchor.download = this.path.split('/').at(-1); anchor.click();
      setTimeout(() => { URL.revokeObjectURL(url); this.urls.delete(url); }, 5000);
    } catch (error) { if (error.name !== 'AbortError') this.meta.textContent = error.message; }
    finally { this.downloadButton.disabled = false; }
  }
}
