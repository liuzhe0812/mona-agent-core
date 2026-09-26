import { element as el } from '../dom.mjs';
import { planContent } from '../plan-view.mjs';
import { parseDiff, diffContent } from '../../diff-view.mjs';
import { SpillClient } from '../../spill.mjs';
export const version = 1;
const button = (label, action, className = 'text-button') => { const n = el('button',className,label); n.type='button'; n.addEventListener('click',action); return n; };
function diffView(detail, options) {
  if (!detail || typeof detail !== 'object') return null;
  const wrap = el('div', 'structured-diff'), head = el('div', 'structured-detail-head');
  const name = typeof detail.path === 'string' && detail.path ? detail.path : '文件变更';
  head.append(options.openFile && detail.path ? button(name, () => { Promise.resolve().then(() => options.openFile(name)).catch(error => options.onError?.(error.message)); }, 'message-file-link') : el('strong','',name));
  if (Number.isInteger(detail.firstChangedLine)) head.append(el('span','muted-note',`第 ${detail.firstChangedLine} 行附近`));
  wrap.append(head);
  if (typeof detail.diff === 'string') {
    let split = false; const view = el('div','tool-diff-content'), stats = parseDiff(detail.diff);
    head.append(el('span','muted-note',`+${stats.added} −${stats.removed}${stats.truncated || detail.diffTruncated ? '（预览）' : ''}`));
    const toggle = button('分栏', () => { split = !split; toggle.textContent = split ? '合并' : '分栏'; toggle.setAttribute('aria-pressed',String(split)); view.replaceChildren(diffContent(detail.diff,{split})); }); toggle.setAttribute('aria-pressed','false');
    const copy = button('复制差异', () => { navigator.clipboard.writeText(detail.diff).then(()=>{copy.textContent='已复制';}).catch(()=>{copy.textContent='复制失败';}); }); head.append(toggle,copy);
    view.append(diffContent(detail.diff)); wrap.append(view);
  }
  if (detail.diffTruncated) wrap.append(el('p','muted-note','差异较长，仅显示已保存的预览。'));
  return wrap;
}
export function mount(ctx) {
  ctx.register('tool.view', { id:'coding.diff', value:diffView });
  ctx.register('tool.view', { id:'planner.plan', value:planContent });
  const artifacts = new SpillClient(); ctx.provide('artifacts',artifacts); ctx.own(()=>artifacts.clear());
  ctx.register('presentation',{id:'artifacts',order:-10,value:()=>({artifactReader:artifacts})});
  ctx.on('connection',connection=>{if(connection)artifacts.configure(connection.base,connection.bearer);else artifacts.clear();});
}
