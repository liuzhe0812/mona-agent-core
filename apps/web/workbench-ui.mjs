import { panelButton, copyPanelText } from './pane-icons.mjs';
import { diffContent } from './diff-view.mjs';
const node = (tag, className, text = '') => { const n = document.createElement(tag); n.className = className; n.textContent = text; return n; };
let terminalAssets;
function loadScript(path) { return new Promise((resolve, reject) => {
  const script = document.createElement('script'); script.src = path;
  const timer = setTimeout(() => { script.remove(); reject(new Error('终端资源加载超时，请关闭标签后重新打开。')); }, 20000);
  script.onload = () => { clearTimeout(timer); resolve(); };
  script.onerror = () => { clearTimeout(timer); script.remove(); reject(new Error('终端资源加载失败。')); }; document.head.append(script);
}); }
function terminalLibrary() {
  if (!terminalAssets) terminalAssets = (async () => {
    const link = document.createElement('link'); link.rel = 'stylesheet'; link.href = '/apps/web/vendor/xterm/xterm.css'; document.head.append(link);
    await loadScript('/apps/web/vendor/xterm/xterm.js'); await loadScript('/apps/web/vendor/xterm/addon-fit.js');
  })().catch(error => { terminalAssets = null; throw error; });
  return terminalAssets;
}
export class WorkbenchUI {
  constructor({ pane, workspace, header }) {
    this.pane = pane; this.workspace = workspace; this.header = header; this.serial = 0;
    this.events = new AbortController();
    header.addEventListener('click', () => this.toggleTerminal(), { signal: this.events.signal });
    pane.shell.addEventListener('mona:right-pane-render', () => this.renderHeader(), { signal: this.events.signal });
    document.addEventListener('keydown', event => {
      if (event.defaultPrevented || !(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey
        || event.key.toLowerCase() !== 'j' || header.hidden || document.querySelector('dialog[open]')
        || header.closest('[hidden], [inert]')) return;
      event.preventDefault(); this.toggleTerminal();
    }, { signal: this.events.signal });
    for (const [id, label, run] of [['git', '审查', () => this.openReview()], ['terminal', '终端', () => this.openTerminal()]]) {
      pane.registerAction({ id, label, available: () => Boolean(workspace.available && this.target() && workspace.features?.[id === 'git' ? 'review' : id]), run });
    }
    this.renderHeader();
  }
  dispose() { this.events.abort(); this.header.hidden = true; }
  renderHeader() {
    const available = Boolean(this.workspace.available && this.workspace.features?.terminal && this.target());
    this.header.hidden = !available;
    const active = this.pane.tabs.get(`${this.pane.scope}\0${this.pane.active.get(this.pane.scope)}`);
    const shown = available && this.pane.open && !this.pane.pane.hidden && active?.kind === 'terminal';
    this.header.setAttribute('aria-pressed', String(shown));
  }
  toggleTerminal() {
    if (this.header.hidden) return;
    const scope = this.pane.scope;
    const active = this.pane.tabs.get(`${scope}\0${this.pane.active.get(scope)}`);
    if (this.pane.open && active?.kind === 'terminal') { this.pane.setOpen(false, true); return; }
    const existing = this.pane.scopeTabs().find(tab => tab.kind === 'terminal');
    if (existing) this.pane.activate(existing.id);
    else void this.openTerminal();
  }
  target() {
    const scope = this.pane.scope, split = scope.indexOf(':');
    return split > 0 && ['session', 'project'].includes(scope.slice(0, split)) ? { kind: scope.slice(0, split), id: scope.slice(split + 1) } : null;
  }
  openReview() {
    const target = this.target(); if (!target) return;
    const scope = this.pane.scope, id = 'git'; if (this.pane.hasTab(id, scope)) { this.pane.activate(id, scope); return; }
    const root = node('section', 'review-pane'); const toolbar = node('div', 'right-file-toolbar'); const status = node('span', 'file-meta', '正在读取…');
    const body = node('div', 'review-body'), list = node('div', 'review-files'), detail = node('div', 'review-detail'); body.append(list);
    const source = document.createElement('select'); source.setAttribute('aria-label', '差异来源');
    for (const [value, label] of [['working', '工作区变更'], ['staged', '已暂存变更']]) { const option = document.createElement('option'); option.value = value; option.textContent = label; source.append(option); }
    let controller = new AbortController(), generation = 0, currentPath = '', diffText = '', split = false;
    const renderDiff = () => { detail.replaceChildren(diffContent(diffText, { split })); };
    const pendingDiff = () => { currentPath = ''; diffText = ''; open.disabled = copy.disabled = layout.disabled = true; };
    const diff = async (path, staged) => {
      const mine = ++generation; pendingDiff(); status.textContent = '正在读取差异…';
      try {
        const result = await this.workspace.api.diff(target, path, staged, controller.signal);
        if (mine !== generation) return;
        if (typeof result.text !== 'string' || result.text.length > 1024 * 1024) throw new Error('差异响应无效。');
        currentPath = path; diffText = result.text || '没有可显示的文本差异。'; open.disabled = copy.disabled = layout.disabled = false; renderDiff(); status.textContent = `${path}${staged ? ' · 已暂存' : ' · 工作区'}${result.truncated ? ' · 预览已截断' : ''}`;
      } catch (error) { if (mine === generation && error.name !== 'AbortError') status.textContent = error.message; }
    };
    const refresh = async () => {
      controller.abort(); controller = new AbortController(); const mine = ++generation; pendingDiff(); status.textContent = '正在读取变更…';
      try {
        const result = await this.workspace.api.review(target, controller.signal); if (mine !== generation) return;
        if (!Array.isArray(result.entries) || result.entries.length > 1000) throw new Error('变更列表无效。');
        list.replaceChildren(); detail.replaceChildren(); currentPath = '';
        status.textContent = result.available ? result.branch || '分离 HEAD' : result.message;
        if (!result.available) { detail.textContent = result.message; list.append(detail); return; }
        for (const [heading, staged] of [['已暂存', true], ['工作区', false]]) {
          if (staged !== (source.value === 'staged')) continue;
          const entries = result.entries.filter(e => staged ? ![' ', '?'].includes(e.index) : e.working !== ' ');
          if (!entries.length) continue;
          list.append(node('strong', 'review-group', `${heading} (${entries.length})`));
          for (const entry of entries) {
            const item = panelButton('', () => {
              if (detail.previousElementSibling === item) { generation++; detail.remove(); pendingDiff(); item.setAttribute('aria-expanded', 'false'); return; }
              for (const peer of list.querySelectorAll('.review-entry')) peer.setAttribute('aria-expanded', 'false');
              item.setAttribute('aria-expanded', 'true'); detail.textContent = '正在读取差异…'; item.after(detail); void diff(entry.path, staged);
            }, 'review-entry'); item.setAttribute('aria-expanded', 'false');
            item.append(node('span', '', entry.path), node('small', '', staged ? entry.index : entry.working)); list.append(item);
          }
        }
        if (!list.childElementCount) { detail.textContent = '此来源没有未提交的变更。'; list.append(detail); }
      } catch (error) { if (mine === generation && error.name !== 'AbortError') status.textContent = error.message; }
    };
    const layout = panelButton('分栏', () => { split = !split; layout.textContent = split ? '合并' : '分栏'; renderDiff(); });
    const copy = panelButton('复制差异', () => { void copyPanelText(diffText, copy); });
    const open = panelButton('打开文件', () => { if (currentPath) this.workspace.openFile(currentPath, target, scope); });
    pendingDiff(); source.addEventListener('change', () => { void refresh(); });
    toolbar.append(source, status, layout, copy, open, panelButton('刷新', () => { void refresh(); })); root.append(toolbar, body);
    this.pane.openTab({ id, title: '审查', kind: 'git', node: root, scope, onClose: () => { generation++; controller.abort(); }, reopen: () => this.openReview() });
    void refresh();
  }
  async openTerminal() {
    const target = this.target(), scope = this.pane.scope; if (!target) return;
    const api = this.workspace.api.fork(), id = crypto.randomUUID(), title = `终端 ${++this.serial}`;
    const root = node('section', 'terminal-pane'), status = node('p', 'terminal-status', '正在启动终端…'), viewport = node('div', 'terminal-viewport');
    const resume = panelButton('重新连接输出', () => { if (!disposed && !polling) { resume.hidden = true; void poll(); } }); resume.hidden = true;
    root.append(viewport, status, resume); status.setAttribute('role', 'status');
    const controller = new AbortController(); let term, fit, observer, timer, disposed = false, started = false, openAttempted = false, after = 0, sequence = 0, faulted = false, writeUncertain = false, polling = false, resizeTimer;
    let writes = Promise.resolve(), pendingInputBytes = 0;
    const close = async () => {
      disposed = true; clearTimeout(timer); clearTimeout(resizeTimer); controller.abort(); observer?.disconnect(); themeObserver.disconnect(); term?.dispose();
      if (started) { try { await api.terminalClose(target, id); } finally { api.clear(); } }
      else if (!openAttempted) api.clear();
    };
    const applyTheme = () => { if (!term) return; const css = getComputedStyle(document.documentElement); term.options.theme = { background: css.getPropertyValue('--surface-elevated').trim(), foreground: css.getPropertyValue('--text-primary').trim(), cursor: css.getPropertyValue('--text-primary').trim() }; };
    const themeObserver = new MutationObserver(applyTheme); themeObserver.observe(document.documentElement, { attributes: true });
    const tab = this.pane.openTab({ id, title, kind: 'terminal', node: root, scope, onClose: close, onActivate: () => { if (term && viewport.clientWidth) { fit.fit(); term.focus(); } } });
    if (!tab) return;
    const poll = async () => {
      if (disposed || polling) return; polling = true;
      try {
        const page = await api.terminalOutput(target, id, after, controller.signal);
        if (disposed) return;
        if (typeof page.base64 !== 'string' || page.base64.length > 100000 || !Number.isSafeInteger(page.next) || page.next < after) throw new Error('终端输出无效。');
        if (page.dropped) { term.reset(); term.writeln('较早的终端输出已淘汰，已同步保留内容。'); }
        const bytes = Uint8Array.from(atob(page.base64), c => c.charCodeAt(0));
        await new Promise(resolve => term.write(bytes, resolve)); after = page.next;
        if (disposed) return;
        faulted = writeUncertain; status.textContent = writeUncertain ? '输入结果未确认，仅恢复输出；不会重放输入，请关闭此终端后重开。' : ''; resume.hidden = true;
        if (page.exited) { status.textContent = `进程已退出${page.exit_code == null ? '' : ` (${page.exit_code})`}`; faulted = true; return; }
        timer = setTimeout(poll, root.hidden || document.hidden || this.pane.pane.hidden ? 800 : 180);
      } catch (error) { if (!disposed && error.name !== 'AbortError') { status.textContent = error.message; faulted = true; resume.hidden = error.status === 404 || error.status === 403; } }
      finally { polling = false; }
    };
    try {
      await terminalLibrary(); if (disposed) return;
      term = new window.Terminal({ cursorBlink: true, scrollback: 4000, fontSize: 13, fontFamily: getComputedStyle(document.documentElement).getPropertyValue('--ui-font-code').trim(), allowProposedApi: false });
      fit = new window.FitAddon.FitAddon(); term.loadAddon(fit); term.open(viewport); applyTheme(); fit.fit();
      openAttempted = true;
      await api.terminalOpen(target, id, Math.max(2, term.rows), Math.max(2, term.cols)); started = true;
      if (disposed) { try { await api.terminalClose(target, id); } finally { api.clear(); } return; }
      status.textContent = '';
      term.onData(data => {
        if (disposed || faulted) return;
        const bytes = new TextEncoder().encode(data).length;
        if (bytes > 16384 || pendingInputBytes + bytes > 32768) { status.textContent = '终端输入队列超过限制，请等待已输入内容完成后分段发送。'; return; }
        pendingInputBytes += bytes;
        writes = writes.then(() => { if (!faulted && !disposed) return api.terminalInput(target, id, ++sequence, data); })
          .catch(error => { faulted = true; writeUncertain = true; status.textContent = `${error.message} 输入结果未确认，不会自动重发。`; })
          .finally(() => { pendingInputBytes -= bytes; });
      });
      observer = new ResizeObserver(() => {
        clearTimeout(resizeTimer); resizeTimer = setTimeout(() => {
          if (disposed || !viewport.clientWidth || !viewport.clientHeight) return;
          fit.fit(); void api.terminalSize(target, id, Math.max(2, Math.min(300, term.rows)), Math.max(2, Math.min(500, term.cols))).catch(error => { if (!disposed) status.textContent = error.message; });
        }, 100);
      }); observer.observe(viewport); if (this.pane.scope === scope && !root.hidden && !this.pane.pane.hidden) term.focus(); void poll();
    } catch (error) {
      if (!disposed) status.textContent = error.message;
      if (openAttempted) { try { await api.terminalClose(target, id); } catch { if (!disposed) status.textContent += ' 进程状态未确认；不会自动重新启动。'; } }
      api.clear();
    }
  }
}
