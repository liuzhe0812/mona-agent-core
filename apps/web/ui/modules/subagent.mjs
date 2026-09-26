import { ProductClient } from '../client.mjs';
import { element as el } from '../dom.mjs';
import { childStatus, validateChildren, validateChildPage, validateSettings, roleFromFields } from '../subagent-view.mjs';
import { TurnView } from '../../run-view.mjs';
import { RunView, requestId } from '../../../../packages/client/src/index.mjs';
export const version = 1;
const sid = value => { if (typeof value !== 'string' || !/^[A-Za-z0-9_-]{1,64}$/.test(value)) throw Error('会话身份无效。'); return value; };
const button = (label, action) => { const n = el('button','text-button',label); n.type='button'; if(action)n.addEventListener('click',action); return n; };
export function mount(ctx) {
  const api = new ProductClient(), pane = ctx.shell.pane(ctx.id), panels = new Map();
  let current = '', epoch = 0, timer = null, settings = null, saving = false, settingsRead = 0, draft = null, roleId = '';
  const enabled = () => ctx.capability('subagent')?.active === true;
  const base = root => `/api/sessions/${sid(root)}/agents`;
  const valid = (p, generation) => !ctx.signal.aborted && generation === epoch && panels.get(p.id) === p;
  function makePanel(id) {
    const root=el('section','subagent-panel'); root.dataset.subagentRoot=id;
    const toolbar=el('div','subagent-toolbar'), notice=el('p','subagent-notice'), list=el('div','subagent-list'); notice.setAttribute('role','status');
    const detail=el('section','subagent-detail'), turns=el('select','subagent-turns'); turns.setAttribute('aria-label','子任务轮次');
    const summary=el('p','subagent-notice'), history=el('div','subagent-history');
    const older=button('较早过程'), latest=button('返回最新过程'), input=el('textarea','subagent-message'); input.rows=2;input.maxLength=16000;input.placeholder='给选中的子 Agent 补充信息或任务';input.setAttribute('aria-label','子任务补充内容');
    const message=button('补充信息'), followup=button('继续子任务'), interrupt=button('停止子任务');
    const actions=el('div','subagent-actions'); actions.append(message,followup,interrupt);
    toolbar.append(button('刷新',ctx.guard(()=>read(p))),button('子 Agent 设置',()=>ctx.shell.showSettings('subagent')));
    detail.append(turns,summary,history,older,latest,input,actions); detail.hidden=true; root.append(toolbar,notice,list,detail);
    const p={id,root,notice,list,detail,turns,summary,history,older,latest,input,message,followup,interrupt,rows:new Map(),data:null,selected:null,historyTurn:null,before:null,page:null,view:null,signature:'',read:null,detailRead:null,mutating:false};
    turns.addEventListener('change',ctx.guard(()=>{p.historyTurn=turns.value;return readDetail(p,turns.value);}));
    older.addEventListener('click',ctx.guard(()=>readDetail(p,p.page?.page.history?.turn.id,p.page?.page.history?.next_before)));
    latest.addEventListener('click',ctx.guard(()=>{p.historyTurn=null;return readDetail(p);}));
    message.addEventListener('click',ctx.guard(()=>act(p,'message')));followup.addEventListener('click',ctx.guard(()=>act(p,'followup')));interrupt.addEventListener('click',ctx.guard(()=>act(p,'interrupt')));
    return p;
  }
  function controls(p) {
    const child=p.page?.agent, running=child?.status==='running', live=Boolean(p.data?.enabled && p.data?.parent_run && current===p.id && !p.mutating);
    p.message.disabled=!child||!live;
    p.followup.disabled=!child||running||p.mutating||current!==p.id||!enabled();
    p.followup.textContent=p.data?.parent_run?'继续子任务':'交给主 Agent 继续';
    p.interrupt.disabled=!child||!running||p.mutating||current!==p.id;
    p.input.disabled=p.mutating||current!==p.id;
  }
  function draw(p) {
    const alive=new Set();
    for(const child of p.data?.agents||[]) {
      alive.add(child.id);let row=p.rows.get(child.id);
      if(!row) { row=button('',ctx.guard(async()=>{p.selected=child.id;p.historyTurn=null;p.before=null;p.input.value='';p.page=null;p.signature='';await readDetail(p);draw(p);}));row.className='subagent-row';row.dataset.child=child.id;p.rows.set(child.id,row);p.list.append(row); }
      const signature=JSON.stringify([child,p.selected]);
      if(row.dataset.signature!==signature) {
        row.dataset.signature=signature;row.setAttribute('aria-pressed',String(p.selected===child.id));
        const title=el('strong','',child.title||child.id), status=el('span','subagent-row-status',childStatus(child.status));
        const meta=el('small','',`${child.role} · ${child.model?.model_id||'继承父任务模型'} · ${child.usage.model_calls} 次调用 · ${child.usage.usage_complete?'':'≥'}${child.usage.reported_tokens} tokens`);
        row.replaceChildren(title,status,meta);
      }
    }
    for(const [id,row] of p.rows)if(!alive.has(id)){row.remove();p.rows.delete(id);}
    if(!alive.size && !p.notice.textContent)p.notice.textContent='暂无子任务。主 Agent 按需要委派，不会为普通任务强制创建子 Agent。';
    p.detail.hidden=!p.selected;controls(p);
  }
  async function read(p) {
    if(!ctx.connection()||p.read)return p.read;
    const generation=epoch;
    const job=(async()=>{
      try {
        const data=validateChildren(await api.request(base(p.id),null,{maxBytes:256*1024}),p.id);
        if(!valid(p,generation))return;
        p.data=data;p.notice.textContent=data.enabled?'':'子 Agent 未启用，已保存任务仍可查看。';draw(p);
        if(p.selected&&!p.detailRead)await readDetail(p,p.historyTurn,p.before);
      }catch(error){if(valid(p,generation)&&error.name!=='AbortError')p.notice.textContent=error.message;}
    })();p.read=job;try{await job;}finally{if(p.read===job)p.read=null;}
  }
  async function readDetail(p, turn, before) {
    if(!p.selected||!ctx.connection())return;
    p.detailRead?.abort();const controller=new AbortController();p.detailRead=controller;
    const selected=p.selected,generation=epoch,query=new URLSearchParams();if(turn)query.set('turn',turn);if(before!=null)query.set('before',String(before));
    try {
      const value=validateChildPage(await api.request(`${base(p.id)}/${sid(selected)}${query.size?'?'+query:''}`,null,{signal:controller.signal,maxBytes:512*1024}),p.id,selected);
      if(!valid(p,generation)||p.selected!==selected||controller.signal.aborted)return;
      p.page=value;p.before=before??null;const history=value.page.history;
      if(before!=null&&history)p.historyTurn=history.turn.id;
      const signature=JSON.stringify([value.agent.revision,history?.turn.id,before]);
      if(signature!==p.signature) {
        p.signature=signature;
        p.turns.replaceChildren(...value.page.turns.map(t=>{const n=el('option','',`${childStatus(t.status)} · ${t.prompt.slice(0,60)}`);n.value=t.id;return n;}));
        if(history)p.turns.value=history.turn.id;p.turns.hidden=!history;
        p.view?.dispose?.();p.history.replaceChildren();p.view=null;
        if(history){const view=new RunView(history.snapshot.run_id);view.apply({kind:'snapshot',reason:'initial',snapshot:history.snapshot});
          const dom=new TurnView(history.turn.prompt,{artifactReader:ctx.service('artifacts'),...ctx.shell.presentation({id:selected})});dom.update(view.state);p.history.append(dom.turn);p.view=dom;}
        p.summary.textContent=`${childStatus(value.agent.status)} · ${value.agent.model?.model_id||'父任务绑定模型'}${value.page.older_turns?' · 仅列出最近 50 轮':''}${value.agent.output_truncated?' · 结果已截断，查看下方过程':''}`;
        p.older.hidden=history?.next_before==null;p.latest.hidden=before==null;
      }
      controls(p);
    }catch(error){if(valid(p,generation)&&p.selected===selected&&error.name!=='AbortError')p.notice.textContent=error.message;}
    finally{if(p.detailRead===controller)p.detailRead=null;}
  }
  async function act(p,action) {
    const child=p.page?.agent,id=p.selected,generation=epoch;
    if(p.mutating||!child||!id||current!==p.id)return;
    const text=p.input.value.trim(), parentRun=p.data?.parent_run;
    if(action!=='interrupt'&&!text){p.notice.textContent='请先填写补充信息或后续任务。';p.input.focus();return;}
    if(action==='followup'&&!parentRun) {
      if(ctx.shell.draft().trim()){p.notice.textContent='主输入框已有草稿，请先处理草稿。';return;}
      p.mutating=true;controls(p);
      try{await ctx.shell.submitText(`请继续子 Agent ${id} 的任务：\n${text}`,p.id);}
      finally{p.mutating=false;if(valid(p,generation))controls(p);}return;
    }
    p.mutating=true;controls(p);
    try {
      const body=action==='interrupt'?{run_id:child.run_id}:{parent_run:parentRun,request_id:requestId(),text};
      await api.request(`${base(p.id)}/${sid(id)}/${action}`,body);
      if(!valid(p,generation)||p.selected!==id)return;
      if(action!=='interrupt'&&p.input.value.trim()===text)p.input.value='';
      if(action==='followup')p.historyTurn=null;
      await read(p);p.notice.textContent=action==='message'?'信息已保存，将在子 Agent 下一次模型请求时提供；未额外启动任务。':action==='interrupt'?'子任务已停止；已发生的文件修改不会回滚。':'已启动同一子 Agent 的下一轮。';
    }catch(error){if(valid(p,generation))p.notice.textContent=`${error.message} 未自动重试，请刷新确认。`;}
    finally{p.mutating=false;if(valid(p,generation))controls(p);}
  }
  async function openPanel() {
    if(!current)return;const id=current;let p=panels.get(id);
    if(!p){p=makePanel(id);panels.set(id,p);}
    pane.openTab({id:'subagents',title:'子 Agent',kind:'subagents',scope:`session:${id}`,node:p.root,
      onActivate:async()=>{await read(p);schedule();},onClose:()=>{p.detailRead?.abort();p.view?.dispose?.();panels.delete(id);}});
    await read(p);schedule();
  }
  function schedule() {
    const p=panels.get(current);
    if(ctx.signal.aborted||document.hidden||!p||!p.root.getClientRects().length){clearTimeout(timer);timer=null;return;}
    if(timer!=null)return;
    timer=setTimeout(async()=>{timer=null;await read(p);schedule();},1000);
  }
  ctx.register('right.pane',{id:'subagents',order:15,value:{label:'子 Agent',icon:'side',available:()=>Boolean(current),run:openPanel}});
  // A status contribution is shown only when actual subagent tools run; it is not a second task control plane.
  const detailRenderer=value=>{const n=button('查看子 Agent',ctx.guard(openPanel));n.disabled=!current||value?.root!==current;return n;};
  ctx.register('tool.view',{id:'subagent.status',value:detailRenderer});

  const tab=button('子 Agent'), panel=el('section','settings-content subagent-settings');tab.id='settings-subagent-tab';panel.id='subagent-settings-section';
  const heading=el('h2','','子 Agent'), description=el('p','muted-note','配置子任务并发、递归深度和职责。角色不能扩大父任务的工具权限；保存后重启宿主生效。');
  const form=el('form','subagent-config-form'), status=el('p','settings-notice');status.id='subagent-settings-notice';status.setAttribute('role','status');
  const fields=new Map();
  function field(key,label,type='text') {const wrap=el('label','subagent-field'),input=el(type==='textarea'?'textarea':'input');if(type!=='textarea')input.type=type;input.id=`subagent-${key}`;wrap.append(el('span','',label),input);form.append(wrap);fields.set(key,input);return input;}
  for(const [key,label,min,max]of [['max_parallel','最大并发子任务',1,16],['max_depth','最大委派深度',1,4],['max_children','每个主会话最多子任务',1,128]]){const n=field(key,label,'number');n.min=min;n.max=max;n.required=true;}
  const role=el('select');role.id='subagent-role';role.setAttribute('aria-label','配置角色');form.append(role);
  field('description','角色说明');field('instructions','角色指令','textarea');field('provider_id','供应商 ID（留空继承父任务模型）');field('model_id','模型 ID');
  const inheritWrap=el('label','subagent-inherit-tools'), inheritTools=el('input');inheritTools.type='checkbox';inheritTools.id='subagent-inherit-tools';inheritWrap.append(inheritTools,el('span','','继承父任务工具上限'));form.append(inheritWrap);
  field('tools','工具白名单（逗号分隔；不继承且留空表示禁用全部工具）');
  const save=button('保存配置');save.id='subagent-save';save.type='submit';const refresh=button('重新读取',ctx.guard(loadSettings));refresh.id='subagent-settings-refresh';form.append(save,refresh);panel.append(heading,description,status,form);
  function settingControls(){const disabled=saving||!settings||settings.locked;for(const input of [...fields.values(),role,inheritTools,save])input.disabled=disabled;fields.get('tools').disabled=disabled||inheritTools.checked;refresh.disabled=saving;}
  function captureRole(){if(!draft||!roleId)return;draft.roles[roleId]=roleFromFields({description:fields.get('description').value,instructions:fields.get('instructions').value,provider:fields.get('provider_id').value,model:fields.get('model_id').value,tools:fields.get('tools').value,inheritTools:inheritTools.checked});}
  function renderRole(){const r=draft?.roles[role.value];if(!r)return;roleId=role.value;for(const key of ['description','instructions'])fields.get(key).value=r[key];fields.get('provider_id').value=r.model?.provider_id||'';fields.get('model_id').value=r.model?.model_id||'';fields.get('tools').value=r.tools?.join(', ')||'';inheritTools.checked=r.tools===null;settingControls();}
  function showSettings(value){
    validateSettings(value);settings=value;draft=structuredClone(value.config);for(const key of ['max_parallel','max_depth','max_children'])fields.get(key).value=value.config[key];
    const selected=role.value;role.replaceChildren(...Object.keys(value.config.roles).map(id=>{const n=el('option','',id);n.value=id;return n;}));if(value.config.roles[selected])role.value=selected;
    renderRole();settingControls();
    status.textContent=value.locked?'配置由部署锁定。':value.restart_required?'已保存，重启宿主后生效。':value.enabled?'当前配置已生效。':'子 Agent 当前未启用；可在 Agent 组件中启用后重启。';
  }
  async function loadSettings(){if(!ctx.connection()||saving)return;const generation=epoch,read=++settingsRead;try{const value=await api.request('/api/subagents',null,{maxBytes:128*1024});if(!ctx.signal.aborted&&generation===epoch&&read===settingsRead)showSettings(value);}catch(e){if(generation===epoch&&!ctx.signal.aborted)status.textContent=e.message;}}
  ctx.listen(inheritTools,'change',settingControls);
  ctx.listen(role,'change',()=>{try{captureRole();renderRole();}catch(error){role.value=roleId;status.textContent=error.message;}});
  ctx.listen(form,'submit',ctx.guard(async event=>{
    event.preventDefault();if(saving||!settings||settings.locked)return;
    try{captureRole();}catch(error){status.textContent=error.message;return;}
    const generation=epoch,config=structuredClone(draft);settingsRead++;
    for(const key of ['max_parallel','max_depth','max_children'])config[key]=Number(fields.get(key).value);
    saving=true;settingControls();
    try{const value=await api.request('/api/subagents',{revision:settings.revision,config},{maxBytes:128*1024,maxRequestBytes:64*1024});if(generation===epoch&&!ctx.signal.aborted){saving=false;showSettings(value);}}
    catch(e){if(generation===epoch&&!ctx.signal.aborted)status.textContent=`${e.message} 未自动重试。`;}
    finally{if(generation===epoch&&!ctx.signal.aborted){saving=false;settingControls();}}
  }));
  settingControls();
  ctx.register('settings',{id:'subagent',order:45,value:{tab,panel,activate:()=>settings?undefined:loadSettings()}});
  ctx.on('connection',connection=>{epoch++;settingsRead++;settings=null;draft=null;roleId='';saving=false;settingControls();for(const p of panels.values()){p.detailRead?.abort();p.view?.dispose?.();}panels.clear();clearTimeout(timer);if(connection)api.configure(connection.base,connection.bearer);else api.clear();});
  ctx.on('session',session=>{const id=session?.id||'';if(id!==current){current=id;for(const p of panels.values())controls(p);}pane.renderActions();schedule();});
  ctx.on('assembly',()=>{for(const p of panels.values())controls(p);pane.renderActions();});
  ctx.on('run',state=>{if(state.outcome){const p=panels.get(current);if(p)void read(p);}schedule();});
  ctx.listen(document,'visibilitychange',schedule);
  ctx.own(()=>{epoch++;clearTimeout(timer);api.clear();for(const p of panels.values()){p.detailRead?.abort();p.view?.dispose?.();}panels.clear();});
}
