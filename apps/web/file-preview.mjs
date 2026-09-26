import { renderMessage, codeBlock, updateCodeBlock, disposeMessage } from './content-renderer.mjs';
import { panelButton } from './pane-icons.mjs';
import { PaneMenu, paneButton, copyPaneText } from './pane-controls.mjs';
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
  constructor({ api, target, path, read, presentation = {} }) {
    this.presentation = presentation;
    this.api = api; this.target = target; this.path = path; this.read = read; this.generation = 0; this.text = ''; this.revision = null; this.next = 0; this.mode = 'preview'; this.zoom = 100; this.urls = new Set(); this.abort = new AbortController();
    this.root = node('section', 'right-file-preview'); this.toolbar = node('div', 'right-file-toolbar file-preview-toolbar');
    this.scrollPositions = new Map();
    this.menuNode = node('div', 'file-actions-menu'); this.menu = new PaneMenu(this.menuNode);
    this.crumbs = node('span', 'file-breadcrumbs', path.split('/').join(' › ')); this.crumbs.dataset.tooltip = path;
    this.toggle = panelButton('源码', () => { this.mode = this.mode === 'preview' ? 'source' : 'preview'; this.render(); this.onPreferenceChange?.(); });
    this.wrap = panelButton('自动换行', () => { const value = this.root.classList.toggle('is-wrapped'); this.wrap.setAttribute('aria-pressed', String(value)); this.wrap.setAttribute('aria-checked', String(value)); this.onPreferenceChange?.(); }); this.wrap.setAttribute('role', 'menuitemcheckbox'); this.wrap.setAttribute('aria-checked', 'false');
    this.copy = panelButton('复制文本', () => { void copyPaneText(this.text, this.copy); }); this.copy.setAttribute('aria-label', '复制文本');
    const pathButton = panelButton('复制路径', () => { void copyPaneText(path, pathButton); });
    this.downloadButton = panelButton('下载文件', () => { this.menu.close(true); void this.download(); });
    for (const button of [this.copy, pathButton, this.downloadButton]) button.setAttribute('role', 'menuitem');
    this.menuNode.append(this.copy, pathButton, this.wrap, this.downloadButton);
    this.refreshButton = paneButton('刷新文件', 'refresh', () => { void this.load(true); });
    this.actionsButton = paneButton('文件操作', 'more', () => this.menu.opened ? this.menu.close(true) : this.menu.open(this.actionsButton)); this.actionsButton.setAttribute('aria-haspopup', 'menu'); this.actionsButton.setAttribute('aria-expanded', 'false');
    this.toolbar.append(this.crumbs, this.toggle, this.refreshButton, this.actionsButton);
    this.content = node('div', 'right-file-content'); this.meta = node('p', 'right-file-meta'); this.meta.setAttribute('role', 'status');
    this.more = panelButton('读取下一页', () => { void this.load(false); }, 'text-button right-file-more'); this.more.hidden = true;
    this.root.append(this.toolbar, this.meta, this.content, this.more, this.menuNode);
  }
  objectUrl(bytes, type) { const url = URL.createObjectURL(new Blob([bytes], { type })); this.urls.add(url); return url; }
  revoke() { for (const url of this.urls) URL.revokeObjectURL(url); this.urls.clear(); }
  disposeContent() { this.binary?.dispose(); this.binary = null; for (const body of this.content.querySelectorAll('.message-text')) disposeMessage(body); for (const part of this.content.querySelectorAll('[data-content-owned]')) part.dispose?.(); }
  close() { this.closed = true; this.generation++; this.abort.abort(); this.menu.dispose(); this.disposeContent(); this.revoke(); }
  async focusLine(line) {
    if (!Number.isSafeInteger(line) || line < 1 || this.closed) return;
    this.pendingLine = line;
    if (this.loading || this.seeking) return;
    this.seeking = true;
    try {
      while (this.pendingLine && !this.closed) {
        const requested = this.pendingLine; this.pendingLine = null;
        for (let count = 0; !this.closed && !this.pendingLine && this.page?.kind === 'text' && this.text.split('\n').length < requested && !this.page.eof && this.next < 1024 * 1024 && count < 16; count++) {
          const previous = this.next; await this.load(false); if (this.next <= previous) break;
        }
        if (this.closed) return;
        if (this.pendingLine) continue;
        if (this.loading) { this.pendingLine = requested; return; }
        if (this.page?.kind !== 'text' || this.text.split('\n').length < requested) { this.meta.textContent = '引用行超出已加载的文本预览范围。'; return; }
        this.mode = 'source'; this.render();
        // Mode rendering restores its reading position on the next frame. A new
        // explicit navigation takes precedence, and only scrolls this document.
        await new Promise(resolve => requestAnimationFrame(resolve));
        if (this.closed) return;
        if (this.pendingLine) continue;
        const marker = this.content.querySelector(`[data-line="${requested}"]`);
        for (const previous of this.content.querySelectorAll('.is-referenced-line')) previous.classList.remove('is-referenced-line');
        if (marker) {
          marker.classList.add('is-referenced-line');
          const row = marker.getBoundingClientRect(), viewport = this.content.getBoundingClientRect();
          this.content.scrollTop += row.top - viewport.top - (viewport.height - row.height) / 2;
        }
        this.onPreferenceChange?.();
      }
    } finally { this.seeking = false; }
  }
  async load(reset = false) {
    if (this.closed || (this.loading && !reset)) return;
    if (reset) { this.abort.abort(); this.abort = new AbortController(); }
    const generation = ++this.generation; this.loading = true; this.more.disabled = true; this.meta.textContent = '正在读取…'; this.root.setAttribute('aria-busy', 'true');
    try {
      const page = await this.read(this.target, this.path, { offset: reset ? 0 : this.next, revision: reset ? undefined : this.revision, signal: this.abort.signal });
      if (generation !== this.generation) return;
      const candidate = page.kind === 'binary' ? await this.prepareBinary(page, generation, this.abort.signal) : null;
      if (generation !== this.generation || this.closed) { candidate?.dispose(); return; }
      if (reset || candidate) {
        this.text = ''; this.next = 0; this.disposeContent(); this.revoke();
        for (const child of [...this.content.childNodes]) if (child !== candidate?.node) child.remove();
        this.renderKind = null; this.sourceBlock = null; this.gutter = null; this.markdownBody = null;
      }
      this.revision = page.revision; this.next = page.next_offset; this.page = page;
      if (page.kind === 'text') this.text += page.text;
      if (candidate) {
        this.binary = candidate; candidate.commit(); this.renderKind = 'binary'; this.content.dataset.previewKind = 'binary';
        this.toggle.hidden = this.copy.hidden = this.wrap.hidden = true;
      } else this.render();
      this.more.hidden = page.eof || page.kind !== 'text' || this.next >= 1024 * 1024;
      this.meta.textContent = !page.eof && page.kind === 'text' ? `已读取 ${page.next_offset.toLocaleString()} / ${page.bytes.toLocaleString()} 字节` : '';
      this.crumbs.dataset.tooltip = `${this.path} · ${page.bytes.toLocaleString()} 字节`;
      if (!page.eof && this.next >= 1024 * 1024) this.meta.textContent += ' · 预览已达 1 MiB，可下载完整文件';
    } catch (error) { if (generation === this.generation && error.name !== 'AbortError') { this.meta.textContent = (reset && this.page ? '刷新失败，仍显示先前内容。' : '') + error.message; this.more.hidden = true;
        const retry = panelButton('重试', () => { void this.load(reset); }); this.meta.append(retry); } }
    finally { if (generation === this.generation) { this.loading = false; this.more.disabled = false; this.root.setAttribute('aria-busy', 'false'); if (this.pendingLine) { const line = this.pendingLine; this.pendingLine = null; void this.focusLine(line); } } }
  }
  render() {
    if (!this.page) return;
    const extension = ext(this.path), previewable = ['md', 'markdown', 'html', 'htm', 'svg'].includes(extension);
    this.toggle.hidden = !previewable || this.page.kind !== 'text'; this.toggle.textContent = this.mode === 'preview' ? '源码' : '预览';
    this.copy.hidden = this.wrap.hidden = this.page.kind !== 'text';
    this.copy.textContent = this.page.eof ? '复制文本' : '复制已加载文本'; this.copy.setAttribute('aria-label', this.copy.textContent);
    this.wrap.setAttribute('aria-checked', String(this.root.classList.contains('is-wrapped')));
    const renderKind = this.page.kind === 'text' ? (previewable && this.mode === 'preview' ? extension : 'source') : this.page.kind;
    if (renderKind === this.renderKind && renderKind === 'source' && this.sourceBlock) {
      updateCodeBlock(this.sourceBlock, this.text, languages[extension] || extension);
      this.sourceBlock.classList.remove('is-collapsed'); this.updateLineNumbers(); return;
    }
    if (renderKind === this.renderKind && ['md', 'markdown'].includes(renderKind) && this.markdownBody) {
      void renderMessage(this.markdownBody, this.text, this.presentation); return;
    }
    if (this.renderKind) this.scrollPositions.set(this.renderKind, { top: this.content.scrollTop, left: this.content.scrollLeft });
    this.disposeContent(); this.content.replaceChildren(); this.renderKind = renderKind;
    this.content.dataset.previewKind = renderKind;
    requestAnimationFrame(() => { if (this.closed || this.renderKind !== renderKind) return; const at = this.scrollPositions.get(renderKind); this.content.scrollTop = at?.top || 0; this.content.scrollLeft = at?.left || 0; });
    if (this.page.kind === 'image') {
      const image = node('img', 'workspace-image'); image.alt = this.path; image.src = `data:${this.page.media_type};base64,${this.page.base64}`;
      image.addEventListener('error', () => { this.meta.textContent = '图片无法解码。'; });
      const controls = node('div', 'image-controls');
      for (const [label, zoom] of [['50%', 50], ['适应', 100], ['150%', 150], ['200%', 200]]) controls.append(panelButton(label, () => { image.className = `workspace-image image-zoom-${zoom}`; }));
      this.content.append(controls, image); return;
    }
    if (this.page.kind !== 'text') { this.content.append(node('p', 'workspace-empty', '正在检查文件预览…')); return; }
    if (previewable && this.mode === 'preview') {
      if (['md', 'markdown'].includes(extension)) { const body = node('article', 'workspace-markdown-preview message-text'); this.markdownBody = body; void renderMessage(body, this.text, this.presentation); this.content.append(body); }
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
    this.sourceBlock = codeBlock(this.text, languages[extension] || extension); code.append(this.sourceBlock);
    code.querySelector('.message-code-block')?.classList.remove('is-collapsed');
    code.querySelector('.message-code-head')?.remove();
    const gutter = node('div', 'file-line-numbers'); gutter.setAttribute('aria-hidden', 'true');
    this.gutter = gutter; this.updateLineNumbers(); code.prepend(gutter);
    this.content.append(code);
    if (!this.text) this.content.append(node('p', 'workspace-empty', '文件为空。'));
  }
  updateLineNumbers() {
    if (!this.gutter) return;
    const count = this.text.split('\n').length;
    while (this.gutter.children.length > count) this.gutter.lastChild.remove();
    const fragment = document.createDocumentFragment();
    for (let index = this.gutter.children.length; index < count; index++) { const line = node('div', '', String(index + 1)); line.dataset.line = String(index + 1); fragment.append(line); }
    this.gutter.append(fragment);
  }
  async prepareBinary(page, generation, signal) {
    // A replacement loads beside the confirmed view. Its iframe never changes parent;
    // only visibility is committed after bytes/Office rendering succeed.
    const holder = node('div', 'binary-document-preview is-staging'); holder.inert = true; holder.setAttribute('aria-hidden', 'true');
    const urls = new Set(); let ended = false, cancelRender = () => {};
    const aborted = () => { cancelRender(); dispose(); };
    const dispose = () => {
      if (ended) return; ended = true; signal.removeEventListener('abort', aborted); cancelRender();
      for (const media of holder.querySelectorAll('audio,video')) { media.pause(); media.removeAttribute('src'); media.load(); }
      holder.remove(); for (const url of urls) URL.revokeObjectURL(url); urls.clear();
    };
    const live = () => { if (ended || signal.aborted || generation !== this.generation || this.closed) throw new DOMException('预览已关闭。', 'AbortError'); };
    live(); signal.addEventListener('abort', aborted, { once: true }); this.content.append(holder);
    try {
      const kind = ext(this.path), office = ['docx', 'xlsx', 'pptx'].includes(kind), type = mime[kind];
      if (!office && !type) holder.append(node('p', 'workspace-empty', '此格式暂未安装预览器，可下载后打开。'));
      else {
        if (office && page.bytes > 8 * 1024 * 1024) throw new Error('Office 文件超过 8 MiB 预览限制，可下载后打开。');
        const [bytes, wasm] = await Promise.all([this.api.bytes(this.target, this.path, page.revision, signal), kind === 'xlsx' ? wasmBytes() : null]);
        live();
        if (office) {
          const nonce = crypto.randomUUID(), frame = document.createElement('iframe'); frame.className = 'static-document-preview office-document-preview';
          frame.title = this.path; frame.referrerPolicy = 'no-referrer'; frame.setAttribute('sandbox', 'allow-scripts'); frame.src = `/apps/web/document-preview.html#${nonce}`;
          await new Promise((resolve, reject) => {
            let settled = false;
            const finish = error => {
              if (settled) return; settled = true; clearTimeout(timer); window.removeEventListener('message', receive); cancelRender = () => {};
              error ? reject(error) : resolve();
            };
            const receive = event => {
              if (event.source !== frame.contentWindow || event.data?.nonce !== nonce || generation !== this.generation || ended) return;
              if (event.data.type === 'ready') frame.contentWindow.postMessage({ type: 'document', nonce, kind, buffer: bytes.buffer, wasm, name: this.path, dark: document.documentElement.dataset.theme === 'dark' }, '*');
              else if (event.data.type === 'rendered') { frame.dataset.rendered = 'true'; finish(); }
              else if (event.data.type === 'error') finish(new Error(String(event.data.value).slice(0, 300)));
            };
            const timer = setTimeout(() => finish(new Error('文档预览超时，可下载后打开。')), 40000);
            cancelRender = () => finish(new DOMException('预览已关闭。', 'AbortError'));
            window.addEventListener('message', receive); holder.append(frame);
          });
        } else {
          if (kind === 'pdf' && new TextDecoder().decode(bytes.slice(0, 5)) !== '%PDF-') throw new Error('文件不是有效的 PDF。');
          const url = URL.createObjectURL(new Blob([bytes], { type })); urls.add(url);
          if (kind === 'pdf') {
            const frame = document.createElement('iframe'); frame.title = this.path; frame.className = 'static-document-preview'; frame.src = url; holder.append(frame);
          } else {
            const media = document.createElement(type.startsWith('audio') ? 'audio' : 'video'); media.controls = true; media.preload = 'metadata'; media.src = url;
            media.addEventListener('error', () => { if (!ended && generation === this.generation) this.meta.textContent = '浏览器不支持此媒体编码，可下载后打开。'; }); holder.append(media);
          }
        }
      }
      live(); return { node: holder, dispose, commit: () => {
        signal.removeEventListener('abort', aborted); holder.classList.remove('is-staging'); holder.inert = false; holder.removeAttribute('aria-hidden');
      } };
    } catch (error) { dispose(); throw error; }
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
