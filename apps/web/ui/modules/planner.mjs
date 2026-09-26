import { ProductClient } from '../client.mjs';
import { element as el } from '../dom.mjs';
import { planContent, validatePlan } from '../plan-view.mjs';
export const version = 1;
const sessionId = value => { if (typeof value !== 'string' || !/^[A-Za-z0-9_-]{1,64}$/.test(value)) throw new Error('计划所属会话无效。'); return value; };
export function validateView(value, id) {
  if (value?.session_id !== id || !Number.isSafeInteger(value.revision) || value.revision < 0 || typeof value.enabled !== 'boolean'
      || !['idle','running','completed','failed','cancelled','timed_out','limited','interrupted'].includes(value.status)) throw new Error('计划响应身份或状态无效。');
  validatePlan(value.plan); return value;
}
const SVG_NS='http://www.w3.org/2000/svg';
function planGlyph(className='') { const node=document.createElementNS(SVG_NS,'svg');node.setAttribute('class',className);node.setAttribute('viewBox','0 0 16 16');node.setAttribute('fill','none');node.setAttribute('aria-hidden','true');
  const stroke=d=>{const p=document.createElementNS(SVG_NS,'path');p.setAttribute('d',d);p.setAttribute('stroke','currentColor');p.setAttribute('stroke-width','1');node.append(p);};
  const fill=d=>{const p=document.createElementNS(SVG_NS,'path');p.setAttribute('d',d);p.setAttribute('fill','currentColor');node.append(p);};
  stroke('M4.9375 5.90295H11.0625');stroke('M4.9375 9.02991H8.27841');fill('M12.5 1.32617C13.3039 1.32617 14 1.95171 14 2.77637V7.61328L13 8.68164V2.77637C13 2.55186 12.8007 2.32617 12.5 2.32617H3.5C3.1993 2.32617 3 2.55186 3 2.77637V13.2246C3.00044 13.4489 3.19963 13.6738 3.5 13.6738H8.32812L7.39258 14.6738H3.5C2.69637 14.6738 2.00042 14.0489 2 13.2246V2.77637C2 1.95171 2.69613 1.32617 3.5 1.32617H12.5Z');fill('M8.97212 14.3693C9.17511 14.5723 9.37811 14.7753 9.5811 14.9783L14.9221 9.99803L13.9524 9.02834L8.97212 14.3693Z');fill('M11.6323 13.7841V14.5504L14.5657 14.6672V13.6672L11.6323 13.7841Z');return node; }
function closeGlyph(className='') { const node=document.createElementNS(SVG_NS,'svg');node.setAttribute('class',className);node.setAttribute('viewBox','0 0 16 16');node.setAttribute('fill','none');node.setAttribute('aria-hidden','true');const circle=document.createElementNS(SVG_NS,'circle');circle.setAttribute('cx','8');circle.setAttribute('cy','8');circle.setAttribute('r','6.25');circle.setAttribute('fill','currentColor');const path=document.createElementNS(SVG_NS,'path');path.setAttribute('d','M5.6 5.6l4.8 4.8m0-4.8-4.8 4.8');path.setAttribute('stroke','white');path.setAttribute('stroke-width','1.2');path.setAttribute('stroke-linecap','round');node.append(circle,path);return node; }

export function mount(ctx) {
  const api = new ProductClient(), pane = ctx.shell.pane(ctx.id), panels = new Map();
  let current = '', view = null, generation = 0, readSequence = 0, controller = null, pendingRead = null, mutation = false, timer = null, busy = false, noticeRevision = 0;
  const chip=el('button','planner-mode-chip');chip.id='planner-mode-chip';chip.type='button';chip.setAttribute('aria-label','计划模式已开启，点击退出');chip.hidden=true;
  const glyph=el('span','planner-mode-glyph');glyph.append(planGlyph('planner-mode-plan'),closeGlyph('planner-mode-close'));chip.append(glyph,el('span','','计划'));
  const dock = el('div','planner-dock'), summary = el('span','planner-dock-summary'), notice = el('span','planner-notice'); notice.setAttribute('role','status');
  const open = el('button','text-button','查看计划'); open.type='button'; dock.append(summary,open,notice);
  ctx.register('composer.mode',{id:'planner',order:20,value:chip});
  ctx.register('composer.dock',{id:'planner',order:10,value:dock});
  const path = id => `/api/sessions/${sessionId(id)}/plan`;
  const enabled = () => ctx.capability('planner')?.active === true;
  ctx.register('composer.commands', { id: 'plan', order: 10, value: {
    menu: { section: 'add', label: '计划', description: '进入或退出计划模式', icon: () => planGlyph(),
      visible: enabled, disabled: () => mutation || busy || !ctx.connection() || Boolean(current && !view) || view?.status === 'running',
      select: () => change(view?.plan.mode === 'plan_only' ? 'normal' : 'plan'),
    },
    run: async raw => {
      const arg = String(raw ?? '').trim().toLowerCase();
      if (!arg || arg === 'on') return change('plan');
      if (arg === 'off') return change('normal');
      throw new Error('计划命令只支持 /plan 或 /plan off。');
    },
  } });
  const button = (label, action) => { const n=el('button','text-button',label); n.type='button'; n.addEventListener('click',ctx.guard(action)); return n; };
  function renderPanel(id, data) {
    const panel = panels.get(id); if (!panel) return;
    const signature=JSON.stringify([data.plan,data.revision,data.enabled,data.status,busy,mutation,current]);
    if (panel.signature === signature) return; panel.signature=signature;
    const planSignature=JSON.stringify(data.plan);
    if (panel.planSignature !== planSignature) {
      const content=planContent(data.plan,ctx.shell.presentation({id}));
      if (panel.content) { panel.content.dispose?.(); panel.content.replaceWith(content); }
      else panel.root.replaceChildren(content);
      panel.content=content; panel.planSignature=planSignature;
    }
    panel.actions?.remove();
    const actions=el('div','planner-panel-actions'); panel.actions=actions;
    const refresh=button('刷新',()=>read(id)); refresh.disabled=mutation; actions.append(refresh);
    const actionable = data.enabled && data.status !== 'running' && ctx.shell.session()?.id === id && !busy && !mutation;
    if (data.plan.mode === 'plan_only' && data.plan.proposal) {
      const refine=button('继续修改',()=>change('refine',data));
      const resume=button('按此计划执行',async()=>{
        if (ctx.shell.draft?.().trim()) { noticeRevision++; notice.textContent='输入框已有草稿，请先发送或保留草稿后继续。'; render(); ctx.shell.focusPrompt(); return; }
        if (await change('resume',data)) await ctx.shell.submitText('按此计划继续执行。',id);
      });
      refine.disabled=resume.disabled=!actionable; actions.append(refine,resume);
    }
    if (!data.enabled) actions.append(el('span','muted-note','Planner 当前未启用，历史仅供查看。'));
    panel.root.append(actions);
  }
  function render() {
    const state=view?.plan, planning=state?.mode==='plan_only'; chip.hidden=!enabled()||!planning;
    chip.disabled=mutation||busy||(current&&!view)||view?.status==='running'||!enabled();
    chip.dataset.tooltip='计划模式：只调研与形成方案；点击退出计划模式';
    summary.textContent=state?.steps.length ? `${state.steps.filter(s=>s.status==='completed').length}/${state.steps.length} 步 · ${planning?(state.proposal?'方案待继续':'正在规划'):'执行计划'}` : '';
    open.hidden=!state?.steps.length; dock.hidden=!state?.steps.length&&!notice.textContent;
    if (view) renderPanel(view.session_id,view);
    pane.renderActions();
    ctx.refreshCommands();
  }
  async function read(id = current) {
    if (!id || !ctx.connection()) return;
    // Session notifications and token bursts share one read instead of cancelling it repeatedly.
    if (pendingRead?.id === id && pendingRead.generation === generation) return pendingRead.promise;
    const ownGeneration=generation, sequence=++readSequence, ownNoticeRevision=noticeRevision; controller?.abort(); controller=new AbortController();
    const job={id,generation:ownGeneration}; pendingRead=job;
    job.promise=(async()=>{
      try {
        const data=validateView(await api.request(path(id),null,{signal:controller.signal,maxBytes:32*1024}),id);
        if (ctx.signal.aborted || ownGeneration!==generation || sequence!==readSequence) return;
        if(id===current){if(view && data.revision<view.revision)return;view=data;if(ownNoticeRevision===noticeRevision)notice.textContent='';render();}else renderPanel(id,data);
      } catch(error) { if(error.name!=='AbortError'&&ownGeneration===generation&&!ctx.signal.aborted){noticeRevision++;notice.textContent=error.message;render();} }
      finally { if(pendingRead===job)pendingRead=null; }
    })();
    return job.promise;
  }
  async function change(action, expected = null) {
    if(mutation || busy || !enabled() || ctx.signal.aborted) return false;
    const initialId=ctx.shell.session()?.id || '';
    if(expected && (expected.session_id!==initialId || expected.session_id!==current)) return false;
    mutation=true; render();
    let unlock=()=>{}, ownGeneration=generation, id=initialId;
    try {
      // Admission checks before taking our own composer lock; no await permits a second click.
      const admission=ctx.shell.ensureSession(); unlock=ctx.lockComposer();
      const selected=await admission;
      if(!selected || ctx.signal.aborted || (initialId && selected.id!==initialId) || ctx.shell.session()?.id!==selected.id)return false;
      id=selected.id;
      if(current!==id){current=id;generation++;view=null;}
      ownGeneration=generation;
      if(!view || view.session_id!==id)await read(id);
      if(ctx.signal.aborted || ownGeneration!==generation || ctx.shell.session()?.id!==id || !view || view.status==='running')return false;
      // A pane action applies to the exact displayed revision, never whichever plan is current later.
      const old=expected || view;
      const data=validateView(await api.request(path(id),{revision:old.revision,plan_revision:old.plan.revision,action},{maxBytes:32*1024}),id);
      if(ctx.signal.aborted || ownGeneration!==generation || current!==id || ctx.shell.session()?.id!==id)return false;
      view=data;await ctx.shell.refreshSessionHeader(id);
      if(ctx.signal.aborted || ownGeneration!==generation || ctx.shell.session()?.id!==id)return false;
      noticeRevision++;notice.textContent=action==='refine'?'已进入计划模式，输入修改要求后发送。':action==='resume'?'执行模式已确认。':data.plan.mode==='plan_only'?'计划模式：仅调研和形成方案，不执行业务修改。':'执行模式：按现有工具授权直接推进。';
      ctx.shell.focusPrompt();return true;
    } catch(error) {
      if(ownGeneration===generation&&!ctx.signal.aborted){
        await read(id);
        if(ownGeneration===generation&&!ctx.signal.aborted){noticeRevision++;notice.textContent=error.status===409?'计划或会话已更新，请查看最新版本后再操作。':error.message;}
      }
      return false;
    } finally {mutation=false;unlock();if(!ctx.signal.aborted)render();}
  }
  async function openPanel() {
    if(!current)return;
    const id=current,scope=`session:${id}`;
    let panel=panels.get(id);
    if(!panel){panel={root:el('section','planner-panel'),signature:null};panels.set(id,panel);panel.root.append(el('p','muted-note','正在读取计划…'));}
    pane.openTab({id:'plan',title:'计划',kind:'plan',scope,node:panel.root,onActivate:()=>read(id),
      onClose:()=>{panel.content?.dispose?.();panels.delete(id);}});
    if(view?.session_id===id)renderPanel(id,view); await read(id);
  }
  ctx.register('right.pane',{id:'plan',order:10,value:{label:'计划',icon:'plan',available:()=>Boolean(current && (enabled()||view?.plan.steps.length)),run:openPanel}});
  ctx.listen(open,'click',ctx.guard(openPanel));
  ctx.listen(chip,'click',ctx.guard(async()=>{await change('normal');}));
  ctx.on('connection',connection=>{generation++;view=null;controller?.abort();clearTimeout(timer);noticeRevision++;notice.textContent='';if(connection)api.configure(connection.base,connection.bearer);else api.clear();render();});
  ctx.on('session',session=>{const id=session?.id||'';if(id!==current){generation++;controller?.abort();current=id;view=null;noticeRevision++;notice.textContent='';}render();if(id && !mutation && view?.revision!==session.revision)void read(id);});
  ctx.on('busy',value=>{if(busy!==value){busy=value;render();}});
  ctx.on('assembly',()=>{render();void read();});
  ctx.on('run',state=>{
    if(mutation)return;
    if(state.outcome){clearTimeout(timer);timer=null;}
    if(timer==null)timer=setTimeout(()=>{timer=null;void read();},state.outcome?0:350);
  });
  ctx.own(()=>{generation++;controller?.abort();clearTimeout(timer);api.clear();for(const panel of panels.values())panel.content?.dispose?.();panels.clear();});
  render();
}
