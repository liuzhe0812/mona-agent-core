import { CENTER_MIN_WIDTH, SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_DEFAULT_WIDTH, SIDEBAR_KEYBOARD_STEP, MOBILE_WIDTH, clamp, draftHeight } from './layout-geometry.mjs';
import { setBaseInert, setShellOverlay, focusable, trapTab } from './overlay-scope.mjs';
const $ = selector => document.querySelector(selector);
const SIDEBAR_WIDTH_KEY = 'mona.web.sidebar.v1';
const SIDEBAR_SECTIONS_KEY = 'mona.web.sidebar.sections.v1';
// Owns only navigation presentation. Project/session selection stays in its existing controllers.
export class NavigationLayout {
  constructor({ pane } = {}) {
    this.pane = pane; this.shell = $('.shell'); this.sidebar = $('#chat-sidebar'); this.navigation = this.sidebar.querySelector('.sidebar-navigation'); this.brand = this.sidebar.querySelector('.brand'); this.heading = $('.thread-heading'); this.trigger = $('#sidebar-toggle'); this.scrim = $('#sidebar-scrim'); this.resizer = $('#sidebar-resizer');
    this.mobile = matchMedia(`(max-width: ${MOBILE_WIDTH}px)`); this.events = new AbortController();
    const on = (node, type, fn, options = {}) => node.addEventListener(type, fn, { ...options, signal: this.events.signal });
    try { const stored = Number(localStorage.getItem(SIDEBAR_WIDTH_KEY)); if (stored > 0 && Number.isFinite(stored)) this.preferred = clamp(stored, SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH); } catch { /* Browser preference is optional. */ }
    on(this.trigger, 'click', () => this.toggle()); on(this.scrim, 'click', () => this.close(true));
    on(this.mobile, 'change', () => this.close()); on(window, 'resize', () => this.sync());
    on(this.shell, 'mona:right-overlay', () => { if (this.shell.classList.contains('sidebar-open')) this.close(); });
    on(document, 'keydown', event => {
      if (!event.defaultPrevented && (event.ctrlKey || event.metaKey) && !event.altKey && !event.shiftKey
        && event.key.toLowerCase() === 'b' && !document.querySelector('dialog[open]') && this.sidebar.hidden === false) {
        event.preventDefault(); this.toggle(); return;
      }
      if (event.defaultPrevented || !this.mobile.matches || !this.shell.classList.contains('sidebar-open') || this.sidebar.inert || document.querySelector('dialog[open], [popover]:popover-open')) return;
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); this.close(true); }
      if (event.key === 'Tab') trapTab(event, this.sidebar);
    });
    on(this.resizer, 'pointerdown', event => {
      if (event.button !== 0 || this.mobile.matches || this.shell.classList.contains('sidebar-collapsed')) return;
      this.gesture = { id: event.pointerId, x: event.clientX, width: this.actual };
      this.resizer.setPointerCapture(event.pointerId); this.shell.classList.add('sidebar-resizing'); event.preventDefault();
    });
    on(this.resizer, 'pointermove', event => { if (this.gesture?.id === event.pointerId) this.applyWidth(this.gesture.width + event.clientX - this.gesture.x); });
    const end = event => { if (this.gesture?.id !== event.pointerId) return; this.gesture = null; this.shell.classList.remove('sidebar-resizing'); this.persist(); };
    for (const type of ['pointerup', 'pointercancel', 'lostpointercapture']) on(this.resizer, type, end);
    on(this.resizer, 'keydown', event => {
      const delta = event.key === 'ArrowLeft' ? -SIDEBAR_KEYBOARD_STEP : event.key === 'ArrowRight' ? SIDEBAR_KEYBOARD_STEP : 0;
      if (!delta && !['Home', 'End'].includes(event.key)) return;
      event.preventDefault(); this.applyWidth(event.key === 'Home' ? SIDEBAR_MIN_WIDTH : event.key === 'End' ? this.maximum : this.actual + delta); this.persist();
    });
    on(this.resizer, 'dblclick', () => { this.applyWidth(SIDEBAR_DEFAULT_WIDTH); this.persist(); });
    on(this.navigation, 'dragstart', event => {
      const section = event.target.closest?.('[data-section-handle]')?.closest('#projects-region,.sessions-region');
      if (!section || section.hidden) return;
      this.dragSection = section; section.classList.add('is-section-dragging');
      event.dataTransfer?.setData('text/plain', 'mona-sidebar-section');
      if (event.dataTransfer) event.dataTransfer.effectAllowed = 'move';
    });
    on(this.navigation, 'dragover', event => {
      const target = event.target.closest?.('#projects-region,.sessions-region');
      if (!this.dragSection || !target || target === this.dragSection || target.hidden) return;
      event.preventDefault(); this.clearSectionDrop();
      target.dataset.sectionDrop = this.dragSection.compareDocumentPosition(target) & Node.DOCUMENT_POSITION_FOLLOWING ? 'after' : 'before';
      if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
    });
    on(this.navigation, 'drop', event => {
      const target = event.target.closest?.('#projects-region,.sessions-region');
      if (this.dragSection && target && target !== this.dragSection && !target.hidden) {
        event.preventDefault();
        if (this.dragSection.compareDocumentPosition(target) & Node.DOCUMENT_POSITION_FOLLOWING) target.after(this.dragSection);
        else target.before(this.dragSection);
        this.saveSectionOrder();
      }
      this.clearSectionDrag();
    });
    on(this.navigation, 'dragend', () => this.clearSectionDrag());
    on(this.navigation, 'keydown', event => {
      if (!event.altKey || !['ArrowUp', 'ArrowDown'].includes(event.key)) return;
      const handle = event.target.closest?.('[data-section-handle]');
      const section = handle?.closest('#projects-region,.sessions-region');
      const other = event.key === 'ArrowUp' ? section?.previousElementSibling : section?.nextElementSibling;
      if (!other || other.hidden) return;
      event.preventDefault();
      this.navigation.insertBefore(section, event.key === 'ArrowUp' ? other : other.nextSibling);
      this.saveSectionOrder(); handle.focus();
    });
    this.restoreSectionOrder();
    this.sync();
  }
  restoreSectionOrder() {
    try {
      const order = JSON.parse(localStorage.getItem(SIDEBAR_SECTIONS_KEY) || 'null');
      if (!Array.isArray(order) || order.length !== 2 || new Set(order).size !== 2 || !order.includes('projects') || !order.includes('tasks')) return;
      for (const id of order) {
        const section = this.navigation.querySelector(id === 'projects' ? '#projects-region' : '.sessions-region');
        if (section) this.navigation.append(section);
      }
    } catch { /* Invalid browser preference leaves the default order. */ }
  }
  saveSectionOrder() {
    const order = [...this.navigation.children].map(node => node.id === 'projects-region' ? 'projects' : 'tasks');
    try { localStorage.setItem(SIDEBAR_SECTIONS_KEY, JSON.stringify(order)); }
    catch { /* Keep this window's order when storage is unavailable. */ }
  }
  clearSectionDrop() { for (const section of this.navigation.querySelectorAll('[data-section-drop]')) delete section.dataset.sectionDrop; }
  clearSectionDrag() { this.clearSectionDrop(); this.dragSection?.classList.remove('is-section-dragging'); this.dragSection = null; }
  applyWidth(value) { this.preferred = clamp(value, SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH); this.sync(); }
  persist() { try { localStorage.setItem(SIDEBAR_WIDTH_KEY, String(this.preferred)); } catch { /* Keep this window's layout, do not claim a save. */ } }
  toggle() {
    this.shell.classList.toggle(this.mobile.matches ? 'sidebar-open' : 'sidebar-collapsed'); this.sync();
    if (this.mobile.matches && this.shell.classList.contains('sidebar-open')) focusable(this.sidebar)[0]?.focus({ preventScroll: true });
  }
  close(focus = false) { this.shell.classList.remove('sidebar-open'); this.sync(); if (focus) this.trigger.focus({ preventScroll: true }); }
  sync() {
    const styles = getComputedStyle(this.shell), gutter = parseFloat(styles.paddingLeft) + parseFloat(styles.paddingRight) + 6;
    this.maximum = Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, this.shell.clientWidth - gutter - CENTER_MIN_WIDTH));
    const desired = this.preferred ?? (this.shell.clientWidth <= 1000 ? 224 : SIDEBAR_DEFAULT_WIDTH);
    this.actual = clamp(desired, SIDEBAR_MIN_WIDTH, this.maximum);
    const width = `${this.actual}px`;
    if (this.shell.style.getPropertyValue('--sidebar-width') !== width) this.shell.style.setProperty('--sidebar-width', width);
    this.resizer.setAttribute('aria-valuemin', String(SIDEBAR_MIN_WIDTH)); this.resizer.setAttribute('aria-valuemax', String(this.maximum)); this.resizer.setAttribute('aria-valuenow', String(this.actual));
    const open = this.mobile.matches ? this.shell.classList.contains('sidebar-open') : !this.shell.classList.contains('sidebar-collapsed');
    const destination = open ? this.brand : this.heading;
    const focused = document.activeElement === this.trigger;
    if (this.trigger.parentElement !== destination) {
      if (open) destination.append(this.trigger); else destination.prepend(this.trigger);
    }
    setBaseInert(this.shell, this.sidebar, !open);
    if (focused) this.trigger.focus({ preventScroll: true });
    this.trigger.setAttribute('aria-expanded', String(open)); this.scrim.hidden = !this.mobile.matches || !open;
    const label = open ? (this.mobile.matches ? '关闭侧栏' : '收起侧栏') : '展开侧栏';
    this.trigger.setAttribute('aria-label', label); this.trigger.dataset.tooltip = label;
    if (this.mobile.matches && open) { this.sidebar.setAttribute('role', 'dialog'); this.sidebar.setAttribute('aria-modal', 'true'); }
    else { this.sidebar.removeAttribute('role'); this.sidebar.removeAttribute('aria-modal'); }
    setShellOverlay(this.shell, this, [this.sidebar, this.scrim], this.mobile.matches && open);
    this.shell.dispatchEvent(new Event('mona:navigation-layout'));
  }
  dispose() { this.events.abort(); this.clearSectionDrag(); setShellOverlay(this.shell, this, [], false); }
}

// A single width/height axis for the transcript, extension dock, composer and statistics.
export class ComposerLayout {
  constructor() {
    this.workspace = $('#chat-workspace'); this.main = $('#main'); this.dock = $('.composer-wrap'); this.composer = $('.composer'); this.input = $('#prompt'); this.events = new AbortController();
    const on = (node, type, fn, options = {}) => node?.addEventListener(type, fn, { ...options, signal: this.events.signal });
    this.mirror = document.createElement('div'); this.mirror.className = 'composer-draft-mirror'; this.mirror.setAttribute('aria-hidden', 'true'); this.input.parentElement.append(this.mirror);
    on(this.input, 'input', () => this.schedule()); on(this.input, 'compositionend', () => this.schedule()); on(this.input, 'focus', () => this.schedule());
    on(window, 'resize', () => this.schedule()); on(window.visualViewport, 'resize', () => this.schedule());
    this.resize = new ResizeObserver(() => this.schedule()); for (const node of [this.workspace, this.main, this.dock, this.input]) this.resize.observe(node);
    this.mutations = new MutationObserver(() => this.schedule());
    for (const node of [$('.composer-row'), $('#composer-ui-dock'), $('#conversation-status-slots'), $('#project-context')]) if (node) this.mutations.observe(node, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ['hidden'] });
    this.mutations.observe(document.documentElement, { attributes: true, attributeFilter: ['style', 'data-theme'] });
    on(this.dock, 'wheel', event => {
      if (event.ctrlKey || event.shiftKey || !event.deltaY || event.target.closest('[popover],select,button')) return;
      if (event.target === this.input) {
        const remaining = this.input.scrollHeight - this.input.clientHeight - this.input.scrollTop;
        if ((event.deltaY > 0 && remaining > 1) || (event.deltaY < 0 && this.input.scrollTop > 1)) return;
      }
      event.preventDefault(); this.main.scrollBy({ top: event.deltaY, behavior: 'instant' });
    }, { passive: false });
    document.fonts?.ready.then(() => { if (!this.disposed) this.schedule(); }); this.schedule();
  }
  schedule() { if (this.frame || this.disposed) return; this.frame = requestAnimationFrame(() => { this.frame = 0; this.measure(); }); }
  measure() {
    if (this.workspace.hidden || !this.input.clientWidth) return;
    const scrollbar = `${Math.max(0, this.main.offsetWidth - this.main.clientWidth)}px`;
    if (this.workspace.style.getPropertyValue('--conversation-scrollbar-width') !== scrollbar) this.workspace.style.setProperty('--conversation-scrollbar-width', scrollbar);
    const value = (this.input.value || ' ') + '\u200b'; if (this.mirror.textContent !== value) this.mirror.textContent = value;
    const field = getComputedStyle(this.input), height = this.input.getBoundingClientRect().height;
    const floor = parseFloat(field.minHeight) || 40, cap = parseFloat(field.maxHeight) || 220;
    const available = Math.min(this.workspace.clientHeight, window.visualViewport?.height || innerHeight);
    const desired = `${Math.round(draftHeight({ natural: this.mirror.scrollHeight, floor, cap, workspace: available, header: this.workspace.querySelector('.topbar').offsetHeight, chrome: this.dock.offsetHeight - height }))}px`;
    if (this.composer.style.getPropertyValue('--composer-draft-height') !== desired) this.composer.style.setProperty('--composer-draft-height', desired);
  }
  dispose() { this.disposed = true; this.events.abort(); this.resize.disconnect(); this.mutations.disconnect(); cancelAnimationFrame(this.frame); this.mirror.remove(); }
}
