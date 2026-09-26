// Pure display policy. Never changes tool authority, execution mode or recorded messages.
export const WORK_POLICY = Object.freeze({ detail: true, openLiveGroups: false, unfold: false });
export const workPolicy = () => WORK_POLICY;
export function category(name) { return ({ read: 'read', read_file: 'read', ls: 'read', write: 'write', write_file: 'write', edit: 'write', edit_file: 'write', shell: 'shell', exec_command: 'shell', bash: 'shell', powershell: 'shell', grep: 'search', find: 'search' })[name] || 'tool'; }
export function workSummary(items, { running = false, detail = true, partial = false } = {}) {
  const tools = items.filter(item => item.content.kind === 'tool_call');
  const active = [...tools].reverse().find(item => ['pending', 'running'].includes(item.state));
  if (running && active) {
    const c = active.content, kind = category(c.name), preparing = active.state === 'pending';
    const verb = ({ read: '读取文件', write: '修改文件', search: '搜索', shell: '运行命令', tool: c.name || '调用工具' })[kind];
    const a = c.arguments; const preview = detail && !preparing && a && typeof a === 'object' ? a.path ?? a.command ?? a.query ?? a.pattern : '';
    return `${preparing ? '正在准备' : '正在'}${verb}${typeof preview === 'string' && preview ? ' · ' + preview.split('\n')[0].slice(0, 100) : ''}`;
  }
  if (running) return '正在生成回复';
  const counts = new Map(); for (const item of tools) { const kind = category(item.content.name); counts.set(kind, (counts.get(kind) || 0) + 1); }
  const noun = { read: '次读取', write: '次修改', search: '次搜索', shell: '条命令', tool: '项工具调用' };
  const title = [...counts].map(([kind, n]) => `${n} ${noun[kind]}`).join('、') || '工作过程';
  const failed = tools.filter(item => ['failed', 'denied', 'unknown', 'cancelled'].includes(item.state)).length;
  return `${partial ? '已加载：' : ''}${title}${failed ? ` · ${failed} 项未成功` : ''}`;
}
export function canFoldTurn(state, hasSupplemental = false) { return state?.outcome?.status === 'completed' && !hasSupplemental; }
