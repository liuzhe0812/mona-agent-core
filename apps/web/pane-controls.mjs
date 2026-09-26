import { paneIcon, panelButton } from './pane-icons.mjs';
import { copyText } from './content-dom.mjs';
export function paneButton(label, icon, action, className = '') {
  const button = panelButton('', action, 'icon-button pane-control ' + className);
  button.setAttribute('aria-label', label); button.dataset.tooltip = label; button.append(paneIcon(icon)); return button;
}
const feedback = new WeakMap();
export async function copyPaneText(value, button) {
  const prior = feedback.get(button); clearTimeout(prior?.timer);
  const state = { label: prior?.label || button.getAttribute('aria-label') || button.textContent }; feedback.set(button, state);
  const success = await copyText(value); if (feedback.get(button) !== state || !button.isConnected) return;
  button.dataset.tooltip = success ? '已复制' : '复制失败'; button.setAttribute('aria-label', button.dataset.tooltip);
  state.timer = setTimeout(() => { if (feedback.get(button) !== state) return; feedback.delete(button); button.dataset.tooltip = state.label; button.setAttribute('aria-label', state.label); }, 1300);
}
// A top-layer menu never gets clipped by a document's scrollport or a narrow split pane.
export class PaneMenu {
  constructor(root, { role = 'menu', onClose = () => {}, gap = 6, edge = 8, side = 'auto', align = 'end' } = {}) {
    this.role = role; this.onClose = onClose; this.gap = gap; this.edge = edge; this.side = side; this.align = align;
    this.root = root; this.root.classList.add('pane-menu'); this.root.setAttribute('popover', 'manual'); this.root.setAttribute('role', role); this.root.hidden = true;
    this.events = new AbortController(); const on = (target, name, fn, options = {}) => target.addEventListener(name, fn, { ...options, signal: this.events.signal });
    on(document, 'pointerdown', event => { if (this.opened && !root.contains(event.target) && !this.trigger?.contains(event.target)) this.close(); });
    on(root, 'keydown', event => {
      const items = [...root.querySelectorAll('button:not(:disabled)')].filter(n => !n.hidden), index = items.indexOf(document.activeElement);
      const at = event.key === 'ArrowDown' ? (index + 1) % items.length : event.key === 'ArrowUp' ? (index + items.length - 1) % items.length : event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1 : -1;
      if (this.role === 'menu' && at >= 0 && items[at]) { event.preventDefault(); event.stopPropagation(); items[at].focus(); }
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); this.close(true); }
      if (event.key === 'Tab' && this.role === 'menu') this.close();
    });
    on(window, 'resize', () => { if (this.opened) this.position(); });
    on(window, 'scroll', () => { if (this.opened) this.position(); }, { capture: true });
  }
  open(trigger, focus = true) {
    this.close(); this.trigger = trigger; this.opened = true; this.root.hidden = false; trigger?.setAttribute('aria-expanded', 'true');
    this.root.showPopover(); this.position();
    if (typeof ResizeObserver !== 'undefined') { this.size = new ResizeObserver(() => { if (this.opened) this.position(); }); this.size.observe(this.root); }
    if (focus) [...this.root.querySelectorAll('button:not(:disabled),input:not(:disabled)')].find(item => !item.hidden && item.getClientRects().length)?.focus({ preventScroll: true });
    this.visibility = new MutationObserver(() => { if (this.opened && (!this.root.isConnected || this.trigger?.closest('[hidden],[inert]'))) this.close(); });
    for (let node = this.root.parentElement; node; node = node.parentElement) this.visibility.observe(node, { attributes: true, attributeFilter: ['hidden', 'inert'], childList: true });
  }
  position() {
    const box = this.trigger?.getBoundingClientRect(); if (!box) return;
    const menu = this.root.getBoundingClientRect(), { gap, edge } = this;
    const x = Math.max(edge, Math.min(this.align === 'start' ? box.left : box.right - menu.width, window.innerWidth - menu.width - edge));
    const below = box.bottom + gap;
    const wanted = this.side === 'top' ? box.top - menu.height - gap : below + menu.height <= window.innerHeight - edge ? below : box.top - menu.height - gap;
    const y = Math.max(edge, Math.min(wanted, window.innerHeight - menu.height - edge));
    this.root.style.setProperty('--pane-menu-x', `${Math.round(x)}px`);
    this.root.style.setProperty('--pane-menu-y', `${Math.round(y)}px`);
  }
  close(focus = false) {
    const wasOpen = this.opened;
    this.size?.disconnect(); this.size = null;
    this.visibility?.disconnect(); this.visibility = null;
    if (this.root.matches(':popover-open')) this.root.hidePopover(); this.root.hidden = true; this.opened = false;
    this.trigger?.setAttribute('aria-expanded', 'false'); if (focus && this.trigger?.isConnected) this.trigger.focus({ preventScroll: true });
    if (wasOpen) this.onClose();
  }
  dispose() { this.close(); this.events.abort(); }
}
