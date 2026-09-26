import { element as el, button } from './content-dom.mjs';
import { paneIcon } from './pane-icons.mjs';
const pending = []; let running = 0;
function schedule(action, signal) {
  if (signal.aborted) return Promise.reject(new DOMException('已取消', 'AbortError'));
  if (pending.length >= 128) return Promise.reject(new Error('文件检查队列已满，请稍后重新检查。'));
  return new Promise((resolve, reject) => { pending.push({ action, signal, resolve, reject }); drain(); });
}
function drain() {
  while (running < 3 && pending.length) {
    const job = pending.shift(); if (job.signal.aborted) { job.reject(new DOMException('已取消', 'AbortError')); continue; }
    running++; Promise.resolve().then(job.action).then(job.resolve, job.reject).finally(() => { running--; drain(); });
  }
}
export function fileReferenceCards(paths, options) {
  const root = el('div', 'message-file-cards'); const controller = new AbortController(); let disposed = false;
  root.setAttribute('aria-label', '回答中的文件'); root.dispose = () => { disposed = true; controller.abort(); };
  for (const path of [...new Set(paths)].slice(0, 8)) {
    const card = el('section', 'message-file-card'), title = el('strong', '', path.replace(/#L\d+(?:-L?\d+)?$/, '').split(/[\\/]/).at(-1) || path), meta = el('span', 'muted-note', '正在确认文件…');
    title.dataset.tooltip = path;
    const open = button('预览', () => { open.disabled = true; Promise.resolve().then(() => options.openFile(path)).catch(error => { meta.textContent = error.message; }).finally(() => { if (!disposed) open.disabled = false; }); }); open.disabled = true;
    const retry = button('重新检查', () => { void inspect(); }); retry.hidden = true;
    card.append(paneIcon('file'), title, meta, open, retry); root.append(card);
    async function inspect() {
      retry.hidden = true; open.hidden = false; meta.textContent = '正在确认文件…'; open.disabled = true;
      try {
        const entry = await schedule(() => options.describeFile(path, { signal: controller.signal }), controller.signal);
        if (disposed) return;
        if (entry.kind !== 'file') throw new Error('该引用不是可预览的文件。');
        meta.textContent = entry.bytes < 1024 ? `${entry.bytes} B` : entry.bytes < 1024 * 1024 ? `${(entry.bytes / 1024).toFixed(1)} KB` : `${(entry.bytes / 1048576).toFixed(1)} MB`; open.disabled = false;
      } catch (error) { if (!disposed && error.name !== 'AbortError') { meta.textContent = error.message; open.hidden = true; retry.hidden = false; } }
    }
    void inspect();
  }
  return root;
}
