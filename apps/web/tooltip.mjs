// One shared presentation for existing hints, including dynamically rendered rows.
const tip = document.createElement('div');
tip.id = 'mona-tooltip';
tip.className = 'ui-tooltip';
tip.setAttribute('role', 'tooltip');
tip.setAttribute('popover', 'manual');
tip.hidden = true;
document.body.append(tip);
let target = null, openTimer = 0, closeTimer = 0;
const owner = node => node instanceof Element ? node.closest('[data-tooltip]') : null;

function hide() {
  clearTimeout(openTimer); clearTimeout(closeTimer);
  if (target) {
    const ids = (target.getAttribute('aria-describedby') || '').split(/\s+/).filter(id => id && id !== tip.id);
    if (ids.length) target.setAttribute('aria-describedby', ids.join(' '));
    else target.removeAttribute('aria-describedby');
  }
  target = null;
  if (tip.matches(':popover-open')) tip.hidePopover();
  tip.hidden = true;
}

function position() {
  const anchor = target.getBoundingClientRect(), box = tip.getBoundingClientRect();
  const gap = 8, width = document.documentElement.clientWidth, height = document.documentElement.clientHeight;
  const above = anchor.top - box.height - gap;
  tip.style.left = `${Math.max(gap, Math.min(anchor.left + (anchor.width - box.width) / 2, width - box.width - gap))}px`;
  tip.style.top = `${Math.max(gap, Math.min(above >= gap ? above : anchor.bottom + gap, height - box.height - gap))}px`;
}

function show() {
  if (!target?.isConnected || target.closest('[hidden], [inert]') || !target.getClientRects().length || !target.dataset.tooltip?.trim()) { hide(); return; }
  tip.textContent = target.dataset.tooltip;
  // Keep modal hints in their dialog's accessible subtree and above overflow clips.
  const parent = target.closest('dialog[open]') || document.body;
  if (tip.parentElement !== parent) parent.append(tip);
  tip.hidden = false;
  if (tip.showPopover && !tip.matches(':popover-open')) tip.showPopover();
  position();
  const ids = new Set((target.getAttribute('aria-describedby') || '').split(/\s+/).filter(Boolean));
  ids.add(tip.id);
  target.setAttribute('aria-describedby', [...ids].join(' '));
}

function schedule(next, delay = 350) {
  clearTimeout(closeTimer);
  if (next === target) return;
  hide();
  if (!next?.dataset.tooltip?.trim()) return;
  target = next;
  openTimer = setTimeout(show, delay);
}

function leave(event) {
  if (!target || target.contains(event.relatedTarget) || tip.contains(event.relatedTarget)) return;
  clearTimeout(closeTimer);
  closeTimer = setTimeout(hide, 120);
}

document.addEventListener('pointerover', event => {
  if (event.pointerType === 'touch') return;
  if (tip.contains(event.target)) { clearTimeout(closeTimer); return; }
  const next = owner(event.target);
  if (next) schedule(next);
  else leave(event);
}, true);
document.addEventListener('pointerout', leave, true);
document.addEventListener('focusin', event => {
  if (event.target.matches?.(':focus-visible')) schedule(owner(event.target), 0);
});
document.addEventListener('focusout', leave);
document.addEventListener('pointerdown', hide, true);
document.addEventListener('contextmenu', hide, true);
document.addEventListener('keydown', event => { if (event.key === 'Escape') hide(); }, true);
document.addEventListener('scroll', event => { if (!tip.contains(event.target)) hide(); }, true);
window.addEventListener('resize', hide);
window.addEventListener('blur', hide);
new MutationObserver(() => {
  if (!target) return;
  if (!target.isConnected || target.closest('[hidden], [inert]') || !target.getClientRects().length) { hide(); return; }
  if (!tip.hidden && tip.textContent !== target.dataset.tooltip) show();
}).observe(document.body, { subtree: true, childList: true, attributes: true, attributeFilter: ['data-tooltip', 'hidden', 'inert', 'open'] });
