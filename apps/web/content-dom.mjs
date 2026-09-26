// Shared presentation helpers. No execution, filesystem authority or model state lives here.
import { paneIcon } from './pane-icons.mjs';
export function element(tag, className = '', text) {
  const node = document.createElement(tag); if (className) node.className = className;
  if (text !== undefined) node.textContent = text; return node;
}
export function button(label, action, className = 'text-button') {
  const node = element('button', className, label); node.type = 'button';
  if (action) node.addEventListener('click', action); return node;
}
export function contentAction(label, icon, action, className = '') {
  const control = button('', action, `message-icon-action ${className}`);
  control.setAttribute('aria-label', label); control.dataset.tooltip = label; control.append(paneIcon(icon));
  if (icon === 'copy') { const success = paneIcon('check'); success.classList.add('copy-success-icon'); control.append(success); }
  return control;
}
const copyFeedback = new WeakMap();
export async function copyText(value, trigger) {
  let feedback;
  if (trigger) {
    const previous = copyFeedback.get(trigger); clearTimeout(previous?.timer);
    feedback = { text: previous?.text ?? trigger.textContent, label: previous?.label ?? trigger.getAttribute('aria-label'), tooltip: previous?.tooltip ?? trigger.dataset.tooltip, icon: trigger.classList.contains('message-icon-action') };
    copyFeedback.set(trigger, feedback);
  }
  const show = (label, state, delay) => {
    if (!trigger || copyFeedback.get(trigger) !== feedback) return;
    if (feedback.icon) { trigger.dataset.copyState = state; trigger.setAttribute('aria-label', label); trigger.dataset.tooltip = label; }
    else trigger.textContent = label;
    feedback.timer = setTimeout(() => {
      if (copyFeedback.get(trigger) !== feedback) return;
      copyFeedback.delete(trigger);
      if (feedback.icon) { delete trigger.dataset.copyState; if (feedback.label != null) trigger.setAttribute('aria-label', feedback.label); if (feedback.tooltip != null) trigger.dataset.tooltip = feedback.tooltip; }
      else trigger.textContent = feedback.text;
    }, delay);
  };
  try {
    if (navigator.clipboard?.writeText) await navigator.clipboard.writeText(value);
    else {
      const active = document.activeElement, area = element('textarea', 'copy-fallback'); area.value = value; area.readOnly = true; document.body.append(area);
      try { area.select(); if (!document.execCommand('copy')) throw new Error('clipboard unavailable'); }
      finally { area.remove(); if (active?.isConnected) active.focus({ preventScroll: true }); }
    }
    show('已复制', 'success', 1200);
    return true;
  } catch { show('复制失败', 'failed', 1800); return false; }
}
export function downloadContent(value, type, name) {
  const url = URL.createObjectURL(new Blob([value], { type })), link = element('a'); link.href = url; link.download = name; link.hidden = true;
  document.body.append(link); link.click(); link.remove(); setTimeout(() => URL.revokeObjectURL(url), 5000);
}
const contentPreviews = new Map();
export function closeContentPreviews(owner) {
  for (const [dialog, source] of contentPreviews) if (owner === source || owner?.contains(source)) dialog.close();
}
export function previewDialog(title, content, { owner } = {}) {
  if (owner && (!owner.isConnected || owner.closest('[hidden],[inert]'))) return null;
  if (owner) closeContentPreviews(owner);
  const active = document.activeElement, dialog = element('dialog', 'content-preview-dialog');
  const header = element('header', 'content-preview-header'), label = element('strong', '', title), close = button('关闭', () => dialog.close());
  const id = `content-dialog-${crypto.randomUUID()}`; label.id = id; dialog.setAttribute('aria-labelledby', id);
  header.append(label, close); const body = element('div', 'content-preview-body'); body.append(content); dialog.append(header, body); document.body.append(dialog);
  dialog.addEventListener('click', event => { if (event.target === dialog) { const r = dialog.getBoundingClientRect(); if (event.clientX < r.left || event.clientX > r.right || event.clientY < r.top || event.clientY > r.bottom) dialog.close(); } });
  const visibility = owner ? new MutationObserver(() => {
    if (!owner.isConnected || owner.closest('[hidden],[inert]')) dialog.close();
  }) : null;
  if (owner) {
    contentPreviews.set(dialog, owner);
    visibility.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['hidden', 'inert'] });
  }
  dialog.addEventListener('close', () => {
    visibility?.disconnect(); contentPreviews.delete(dialog); dialog.remove();
    if (active?.isConnected && !active.closest('[hidden],[inert]') && active.getClientRects().length) active.focus({ preventScroll: true });
  }, { once: true });
  dialog.showModal(); close.focus(); return dialog;
}
export function zoomPreview(content, { stepZoom = false } = {}) {
  const root = element('section', 'content-zoom-preview'), controls = element('div', 'content-preview-controls'), area = element('div', 'content-zoom-area'); area.dataset.zoom = '100'; area.append(content);
  if (stepZoom) {
    let zoom = 100;
    const reading = element('span', 'content-zoom-reading', '100%');
    const minus = contentAction('缩小图片', 'minus', () => change(-50));
    const plus = contentAction('放大图片', 'add', () => change(50));
    const change = delta => { zoom = Math.max(50, Math.min(300, zoom + delta)); area.dataset.zoom = String(zoom); reading.textContent = `${zoom}%`; minus.disabled = zoom === 50; plus.disabled = zoom === 300; };
    controls.append(minus, reading, plus);
  } else for (const [label, zoom] of [['50%', '50'], ['适应', '100'], ['150%', '150'], ['200%', '200']]) controls.append(button(label, () => { area.dataset.zoom = zoom; }));
  root.append(controls, area); return root;
}
export function safeWebUrl(value) {
  if (typeof value !== 'string' || /[\u0000-\u0020\u007f]/.test(value)) return null;
  try { const url = new URL(value); return ['http:', 'https:', 'mailto:'].includes(url.protocol) && !url.username && !url.password ? url.href : null; } catch { return null; }
}
export function appendPlain(node, value) {
  const child = node.firstChild;
  if (child?.nodeType === 3 && node.childNodes.length === 1) {
    const old = child.data; if (old === value) return;
    if (value.startsWith(old)) child.appendData(value.slice(old.length)); else child.replaceData(0, old.length, value);
  } else node.replaceChildren(document.createTextNode(value));
}
export function csvText(rows) { return '\uFEFF' + rows.map(row => row.map(value => {
  value = String(value); if (/^[\t\r\n]|^\s*[=+\-@]/u.test(value)) value = "'" + value;
  return /[",\r\n]/.test(value) ? `"${value.replace(/"/g, '""')}"` : value;
}).join(',')).join('\r\n'); }
