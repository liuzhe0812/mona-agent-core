import { ProductClient } from '../client.mjs';
import { element as el } from '../dom.mjs';
import { parseMcpJson, validateMcpView } from '../mcp-view.mjs';
export const version=1;
export function mount(ctx) {
  const api=new ProductClient();let view=null,selected=null,busy=false,dirty=false,epoch=0,sequence=0;
  const tab=el('button','','MCP');tab.type='button';tab.id='settings-mcp-tab';
  const panel=el('section','settings-content mcp-settings');panel.id='mcp-settings-section';
  const heading=el('h2','','MCP'),intro=el('p','muted-note','连接已安装的本地程序或远程 MCP 服务。配置保存后重启生效；外部服务不受本机文件沙箱隔离，只启用你信任的服务。');
  const notice=el('p','settings-notice');notice.id='mcp-notice';notice.setAttribute('role','status');
  const toolbar=el('div','mcp-toolbar'),list=el('div','mcp-server-list'),form=el('form','mcp-form'),fields=new Map();
  const makeButton=(text,id,action)=>{const b=el('button','text-button',text);b.type='button';b.id=id;ctx.listen(b,'click',ctx.guard(action));return b;};
  const add=makeButton('添加服务','mcp-add',()=>edit(null));const refresh=makeButton('刷新状态','mcp-refresh',()=>load(true));toolbar.append(add,refresh);
  function field(parent,key,label,type='text',initial='') {
    const wrap=el('label',type==='checkbox'?'mcp-toggle':'mcp-field');const input=el(type==='textarea'?'textarea':type==='select'?'select':'input');input.id='mcp-'+key;
    if(!['textarea','select'].includes(type))input.type=type;
    if(type==='checkbox')wrap.append(input,el('span','',label));else wrap.append(el('span','',label),input);
    if(type==='textarea'){input.rows=3;input.spellcheck=false;}
    if(type!=='checkbox')input.value=initial;
    input.autocomplete='off';fields.set(key,input);parent.append(wrap);return input;
  }
  field(form,'id','服务 ID（字母、数字、下划线、连字符）').required=true;
  const transport=field(form,'transport','连接方式','select');for(const [value,text]of [['stdio','本地程序 · stdio'],['streamable-http','远程服务 · Streamable HTTP']]){const o=el('option','',text);o.value=value;transport.append(o);}
  field(form,'enabled','启用此服务','checkbox');
  const local=el('div','mcp-transport'),remote=el('div','mcp-transport');form.append(local,remote);
  field(local,'command','可执行文件（须已安装）');field(local,'args','参数（JSON 字符串数组）','textarea','[]');field(local,'cwd','启动目录（绝对路径；留空使用宿主启动目录）');
  field(local,'env','环境变量（JSON；留空保留已保存值）','textarea');
  field(remote,'url','MCP 端点 URL');field(remote,'headers','HTTP 标头（JSON；留空保留已保存值）','textarea');field(remote,'allow-http','允许非回环 HTTP（仅可信内网）','checkbox');
  const savedKeys=el('p','muted-note');savedKeys.id='mcp-saved-keys';form.append(savedKeys);
  field(form,'clear-secrets','清除旧环境变量 / 请求标头','checkbox');
  const timeout=field(form,'timeout','单次调用超时（毫秒）','number','60000');timeout.min='100';timeout.max='300000';timeout.required=true;
  field(form,'allow-tools','允许工具（原始名称 JSON 数组；null = 全部，[] = 不提供工具）','textarea','null');
  field(form,'deny-tools','排除工具（JSON 数组）','textarea','[]');
  field(form,'read-only-tools','宿主确认的只读工具（JSON 数组；不直接相信远程只读标记）','textarea','[]');
  const actions=el('div','mcp-toolbar'),save=makeButton('保存配置','mcp-save',()=>{});save.type='submit';save.className='primary';
  const remove=makeButton('删除服务','mcp-delete',()=>mutate('/api/mcp/delete',{revision:view.revision,id:selected}));
  const reconnect=makeButton('重新连接','mcp-reconnect',()=>mutate('/api/mcp/reconnect',{id:selected}));actions.append(save,remove,reconnect);form.append(actions);
  const tools=el('div','mcp-tools');tools.id='mcp-tools';panel.append(heading,intro,notice,toolbar,list,form,tools);form.hidden=true;
  function controls(){for(const input of fields.values())input.disabled=busy;fields.get('id').disabled=busy||selected!==null;for(const b of [add,refresh,save,remove,reconnect])b.disabled=busy||!view;remove.hidden=!selected;reconnect.hidden=!selected;reconnect.disabled=busy||!view?.enabled||!view?.status.some(s=>s.id===selected&&s.enabled);}
  function kind(){const stdio=transport.value==='stdio';local.hidden=!stdio;remote.hidden=stdio;}
  function stateText(state){return !state?'未装配，重启后生效':state.needs_restart?'工具定义已变化，需重启':!state.enabled?'未启用':state.connected?'已连接':state.error||'未连接';}
  function renderList(){list.replaceChildren();if(!view.servers.length)list.append(el('p','muted-note','尚未配置 MCP 服务。'));
    for(const s of view.servers){const b=el('button','mcp-server-row');b.type='button';b.id=`mcp-entry-${s.id}`;b.addEventListener('click',ctx.guard(()=>edit(s.id)));b.setAttribute('aria-pressed',String(selected===s.id));b.append(el('strong','',s.id),el('span','',stateText(view.status.find(x=>x.id===s.id))));list.append(b);}
  }
  function renderTools(){tools.replaceChildren();const state=view?.status.find(s=>s.id===selected);if(!state)return;tools.append(el('h3','','当前已装配工具'),el('p','muted-note',stateText(state)));
    for(const tool of state.tools){const row=el('div','mcp-tool-row');row.append(el('code','',tool.name),el('span','',tool.read_only?'只读（宿主确认）':'副作用工具'),el('p','',tool.description));tools.append(row);}
    if(state.resources)tools.append(el('p','muted-note','支持按需发现和读取资源。'));}
  function edit(id){if(busy)return;selected=id;dirty=false;form.hidden=false;
    const entry=view?.servers.find(s=>s.id===id),c=entry?.config,t=c?.transport;
    fields.get('id').value=id||'';transport.value=t?.type||'stdio';fields.get('enabled').checked=c?.enabled||false;
    for(const key of ['command','cwd','url'])fields.get(key).value=t?.[key]||'';
    fields.get('args').value=JSON.stringify(t?.args||[]);for(const key of ['env','headers'])fields.get(key).value='';
    fields.get('allow-http').checked=t?.allow_http||false;fields.get('clear-secrets').checked=false;timeout.value=c?.timeout_ms||60000;
    for(const key of ['allow-tools','deny-tools','read-only-tools'])fields.get(key).value=JSON.stringify(c?.[key.replaceAll('-','_')]??(key==='allow-tools'?null:[]));
    savedKeys.textContent=entry?`已保存的环境变量：${entry.env_keys.join('、')||'无'}；请求标头：${entry.header_keys.join('、')||'无'}。值不回传。`:'';
    kind();controls();renderList();renderTools();}
  function show(value,preserve=false){view=validateMcpView(value);notice.textContent=value.restart_required?'配置已保存，重启宿主后生效。':value.enabled?'已显示宿主实际配置和连接状态。':'MCP 组件未启用，可在 Agent 组件中启用后重启。';
    renderList();if(!preserve&&selected&&value.servers.some(s=>s.id===selected))edit(selected);else if(!preserve&&selected){selected=null;form.hidden=true;tools.replaceChildren();}controls();renderTools();}
  async function load(preserve=false){if(!ctx.connection()||busy)return;const own=epoch,read=++sequence;try{const data=await api.request('/api/mcp',null,{maxBytes:1024*1024});if(own===epoch&&read===sequence&&!ctx.signal.aborted)show(data,preserve||dirty);}catch(e){if(own===epoch&&!ctx.signal.aborted)notice.textContent=e.message;}}
  async function mutate(path,body){if(busy||!view)return;const own=epoch;busy=true;sequence++;controls();try{const data=await api.request(path,body,{maxBytes:1024*1024,maxRequestBytes:64*1024});if(own!==epoch||ctx.signal.aborted)return;busy=false;dirty=false;show(data);if(path.endsWith('/servers'))edit(body.id);}
    catch(e){if(own===epoch&&!ctx.signal.aborted)notice.textContent=`${e.message} 未自动重试；请刷新确认。`;}
    finally{if(own===epoch&&!ctx.signal.aborted){busy=false;controls();}}}
  ctx.listen(form,'input',()=>{dirty=true;});ctx.listen(transport,'change',kind);
  ctx.listen(form,'submit',ctx.guard(async event=>{event.preventDefault();if(busy||!view)return;try{
    const id=fields.get('id').value.trim();if(!/^[A-Za-z0-9_-]{1,32}$/.test(id))throw Error('服务 ID 应为 1–32 位字母、数字、下划线或连字符。');
    const get=k=>fields.get(k).value.trim();const t=transport.value==='stdio'?{type:'stdio',command:get('command'),args:parseMcpJson(get('args'),'list'),cwd:get('cwd')||null,env:parseMcpJson(get('env'),'object')}:{type:'streamable-http',url:get('url'),headers:parseMcpJson(get('headers'),'object'),allow_http:fields.get('allow-http').checked};
    await mutate('/api/mcp/servers',{revision:view.revision,id,clear_secrets:fields.get('clear-secrets').checked,server:{enabled:fields.get('enabled').checked,transport:t,timeout_ms:Number(timeout.value),allow_tools:parseMcpJson(get('allow-tools'),'filter'),deny_tools:parseMcpJson(get('deny-tools'),'list'),read_only_tools:parseMcpJson(get('read-only-tools'),'list')}});
  }catch(e){notice.textContent=e.message;}}));
  ctx.register('settings',{id:'mcp',order:46,value:{tab,panel,activate:()=>load(dirty)}});
  ctx.on('connection',connection=>{epoch++;sequence++;view=null;selected=null;busy=false;dirty=false;form.reset();form.hidden=true;list.replaceChildren();tools.replaceChildren();notice.textContent='';for(const key of ['env','headers'])fields.get(key).value='';if(connection)api.configure(connection.base,connection.bearer);else api.clear();controls();});
  ctx.own(()=>{epoch++;api.clear();view=null;for(const key of ['env','headers'])fields.get(key).value='';});
}
