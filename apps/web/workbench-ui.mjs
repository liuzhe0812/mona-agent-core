import { panelButton, copyPanelText } from './pane-icons.mjs';
const node = (tag, className, text = '') => { const n = document.createElement(tag); n.className = className; n.textContent = text; return n; };
let terminalAssets;
function loadScript(path) { return new Promise((resolve, reject) => { const script = document.createElement('script'); script.src = path; script.onload = resolve; script.onerror = () => reject(new Error('终端资源加载失败。')); document.head.append(script); }); }
function terminalLibrary() {
  if (!terminalAssets) terminalAssets = (async () => {
    const link = document.createElement('link'); link.rel = 'stylesheet'; link.href = '/apps/web/vendor/xterm/xterm.css'; document.head.append(link);
    await loadScript('/apps/web/vendor/xterm/xterm.js'); await loadScript('/apps/web/vendor/xterm/addon-fit.js');
  })().catch(error => { terminalAssets = null; throw error; });
  return terminalAssets;
}
export class WorkbenchUI {
  constructor({ pane, workspace }) {
    this.pane = pane; this.workspace = workspace; this.serial = 0;
    for (const [id, label, run] of [['git', '审查', () => this.openReview()], ['terminal', '终端', () => this.openTerminal()]]) {
      pane.registerAction({ id, label, available: () => Boolean(workspace.available && this.target() && workspace.features?.[id === 'git' ? 'review' : id]), run });
    }
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
    const renderDiff = () => {
      const lines = node('div', split ? 'review-diff is-split' : 'review-diff');
      const allLines = diffText.split('\n');
      for (const line of allLines.slice(0, 6000)) {
        const kind = line.startsWith('@@') || line.startsWith('+++') || line.startsWith('---') ? 'meta' : line.startsWith('+') ? 'add' : line.startsWith('-') ? 'delete' : 'context';
        const row = node('div', `review-line diff-${kind}`);
        if (split && (kind === 'add' || kind === 'delete')) row.append(node('span', '', kind === 'delete' ? line : ''), node('span', '', kind === 'add' ? line : ''));
        else row.textContent = line || ' ';
        lines.append(row);
      }
      detail.replaceChildren(lines);
      if (allLines.length > 6000) detail.append(node('p', 'muted-note', '差异预览超过 6000 行，可复制完整有界差异查看。'));
    };
    const diff = async (path, staged) => {
      const mine = ++generation; status.textContent = '正在读取差异…';
      try {
        const result = await this.workspace.api.diff(target, path, staged, controller.signal);
        if (mine !== generation) return;
        if (typeof result.text !== 'string' || result.text.length > 1024 * 1024) throw new Error('差异响应无效。');
        currentPath = path; diffText = result.text || '没有可显示的文本差异。'; renderDiff(); status.textContent = `${path}${staged ? ' · 已暂存' : ' · 工作区'}${result.truncated ? ' · 预览已截断' : ''}`;
      } catch (error) { if (mine === generation && error.name !== 'AbortError') status.textContent = error.message; }
    };
    const refresh = async () => {
      controller.abort(); controller = new AbortController(); const mine = ++generation; status.textContent = '正在读取变更…';
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
              if (detail.previousElementSibling === item) { generation++; detail.remove(); currentPath = ''; item.setAttribute('aria-expanded', 'false'); return; }
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
    source.addEventListener('change', () => { void refresh(); });
    toolbar.append(source, status, layout, copy, open, panelButton('刷新', () => { void refresh(); })); root.append(toolbar, body);
    this.pane.openTab({ id, title: '审查', kind: 'git', node: root, scope, onClose: () => { generation++; controller.abort(); }, reopen: () => this.openReview() });
    void refresh();
  }
  async openTerminal() {
    const target = this.target(), scope = this.pane.scope; if (!target) return;
    const api = this.workspace.api, id = crypto.randomUUID(), title = `终端 ${++this.serial}`;
    const root = node('section', 'terminal-pane'), status = node('p', 'terminal-status', '正在启动终端…'), viewport = node('div', 'terminal-viewport');
    root.append(viewport, status);
    const controller = new AbortController(); let term, fit, observer, timer, disposed = false, started = false, after = 0, sequence = 0, faulted = false, resizeTimer;
    let writes = Promise.resolve();
    const close = async () => {
      disposed = true; clearTimeout(timer); clearTimeout(resizeTimer); controller.abort(); observer?.disconnect(); themeObserver.disconnect(); term?.dispose();
      if (started) await api.terminalClose(target, id);
    };
    const applyTheme = () => { if (!term) return; const css = getComputedStyle(document.documentElement); term.options.theme = { background: css.getPropertyValue('--surface-elevated').trim(), foreground: css.getPropertyValue('--text-primary').trim(), cursor: css.getPropertyValue('--text-primary').trim() }; };
    const themeObserver = new MutationObserver(applyTheme); themeObserver.observe(document.documentElement, { attributes: true });
    const tab = this.pane.openTab({ id, title, kind: 'terminal', node: root, scope, onClose: close, onActivate: () => { if (term && viewport.clientWidth) { fit.fit(); term.focus(); } } });
    if (!tab) return;
    const poll = async () => {
      if (disposed) return;
      try {
        const page = await api.terminalOutput(target, id, after, controller.signal);
        if (disposed) return;
        if (typeof page.base64 !== 'string' || page.base64.length > 100000 || !Number.isSafeInteger(page.next) || page.next < after) throw new Error('终端输出无效。');
        if (page.dropped) { term.reset(); term.writeln('较早的终端输出已淘汰，已同步保留内容。'); }
        const bytes = Uint8Array.from(atob(page.base64), c => c.charCodeAt(0));
        await new Promise(resolve => term.write(bytes, resolve)); after = page.next;
        if (page.exited) { status.textContent = `进程已退出${page.exit_code == null ? '' : ` (${page.exit_code})`}`; faulted = true; return; }
        timer = setTimeout(poll, root.hidden ? 800 : 180);
      } catch (error) { if (!disposed && error.name !== 'AbortError') { status.textContent = error.message; faulted = true; } }
    };
    try {
      await terminalLibrary(); if (disposed) return;
      term = new window.Terminal({ cursorBlink: true, scrollback: 4000, fontSize: 13, fontFamily: getComputedStyle(document.documentElement).getPropertyValue('--ui-font-code').trim(), allowProposedApi: false });
      fit = new window.FitAddon.FitAddon(); term.loadAddon(fit); term.open(viewport); applyTheme(); fit.fit();
      await api.terminalOpen(target, id, Math.max(2, term.rows), Math.max(2, term.cols)); started = true;
      if (disposed) { await api.terminalClose(target, id); return; }
      status.textContent = '';
      term.onData(data => {
        if (disposed || faulted) return;
        if (new TextEncoder().encode(data).length > 16384) { status.textContent = '粘贴内容超过 16 KiB，请分段输入。'; return; }
        writes = writes.then(() => { if (!faulted && !disposed) return api.terminalInput(target, id, ++sequence, data); }).catch(error => { faulted = true; status.textContent = `${error.message} 输入结果未确认，不会自动重发。`; });
      });
      observer = new ResizeObserver(() => {
        clearTimeout(resizeTimer); resizeTimer = setTimeout(() => {
          if (disposed || !viewport.clientWidth || !viewport.clientHeight) return;
          fit.fit(); void api.terminalSize(target, id, Math.max(2, Math.min(300, term.rows)), Math.max(2, Math.min(500, term.cols))).catch(error => { if (!disposed) status.textContent = error.message; });
        }, 100);
      }); observer.observe(viewport); if (this.pane.scope === scope && !root.hidden && !this.pane.pane.hidden) term.focus(); void poll();
    } catch (error) { if (!disposed) status.textContent = error.message; }
  }
}
