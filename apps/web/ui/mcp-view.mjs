const id = value => typeof value === 'string' && /^[A-Za-z0-9_-]{1,32}$/.test(value);
export function parseMcpJson(text, kind) {
  let value; try { value=JSON.parse(String(text).trim() || (kind==='filter'?'null':kind==='list'?'[]':'{}')); } catch { throw Error('请填写有效 JSON。'); }
  if (kind==='filter' && value===null) return null;
  if (kind==='list'||kind==='filter') {
    if (!Array.isArray(value)||value.length>128||value.some(s=>typeof s!=='string')) throw Error('此字段应为字符串数组；允许工具可填 null 表示全部。');
  } else if (!value||typeof value!=='object'||Array.isArray(value)||Object.keys(value).length>64||Object.values(value).some(s=>typeof s!=='string')) throw Error('此字段应为字符串键值对象。');
  if(new TextEncoder().encode(JSON.stringify(value)).length>64*1024)throw Error('配置字段不能超过 64 KiB。');return value;
}
export function validateMcpView(value) {
  if(value?.version!==1||!Number.isSafeInteger(value.revision)||value.revision<0||typeof value.enabled!=='boolean'||typeof value.restart_required!=='boolean'||!Array.isArray(value.servers)||value.servers.length>16||!Array.isArray(value.status)||value.status.length>16)throw Error('MCP 设置响应无效。');
  const seen=new Set();
  for(const server of value.servers) {
    if(!id(server.id)||seen.has(server.id)||typeof server.config?.enabled!=='boolean'||!['stdio','streamable-http'].includes(server.config.transport?.type)||!Array.isArray(server.env_keys)||!Array.isArray(server.header_keys))throw Error('MCP 服务配置响应无效。');
    seen.add(server.id);
    if('env' in server.config.transport||'headers' in server.config.transport)throw Error('MCP 响应不应包含已保存的凭据。');
  }
  for(const state of value.status) {
    if(!id(state.id)||typeof state.connected!=='boolean'||typeof state.enabled!=='boolean'||typeof state.needs_restart!=='boolean'||!Array.isArray(state.tools)||state.tools.length>64)throw Error('MCP 状态响应无效。');
    for(const t of state.tools)if(typeof t.name!=='string'||t.name.length>64||typeof t.remote_name!=='string'||typeof t.description!=='string'||typeof t.read_only!=='boolean')throw Error('MCP 工具响应无效。');
  }
  return value;
}
