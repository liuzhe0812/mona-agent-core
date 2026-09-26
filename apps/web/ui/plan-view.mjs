import { element as el } from './dom.mjs';
import { renderMessage, disposeMessage } from '../content-renderer.mjs';
const bytes = value => new TextEncoder().encode(value).length;
export function validatePlan(value) {
  if (!value || value.version !== 1 || !Number.isSafeInteger(value.revision) || value.revision < 0
      || !['normal','plan_only'].includes(value.mode) || typeof value.goal !== 'string' || bytes(value.goal) > 2048
      || !Array.isArray(value.steps) || value.steps.length > 32 || JSON.stringify(value).length > 32 * 1024) throw new Error('计划状态无效或版本不受支持。');
  const ids = new Set(); let active = 0;
  for (const step of value.steps) {
    if (!step || typeof step.id !== 'string' || !/^[A-Za-z0-9_-]{1,32}$/.test(step.id) || ids.has(step.id)
        || typeof step.text !== 'string' || !step.text.trim() || bytes(step.text) > 1024 || !['pending','in_progress','completed'].includes(step.status)) throw new Error('计划步骤数据无效。');
    ids.add(step.id); if (step.status === 'in_progress') active++;
  }
  if (active > 1 || (!value.goal) !== (!value.steps.length)) throw new Error('计划步骤状态不一致。');
  for (const [key, limit] of [['explanation',2048],['proposal',6144]]) if (value[key] != null && (typeof value[key] !== 'string' || bytes(value[key]) > limit)) throw new Error('计划正文超过限制。');
  if (bytes(JSON.stringify(value)) > 12 * 1024) throw new Error('计划快照超过限制。');
  return value;
}
export function planContent(value, presentation = {}) {
  const state = validatePlan(value), root = el('section', 'planner-content'); root.dataset.contentOwned = 'plan';
  root.append(el('strong', 'planner-goal', state.goal || '尚未创建计划'));
  const list = el('ol', 'planner-steps');
  for (const step of state.steps) {
    const item = el('li', `planner-step planner-step-${step.status}`); item.dataset.planStep = step.id;
    item.append(el('span','planner-step-state',({pending:'待处理',in_progress:'进行中',completed:'已完成'})[step.status]),el('span','',step.text)); list.append(item);
  }
  root.append(list);
  if (state.explanation) root.append(el('p','planner-explanation',`调整原因：${state.explanation}`));
  if (state.proposal) { const proposal = el('div','message-text planner-proposal'); root.append(proposal); void renderMessage(proposal,state.proposal,presentation); }
  root.append(el('p','muted-note','步骤状态由 Agent 记录，不代表独立业务验收；中断后不会自动继续。'));
  root.dispose = () => { for (const node of root.querySelectorAll('.message-text')) disposeMessage(node); };
  return root;
}
