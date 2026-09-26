const id = value => typeof value === 'string' && /^[A-Za-z0-9_.-]{1,128}$/.test(value);
const integer = value => Number.isSafeInteger(value) && value >= 0;
const states = new Set(['idle','running','completed','failed','cancelled','timed_out','limited','interrupted']);
export const childStatus = value => ({idle:'未开始',running:'执行中',completed:'已完成',failed:'失败',cancelled:'已停止',timed_out:'超时',limited:'达到限制',interrupted:'已中断'})[value] || '未知';
const encoder = new TextEncoder();
const named = value => typeof value === 'string' && /^[A-Za-z0-9_.-]{1,64}$/.test(value);
const bounded = (value, max) => typeof value === 'string' && encoder.encode(value).length <= max;
function validateRole(role) {
  if (!role || !bounded(role.description,512) || !bounded(role.instructions,8192)
      || (role.tools !== null && (!Array.isArray(role.tools) || role.tools.length>128 || !role.tools.every(named) || new Set(role.tools).size!==role.tools.length))
      || (role.model !== null && (!named(role.model?.provider_id) || !bounded(role.model?.model_id,512) || !role.model.model_id || /[\u0000-\u001f\u007f]/.test(role.model.model_id)))) throw Error('子 Agent 角色配置无效或超过容量。');
  return role;
}
export function roleFromFields({description,instructions,provider,model,tools,inheritTools}) {
  provider=provider.trim();model=model.trim();
  if(Boolean(provider)!==Boolean(model))throw Error('供应商与模型 ID 必须同时填写，或同时留空。');
  return validateRole({description,instructions,model:provider?{provider_id:provider,model_id:model}:null,
    tools:inheritTools?null:[...new Set(tools.split(',').map(s=>s.trim()).filter(Boolean))]});
}
export function validateSettings(value) {
  if(value?.version!==1 || !integer(value.revision) || ['enabled','locked','restart_required'].some(k=>typeof value[k]!=='boolean'))throw Error('子 Agent 设置响应无效。');
  for(const config of [value.config,value.active]) {
    for(const [key,max] of [['max_parallel',16],['max_children',128],['max_depth',4],['max_active_parents',1024]])
      if(!Number.isSafeInteger(config?.[key])||config[key]<1||config[key]>max)throw Error('子 Agent 容量配置无效。');
    if(!config.roles||Array.isArray(config.roles)||!Object.hasOwn(config.roles,'default')||Object.keys(config.roles).length>16)throw Error('子 Agent 角色清单无效。');
    for(const [name,role]of Object.entries(config.roles)){if(!named(name))throw Error('子 Agent 角色名称无效。');validateRole(role);}
  }
  return value;
}
export function validateChild(value, root) {
  if (!value || value.root !== root || !id(value.id) || !id(value.parent) || !id(value.role) || !states.has(value.status)
      || !integer(value.revision) || !integer(value.turns) || !integer(value.steps) || !integer(value.depth) || value.depth < 1 || value.depth > 4
      || typeof value.title !== 'string' || value.title.length > 2000 || (value.run_id !== null && !id(value.run_id))
      || !integer(value.usage?.model_calls) || !integer(value.usage?.reported_tokens) || typeof value.usage?.usage_complete !== 'boolean'
      || typeof value.output_truncated !== 'boolean' || (value.output !== null && (typeof value.output !== 'string' || value.output.length > 8192))
      || (value.model !== null && (!named(value.model?.provider_id) || !bounded(value.model?.model_id,512) || !value.model.model_id))) throw Error('子任务响应身份或状态无效。');
  return value;
}
export function validateChildren(value, root) {
  if (value?.version !== 1 || value.root !== root || typeof value.enabled !== 'boolean' || (value.parent_run !== null && !id(value.parent_run))
      || !Array.isArray(value.agents) || value.agents.length > 128) throw Error('子任务列表无效。');
  const ids = new Set();
  for (const child of value.agents) { validateChild(child, root); if (ids.has(child.id)) throw Error('子任务身份重复。'); ids.add(child.id); }
  return value;
}
export function validateChildPage(value, root, child) {
  if (value?.version !== 1 || value.root !== root || value.agent?.id !== child || !Array.isArray(value.page?.turns)
      || value.page.turns.length > 50 || typeof value.page.older_turns !== 'boolean') throw Error('子任务详情响应无效。');
  validateChild(value.agent, root);
  for (const turn of value.page.turns) if (!id(turn.id) || typeof turn.prompt !== 'string' || !states.has(turn.status)) throw Error('子任务轮次无效。');
  const history = value.page.history;
  if (history && (!id(history.turn?.id) || !value.page.turns.some(t => t.id === history.turn.id)
      || history.snapshot?.run_id !== (history.turn.run_id || history.turn.id) || !Array.isArray(history.snapshot?.items)
      || history.snapshot.items.length > 30 || (history.next_before !== null && !integer(history.next_before)))) throw Error('子任务执行片段无效。');
  return value;
}
