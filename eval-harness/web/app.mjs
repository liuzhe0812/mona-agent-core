const $ = id => document.getElementById(id);
let token = '', catalog, selected = null, generation = 0, cursor = 0, logLines = [], busy = false, polling = false, offset = 0, history = [], timer, pendingRequest = null;
const active = status => ['queued', 'running', 'cancelling'].includes(status);
const labels = {pending:'待执行',running:'执行中',queued:'排队中',cancelling:'正在停止',passed:'通过',failed:'未通过',error:'执行错误',timeout:'超时',cancelled:'已停止',interrupted:'已中断',inconclusive:'证据不足',skipped:'不支持／跳过',completed:'全部通过',completed_with_issues:'已结束 · 存在未通过项'};
const modes = {selftest:'免费自检：验证测评引擎，不是模型能力成绩。',controlled:'受控模型测试：正在运行真实 Agent，但模型回复来自明确的测试夹具。',live:'真实模型评测：结果来自本次实际调用；小样本通过不代表所有场景可靠。'};
function node(tag, value, cls) { const el = document.createElement(tag); if(value != null) el.textContent=String(value); if(cls) el.className=cls; return el; }
function error(value) { $('error').textContent=value || ''; $('error').hidden=!value; }
async function api(path, options={}) {
  const response=await fetch(path,{...options,headers:{'X-Eval-Token':token,...(options.body?{'Content-Type':'application/json'}:{}),...options.headers},cache:'no-store'});
  const data=await response.json(); if(!response.ok) throw Error(data.error?.message || `HTTP ${response.status}`); return data;
}
function fill(select, values, placeholder) {
  const before=select.value; select.replaceChildren();
  if(placeholder) { const op=node('option',placeholder);op.value='';select.append(op); }
  for(const v of values) {const op=node('option',v.label);op.value=v.id;op.disabled=v.available===false;select.append(op);}
  if([...select.options].some(o=>o.value===before&&!o.disabled))select.value=before;
}
function updateForm() {
  if(!catalog)return;
  const agent=catalog.agents.find(a=>a.id===$('adapter').value), model=catalog.models.find(m=>m.id===$('model').value);
  $('local-approval').hidden=agent?.kind==='selftest';$('paid-approval').hidden=!model?.paid;
  $('start').textContent=agent?.kind==='selftest'?'开始免费自检':model?.paid?'开始模型评测':'开始受控测试';
  $('start').disabled=busy || !agent?.available || !model?.available || (agent?.kind==='selftest'&&model?.paid);
  const unavailable=catalog.agents.filter(a=>!a.available).map(a=>`${a.label}：${a.reason}`).join('；');
  $('profile-note').textContent=agent?.kind==='selftest'?`无真实模型调用。${unavailable}`:`协议：${model?.protocol}。只启动独立测试进程，不连接已在使用的 Agent 服务。`;
  const tasks=catalog.tasks.filter(t=>$('suite').value==='all'||t.suite===$('suite').value);
  $('task-list').replaceChildren(...tasks.map(t=>node('span',t.title,'task-chip')));
  const count=tasks.length*Number($('repeats').value||1);$('estimate').textContent=`共 ${count} 次任务 · 最多 ${count*Number($('max_model_calls').value||16)} 次模型调用`;
}
async function loadHistory(append=false) {
  const page=await api(`/api/runs?offset=${append?offset:0}&limit=50`);
  history=append?[...history,...page.runs]:page.runs;offset=history.length;
  $('more').hidden=offset>=page.total;
  const focused=document.activeElement?.dataset?.run;
  $('history').replaceChildren(...history.map(run=>{
    const button=node('button',null,'history-row');button.type='button';button.dataset.run=run.id;button.setAttribute('aria-current',String(selected===run.id));
    button.append(node('span',run.config.label||`${run.config.adapter} · ${run.config.suite}`),node('span',`${labels[run.status]||run.status} · ${new Date(run.created*1000).toLocaleString()}`,'muted'));
    button.addEventListener('click',()=>selectRun(run.id));return button;
  }));
  if(focused)$('history').querySelector(`[data-run="${CSS.escape(focused)}"]`)?.focus({preventScroll:true});
  if(!history.length)$('history').append(node('p','尚无记录。可以先运行免费自检。','muted'));
  const options=history.map(r=>({id:r.id,label:`${r.config.label||r.config.adapter} · ${new Date(r.created*1000).toLocaleString()} · ${r.id.slice(-6)}`}));
  if(document.activeElement!==$('left'))fill($('left'),options,'选择基线');
  if(document.activeElement!==$('right'))fill($('right'),options,'选择对照');
}
function metric(label,value) {const box=node('div');box.append(node('div',label,'muted'),node('div',value,'metric-value'));return box;}
function display(value,digits=0) { return value==null?'未知':Number(value).toLocaleString(undefined,{maximumFractionDigits:digits}); }
function renderRun(run) {
  $('run-section').hidden=false;$('configuration').hidden=true;
  $('run-title').textContent=run.config.label||`${run.config.adapter} · ${run.config.suite}`;
  $('run-meta').textContent=`${labels[run.status]||run.status} · ${run.id}`;
  $('run-mode').textContent=modes[run.mode]||run.mode;
  $('cancel').hidden=!active(run.status);$('cancel').disabled=run.status==='cancelling';$('delete').disabled=active(run.status);
  const s=run.summary;$('progress').max=Math.max(s.total,1);$('progress').value=s.finished;
  $('metrics').replaceChildren(metric('通过 / 全部任务',`${s.counts.passed||0} / ${s.total}`),metric('中位耗时（ms）',display(s.median_ms,1)),metric('模型调用',display(s.model_calls)),metric('报告 token',display(s.tokens)));
  $('warnings').textContent=`用量完整覆盖：${s.usage_coverage==null?'未知':Math.round(s.usage_coverage*100)+'%'}。内存峰值、实际磁盘 I/O、金额未测量时不显示为 0。`;
  const opened=new Set([...$('trials').querySelectorAll('details[open]')].map(x=>x.dataset.id));
  const focused=document.activeElement?.dataset?.trial;
  $('trials').replaceChildren(...run.trials.map(t=>{
    const d=node('details',null,'trial');d.dataset.id=t.id;d.open=opened.has(t.id);
    const title=node('summary');title.dataset.trial=t.id;title.append(node('span',`${t.title} · 第 ${t.repeat+1} 次`),node('span',labels[t.status]||t.status,`status ${t.status}`));d.append(title);
    if(t.error)d.append(node('p',t.error,'notice'));
    const checks=node('div',null,'checks');for(const c of t.checks)checks.append(node('div',`${labels[c.status]||c.status} · ${c.type} · ${c.reason}`));d.append(checks);
    for(const [i,turn] of (t.turns||[]).entries()){d.append(node('p',`第 ${i+1} 轮输出${turn.output_truncated?'（预览已截短，完整内容见证据）':''}`,'muted'),node('pre',turn.output));}
    d.append(node('p',`耗时 ${display(t.metrics.wall_ms,1)} ms · 工作区 ${display(t.metrics.workspace?.bytes)} 字节 · 状态目录 ${display(t.metrics.state?.bytes)} 字节`,'muted'));
    if(t.evidence_sha256){const b=node('button','下载本题证据');b.type='button';b.addEventListener('click',()=>download(`/api/runs/${run.id}/trials/${t.id}/evidence`,`${run.id}-${t.id}.json`).catch(e=>error(e.message)));d.append(b);}
    return d;
  }));
  if(focused)$('trials').querySelector(`[data-trial="${CSS.escape(focused)}"]`)?.focus({preventScroll:true});
  $('provenance').textContent=JSON.stringify({config:run.config,provenance:run.provenance,warnings:run.warnings},null,2);
}
async function refreshSelected() {
  if(!selected)return;const id=selected,epoch=generation;
  const run=await api(`/api/runs/${id}`);if(epoch!==generation)return;renderRun(run);
  const events=await api(`/api/runs/${id}/events?after=${cursor}`);if(epoch!==generation)return;
  if(events.truncated)$('log-status').textContent='仅显示有界日志尾部；任务结论以报告与证据为准';
  for(const event of events.events)logLines.push(`${new Date(event.time*1000).toLocaleTimeString()} ${event.trial||''} [${event.type}] ${event.message}`);
  logLines=logLines.slice(-400);cursor=events.next;
  const pre=$('logs'),follow=pre.scrollTop+pre.clientHeight>=pre.scrollHeight-30;pre.textContent=logLines.join('\n');if(follow)pre.scrollTop=pre.scrollHeight;
}
async function selectRun(id) {generation++;selected=id;cursor=0;logLines=[];$('logs').textContent='';$('log-status').textContent='';error('');try{await refreshSelected();await loadHistory();}catch(e){error(e.message);}}
async function download(path,name) {const r=await fetch(path,{headers:{'X-Eval-Token':token}});if(!r.ok){const x=await r.json();throw Error(x.error?.message||'导出失败');}const url=URL.createObjectURL(await r.blob()),a=node('a');a.href=url;a.download=name;a.click();setTimeout(()=>URL.revokeObjectURL(url),1000);}
$('run-form').addEventListener('submit',async event=>{
  event.preventDefault();if(busy)return;error('');busy=true;updateForm();
  const request={adapter:$('adapter').value,model:$('model').value,suite:$('suite').value,label:$('label').value,allow_paid:$('allow_paid').checked,allow_local_execution:$('allow_local_execution').checked};
  for(const k of ['repeats','concurrency','timeout_s','max_model_calls','max_tokens'])request[k]=Number($(k).value);
  const signature=JSON.stringify(request);
  if(!pendingRequest||pendingRequest.signature!==signature)pendingRequest={signature,id:crypto.randomUUID()};
  request.request_id=pendingRequest.id;
  try{const run=await api('/api/runs',{method:'POST',body:JSON.stringify(request)});pendingRequest=null;await selectRun(run.id);}catch(e){error(e.message);}finally{busy=false;updateForm();}
});
$('new-run').addEventListener('click',()=>{generation++;selected=null;$('configuration').hidden=false;$('run-section').hidden=true;error('');});
$('cancel').addEventListener('click',async()=>{const id=selected;try{await api(`/api/runs/${id}/cancel`,{method:'POST',body:'{}'});await refreshSelected();}catch(e){error(e.message);}});
$('delete').addEventListener('click',async()=>{if(!selected||!confirm('删除这次测评的报告、证据和独立测试数据？不会删除被测项目源码。'))return;const id=selected;try{await api(`/api/runs/${id}`,{method:'DELETE'});if(selected===id)$('new-run').click();await loadHistory();}catch(e){error(e.message);}});
$('export').addEventListener('click',()=>download(`/api/runs/${selected}/report`,`${selected}.json`).catch(e=>error(e.message)));
$('compare').addEventListener('click',async()=>{try{const left=$('left').value,right=$('right').value;if(!left||!right||left===right)throw Error('请选择两次不同的测评。');const result=await api(`/api/compare?left=${encodeURIComponent(left)}&right=${encodeURIComponent(right)}`);$('comparison').replaceChildren();if(result.warnings.length)$('comparison').append(node('p',result.warnings.join('；'),'notice'));const wrap=node('div',null,'table-wrap'),table=node('table'),thead=node('thead'),head=node('tr');for(const x of ['指标','基线','对照','差值'])head.append(node('th',x));thead.append(head);table.append(thead);const tbody=node('tbody');for(const m of result.metrics){const row=node('tr');for(const x of [m.name,display(m.left,3),display(m.right,3),display(m.delta,3)])row.append(node('td',x));tbody.append(row);}table.append(tbody);wrap.append(table);$('comparison').append(wrap,node('p',result.note,'muted'));}catch(e){error(e.message);}});
$('theme').addEventListener('click',()=>{document.documentElement.dataset.theme=document.documentElement.dataset.theme==='dark'?'light':'dark';});
$('more').addEventListener('click',()=>loadHistory(true).catch(e=>error(e.message)));
for(const k of ['adapter','model','suite','repeats','max_model_calls'])$(k).addEventListener('change',()=>{if(k==='adapter'||k==='model'){$('allow_paid').checked=false;$('allow_local_execution').checked=false;}updateForm();});
async function connect(){const r=await fetch('/api/bootstrap',{cache:'no-store'});if(!r.ok)throw Error('无法连接本地测评服务');token=(await r.json()).token;catalog=await api('/api/catalog');fill($('adapter'),catalog.agents);fill($('model'),catalog.models);fill($('suite'),[{id:'all',label:'全部任务'},...catalog.suites.map(s=>({id:s.id,label:`${s.id} · ${s.count} 题`}))]);updateForm();await loadHistory();$('connection').textContent='本地已连接';}
$('refresh').addEventListener('click',()=>connect().then(refreshSelected).catch(e=>error(e.message)));
try{await connect();timer=setInterval(async()=>{if(polling)return;polling=true;try{await refreshSelected();if(offset<=50)await loadHistory();$('connection').textContent='本地已连接';}catch(e){$('connection').textContent='连接中断';error(e.message);}finally{polling=false;}},1200);}catch(e){error(e.message);$('connection').textContent='未连接';}
window.addEventListener('pagehide',()=>clearInterval(timer),{once:true});
