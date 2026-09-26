// Reading state for a bounded tool group, independent from conversation and terminal scrolling.
export class ProcessScroll {
  constructor(root, viewport, content) {
    this.root = root; this.viewport = viewport; this.content = content; this.grouped = true; this.live = false;
    this.following = false; this.frame = 0; this.disposed = false; this.events = new AbortController();
    const options = { signal: this.events.signal, passive: true };
    viewport.addEventListener('scroll', event => {
      if (event.target !== viewport) return;
      this.following = viewport.scrollHeight - viewport.clientHeight - viewport.scrollTop <= 3;
      this.edges();
    }, options);
    const interrupt = () => { this.following = false; this.initial = null; this.anchor = null; };
    for (const event of ['wheel', 'pointerdown', 'touchstart']) viewport.addEventListener(event, interrupt, options);
    viewport.addEventListener('keydown', event => { if (['ArrowUp', 'ArrowDown', 'PageUp', 'PageDown', 'Home', 'End', ' '].includes(event.key)) interrupt(); }, options);
    root.addEventListener('toggle', event => { if (event.target === root) this.schedule(); }, options);
    if (typeof ResizeObserver !== 'undefined') {
      this.observer = new ResizeObserver(() => this.schedule()); this.observer.observe(viewport); this.observer.observe(content);
    }
  }
  mode(grouped, live) {
    this.grouped = grouped; this.live = live;
    this.root.dataset.groupExpandedMode = String(!grouped);
    this.viewport.tabIndex = grouped ? 0 : -1;
    if (!grouped) { this.initial = null; this.anchor = null; this.following = false; }
    this.schedule();
  }
  initialize() { this.initial = this.live ? 'bottom' : 'top'; this.following = this.live; this.schedule(); }
  capture() {
    if (!this.grouped || !this.root.open || !this.viewport.clientHeight || this.following) return null;
    const top = this.viewport.getBoundingClientRect().top;
    for (const node of this.content.children) {
      if (node.getClientRects().length && node.getBoundingClientRect().bottom > top + 1) return { node, offset: node.getBoundingClientRect().top - top };
    }
    return null;
  }
  updated(anchor) { this.anchor ||= anchor; this.schedule(); }
  schedule() {
    if (this.disposed || this.frame) return;
    this.frame = requestAnimationFrame(() => { this.frame = 0; this.sync(); });
  }
  sync() {
    const body = this.viewport;
    if (this.disposed || !this.grouped || !this.root.open || !body.clientHeight) { this.edges(); return; }
    const anchor = this.anchor; this.anchor = null;
    if (this.initial) {
      body.scrollTop = this.initial === 'bottom' ? body.scrollHeight : 0; this.initial = null;
    } else if (anchor?.node.parentElement === this.content) {
      body.scrollTop += anchor.node.getBoundingClientRect().top - body.getBoundingClientRect().top - anchor.offset;
    } else if (this.following && this.live) {
      const selection = globalThis.getSelection?.();
      if (!selection || selection.isCollapsed || !body.contains(selection.anchorNode)) body.scrollTop = body.scrollHeight;
    }
    this.edges();
  }
  edges() {
    const body = this.viewport, visible = this.grouped && this.root.open && body.clientHeight > 0;
    body.dataset.scrollUp = String(visible && body.scrollTop > 1);
    body.dataset.scrollDown = String(visible && body.scrollHeight - body.clientHeight - body.scrollTop > 1);
  }
  dispose() { this.disposed = true; cancelAnimationFrame(this.frame); this.observer?.disconnect(); this.events.abort(); this.anchor = null; }
}
