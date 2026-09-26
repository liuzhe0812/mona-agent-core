import { element as el, button } from './content-dom.mjs';
import { domMatchRanges } from './conversation-find.mjs';

function nestedScroller(event, main, delta = 0) {
  for (let node = event.target?.closest?.('*'); node && node !== main; node = node.parentElement) {
    if (!/(?:auto|scroll)/.test(getComputedStyle(node).overflowY) || node.scrollHeight <= node.clientHeight + 1) continue;
    if (!delta || (delta < 0 && node.scrollTop > 0) || (delta > 0 && node.scrollHeight - node.clientHeight - node.scrollTop > 1)) return true;
  }
  return false;
}

// Owns reading intent and navigation, never task execution or transcript persistence.
export class ConversationControls {
  constructor({ main, timeline, loadOlder, hasOlder, onQuote, onSide, canSide, prepareSearch = () => {} }) {
    Object.assign(this, { main, timeline, loadOlder, hasOlder, onQuote, onSide, canSide, prepareSearch });
    this.intentRevision = 0; this.restoreRevision = 0;
    this.following = true; this.memory = new Map(); this.scope = ''; this.matches = []; this.index = -1; this.queryGeneration = 0;
    this.bottom = button('回到底部', () => this.jumpBottom(true), 'text-button conversation-bottom'); this.bottom.id = 'conversation-bottom'; this.bottom.hidden = true;
    main.parentElement.append(this.bottom);
    this.find = el('div', 'conversation-find'); this.find.id = 'conversation-find'; this.find.hidden = true; this.find.setAttribute('role', 'search');
    this.input = el('input'); this.input.type = 'search'; this.input.maxLength = 256; this.input.placeholder = '查找此会话'; this.input.setAttribute('aria-label', '查找此会话');
    this.count = el('span', 'conversation-find-count'); this.count.setAttribute('role', 'status');
    const previous = button('上一项', () => this.move(-1)), next = button('下一项', () => this.move(1));
    this.older = button('继续搜索历史', () => { void this.searchOlder(); }); this.stop = button('停止搜索', () => { this.queryGeneration++; this.searching = false; this.status(); }); this.stop.hidden = true;
    this.find.append(this.input, this.count, previous, next, this.older, this.stop, button('关闭', () => this.showFind(false)));
    main.parentElement.append(this.find);
    this.trigger = button('查找', () => this.showFind(this.find.hidden), 'icon-button conversation-find-trigger'); this.trigger.setAttribute('aria-label', '查找此会话'); this.trigger.dataset.tooltip = '查找此会话（Ctrl+F）'; main.parentElement.querySelector('.topbar').append(this.trigger);
    this.selection = el('div', 'conversation-selection'); this.selection.hidden = true; this.selection.setAttribute('role', 'toolbar'); this.selection.setAttribute('aria-label', '选中文字');
    const quote = button('引用到输入', () => { const value = this.selected; this.selection.hidden = true; if (value) onQuote(value); });
    this.sideButton = button('在侧边询问', () => { const value = this.selected; this.selection.hidden = true; if (value) Promise.resolve().then(() => onSide(value)).catch(error => { this.showFind(true); this.findError = error.message; this.status(); }); });
    this.selection.append(quote, this.sideButton); main.parentElement.append(this.selection);
    this.input.addEventListener('input', () => {
      this.queryGeneration++; this.searching = false; this.findError = ''; this.matches = []; this.index = -1;
      globalThis.CSS?.highlights?.delete('mona-find'); globalThis.CSS?.highlights?.delete('mona-find-active'); this.status();
      clearTimeout(this.findTimer); this.findTimer = setTimeout(() => this.search(), 150);
    });
    this.input.addEventListener('keydown', event => { if (event.key === 'Enter' && !event.isComposing) { event.preventDefault(); this.move(event.shiftKey ? -1 : 1); } if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); this.showFind(false); } });
    document.addEventListener('keydown', event => {
      if (event.defaultPrevented || document.querySelector('dialog[open]') || event.target.closest?.('#files-panel,#settings-page') || main.closest('[hidden],[inert]')) return;
      if ((event.ctrlKey || event.metaKey) && !event.altKey && event.key.toLowerCase() === 'f') { event.preventDefault(); this.showFind(true); }
    });
    main.addEventListener('wheel', event => { if (event.deltaY < 0 && !nestedScroller(event, main, event.deltaY)) { this.intentRevision++; this.adjusting = false; this.following = false; this.anchor = this.capture(); } }, { passive: true });
    main.addEventListener('touchstart', event => { if (!nestedScroller(event, main)) { this.intentRevision++; this.adjusting = false; this.following = false; this.anchor = this.capture(); } }, { passive: true });
    main.addEventListener('keydown', event => {
      if (event.target.closest('input,textarea,select,button,a') || nestedScroller(event, main, -1)) return;
      if (['PageUp', 'ArrowUp', 'Home'].includes(event.key)) { this.intentRevision++; this.adjusting = false; this.following = false; this.anchor = this.capture(); }
    });
    main.addEventListener('scroll', () => {
      const top = main.scrollTop;
      if (this.expectedScrollTop != null && Math.abs(top - this.expectedScrollTop) < 1) { this.expectedScrollTop = null; return; }
      this.expectedScrollTop = null; this.intentRevision++;
      this.following = main.scrollHeight - main.scrollTop - main.clientHeight <= 24;
      this.anchor = this.capture(); this.bottom.hidden = this.following || main.scrollHeight <= main.clientHeight + 120;
      this.selection.hidden = true;
    }, { passive: true });
    timeline.addEventListener('mona:content-resized', () => {
      this.schedule();
      if (!this.find.hidden && this.input.value) { clearTimeout(this.findTimer); this.findTimer = setTimeout(() => this.search(false), 160); }
    });
    this.resize = new ResizeObserver(() => this.schedule()); this.resize.observe(timeline); this.resize.observe(main);
    this.dock = main.parentElement.querySelector('.composer-wrap');
    this.dockResize = new ResizeObserver(() => {
      const height = Math.round(this.dock.getBoundingClientRect().height);
      main.parentElement.style.setProperty('--conversation-dock-height', `${height}px`);
    });
    if (this.dock) this.dockResize.observe(this.dock);
    document.addEventListener('selectionchange', () => { clearTimeout(this.selectionTimer); this.selectionTimer = setTimeout(() => this.inspectSelection(), 120); });
  }
  capture() {
    const top = this.main.getBoundingClientRect().top;
    const turns = [...this.timeline.querySelectorAll('.turn')]; const turn = turns.find(node => node.getBoundingClientRect().bottom > top + 5);
    if (!turn) return { following: this.following, scrollTop: this.main.scrollTop };
    const blocks = [...turn.querySelectorAll('.user-message,.message-text > *')];
    const block = blocks.find(node => node.getBoundingClientRect().bottom > top + 5) || turn;
    return { following: this.following, id: turn.dataset.turnId, before: Number(turn.dataset.historyBefore) || undefined, block: blocks.indexOf(block), top: block.getBoundingClientRect().top - top, scrollTop: this.main.scrollTop };
  }
  restore(anchor) {
    if (!anchor) return;
    this.adjusting = true; const revision = ++this.restoreRevision;
    if (anchor.following) this.main.scrollTop = this.main.scrollHeight;
    else {
      const turn = anchor.id ? this.timeline.querySelector(`[data-turn-id="${CSS.escape(anchor.id)}"]`) : null;
      const block = turn?.querySelectorAll('.user-message,.message-text > *')[anchor.block] || turn;
      if (block) this.main.scrollTop += block.getBoundingClientRect().top - this.main.getBoundingClientRect().top - anchor.top;
      else this.main.scrollTop = anchor.scrollTop;
    }
    this.expectedScrollTop = this.main.scrollTop;
    requestAnimationFrame(() => { if (revision === this.restoreRevision) this.adjusting = false; });
  }
  schedule() {
    if (this.frame) return;
    this.frame = requestAnimationFrame(() => { this.frame = 0; this.restore(this.following ? { following: true } : this.anchor); this.bottom.hidden = this.following || this.main.scrollHeight <= this.main.clientHeight + 120; });
  }
  beforeChange() { const anchor = { ...this.capture(), scope: this.scope, intentRevision: this.intentRevision }; this.anchor = anchor; return anchor; }
  afterChange(anchor = this.anchor) {
    if (anchor?.scope != null && anchor.scope !== this.scope) return;
    // A reader gesture after an async load began wins over its saved offset.
    if (anchor?.intentRevision != null && anchor.intentRevision !== this.intentRevision) this.anchor = this.capture();
    else { this.anchor = anchor; this.restore(anchor); }
    if (!this.find.hidden && this.input.value) this.search(false);
  }
  clear() {
    this.intentRevision++; this.restoreRevision++; this.adjusting = false; this.expectedScrollTop = null; this.selected = null; clearTimeout(this.selectionTimer);
    if (this.scope) { this.memory.delete(this.scope); this.memory.set(this.scope, this.capture()); while (this.memory.size > 64) this.memory.delete(this.memory.keys().next().value); }
    if (this.frame) cancelAnimationFrame(this.frame); this.frame = 0;
    this.scope = ''; this.anchor = null; this.following = true; this.queryGeneration++; this.showFind(false); this.selection.hidden = true; this.input.value = ''; this.findError = ''; clearTimeout(this.findTimer);
  }
  historyBefore(scope) {
    const anchor = this.scope === scope ? this.capture() : this.memory.get(scope);
    return anchor && !anchor.following ? anchor.before : undefined;
  }
  activate(scope, restore = true) {
    this.intentRevision++;
    this.scope = scope || ''; const anchor = restore && this.memory.get(scope); this.following = anchor ? anchor.following : true;
    this.anchor = anchor || { following: true }; this.restore(this.anchor); this.schedule();
  }
  jumpBottom(smooth = false) {
    this.intentRevision++;
    this.following = true; this.anchor = { following: true };
    this.main.scrollTo({ top: this.main.scrollHeight, behavior: 'instant' }); this.bottom.hidden = true;
  }
  showFind(show) {
    this.find.hidden = !show; this.trigger.setAttribute('aria-expanded', String(show));
    if (show) { this.input.focus(); this.input.select(); this.search(false); }
    else { clearTimeout(this.findTimer); this.queryGeneration++; this.searching = false; this.matches = []; globalThis.CSS?.highlights?.delete('mona-find'); globalThis.CSS?.highlights?.delete('mona-find-active'); if (this.find.contains(document.activeElement)) this.trigger.focus(); }
  }
  search(focus = true) {
    if (this.find.hidden) return;
    const query = this.input.value, previous = this.matches[this.index]; this.matches = []; this.index = -1;
    if (query) {
      this.prepareSearch(query);
      const selector = '.user-message,.message-text,.activity-panel-output,.activity-panel-result';
      const roots = [...this.timeline.querySelectorAll(selector)].filter(node => !node.parentElement.closest(selector));
      for (const root of roots) {
        if (this.matches.length >= 5000) break;
        this.matches.push(...domMatchRanges(root, query, 5000 - this.matches.length));
      }
      if (previous) this.index = this.matches.findIndex(range => range.startContainer === previous.startContainer && range.startOffset === previous.startOffset && range.endContainer === previous.endContainer && range.endOffset === previous.endOffset);
    }
    if (globalThis.CSS?.highlights && typeof Highlight !== 'undefined') {
      CSS.highlights.set('mona-find', new Highlight(...this.matches));
      if (this.index >= 0) CSS.highlights.set('mona-find-active', new Highlight(this.matches[this.index])); else CSS.highlights.delete('mona-find-active');
    }
    if (this.matches.length && focus) { this.index = -1; this.move(1); } else this.status();
  }
  status() { this.count.textContent = this.findError || `${this.matches.length ? this.index + 1 : 0}/${this.matches.length}${this.matches.length === 5000 ? '+' : ''} · 已加载 ${this.timeline.querySelectorAll('.turn').length} 轮${this.searching ? ' · 搜索历史中…' : ''}`; this.older.hidden = !this.hasOlder() || this.searching; this.stop.hidden = !this.searching; }
  move(delta) {
    if (!this.matches.length) return;
    this.index = (this.index + delta + this.matches.length) % this.matches.length; const range = this.matches[this.index], parent = range.startContainer.parentElement;
    for (let ancestor = parent; ancestor && ancestor !== this.timeline; ancestor = ancestor.parentElement) {
      if (ancestor.tagName === 'DETAILS') ancestor.open = true;
      if (ancestor.matches('.message-code-block.is-collapsed')) ancestor.querySelector('.code-expand')?.click();
      else if (ancestor.matches('.user-message.is-collapsed')) ancestor.nextElementSibling?.querySelector('[aria-expanded="false"]')?.click();
    }
    this.intentRevision++; this.following = false; parent.scrollIntoView({ block: 'center', behavior: 'instant' }); this.anchor = this.capture();
    if (globalThis.CSS?.highlights && typeof Highlight !== 'undefined') CSS.highlights.set('mona-find-active', new Highlight(range)); else { const selection = window.getSelection(); selection.removeAllRanges(); selection.addRange(range); }
    this.status();
  }
  async searchOlder() {
    if (this.searching || !this.hasOlder()) return;
    const generation = ++this.queryGeneration; this.searching = true; this.findError = ''; this.status();
    try {
      for (let pages = 0; pages < 10 && this.hasOlder() && generation === this.queryGeneration; pages++) {
        const progressed = await this.loadOlder(); if (generation !== this.queryGeneration) return;
        this.search(false); if (progressed === false) break;
      }
      if (generation === this.queryGeneration && this.matches.length) this.move(1);
    } catch (error) { if (generation === this.queryGeneration) this.findError = error.message; }
    finally { if (generation === this.queryGeneration) { this.searching = false; this.status(); } }
  }
  inspectSelection() {
    const selection = window.getSelection(); this.selection.hidden = true;
    if (!selection || selection.isCollapsed || selection.rangeCount !== 1 || !this.timeline.contains(selection.anchorNode) || !this.timeline.contains(selection.focusNode)) return;
    const range = selection.getRangeAt(0), a = range.startContainer.parentElement, b = range.endContainer.parentElement;
    const region = a?.closest('[data-conversation-selectable]'), end = b?.closest('[data-conversation-selectable]');
    if (!region || region !== end || a.closest('button,summary') || b.closest('button,summary')) return;
    const text = selection.toString().trim(); if (!text || text.length > 8000) return;
    this.selected = { text, sessionId: this.scope, turnId: region.closest('.turn')?.dataset.turnId, type: region.dataset.conversationSelectable };
    this.sideButton.hidden = !this.canSide(); this.selection.hidden = false;
  }
}
