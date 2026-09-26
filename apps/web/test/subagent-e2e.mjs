// Formal Web + isolated Rust host + real tools. The model is deterministic and local only.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { access, mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startUiServer } from '../../../scripts/dev-web.mjs';
const root=fileURLToPath(new URL('../../../',import.meta.url));
const report=join(root,'target/subagent-validation/browser');await mkdir(report,{recursive:true});
const testRoot=process.platform==='win32'?join(process.env.LOCALAPPDATA,'mona-agent-core','subagent-tests'):report;
await mkdir(testRoot,{recursive:true});const scratch=await mkdtemp(join(testRoot,'isolated-'));
const state=join(scratch,'state'),home=join(scratch,'home'),temp=join(scratch,'temp');
for(const p of [state,home,temp])await mkdir(p,{recursive:true});
const token='subagent-test-only-credentials-at-least-32-characters';
const checks=[],errors=[],calls=[],toolResults=[];let host,ui,browser,socket,hostLog='',seq=0,gate=null,spawnSeen=false;
const pending=new Map(),blocked=new Set(),sleep=ms=>new Promise(r=>setTimeout(r,ms));
function check(name,value){assert.ok(value,name);checks.push(name);console.log('PASS '+name);}
async function wait(fn,label,ms=30000){const end=Date.now()+ms;while(Date.now()<end){if(await fn())return;await sleep(60);}throw Error('Timeout: '+label);}
async function port(){const s=createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const p=s.address().port;await new Promise(r=>s.close(r));return p;}
async function stop(child){if(!child||child.exitCode!=null)return;const done=once(child,'exit');child.kill('SIGKILL');await Promise.race([done,sleep(5000)]);}
const hostPort=await port(),endpoint=`http://127.0.0.1:${hostPort}`;
const fixture=createServer(async(req,res)=>{
 try{
  let raw='';for await(const part of req){raw+=part;if(Buffer.byteLength(raw)>4*1024*1024)throw Error('fixture request capacity');}
  const body=JSON.parse(raw),number=calls.length+1;calls.push(body);
  const anchor=body.messages.findLastIndex(m=>m.role==='user'&&typeof m.content==='string'&&!m.content.startsWith('[Host context source:')&&!m.content.startsWith('[Earlier conversation summary'));
  const prompt=body.messages[anchor]?.content||'',tail=body.messages.slice(anchor+1),observations=new Map();
  for(const m of tail.filter(m=>m.role==='tool')){
   const result=JSON.parse(m.content);let value;try{value=JSON.parse(result.content);}catch{value=result.content;}
   observations.set(m.tool_call_id,{...result,value});toolResults.push({prompt,id:m.tool_call_id,...result,value});
  }
  if(prompt==='BLOCK_CHILD'){blocked.add(res);res.on('close',()=>blocked.delete(res));return;}
  if(prompt==='DELEGATE_ROOT'&&!observations.size&&gate){spawnSeen=true;await gate.promise;}
  res.writeHead(200,{'content-type':'text/event-stream','cache-control':'no-store'});
  const frame=v=>res.write(`data: ${JSON.stringify(v)}\n\n`);
  const text=content=>frame({choices:[{index:0,delta:{content},finish_reason:'stop'}]});
  const tools=items=>frame({choices:[{index:0,delta:{tool_calls:items.map(([id,name,args],index)=>({index,id,type:'function',function:{name,arguments:JSON.stringify(args)}}))},finish_reason:'tool_calls'}]});
  const get=id=>{const out=observations.get(id);if(out)assert.equal(out.status,'success',`${prompt} ${id}: ${JSON.stringify(out)}`);return out?.value;};
  const finalWait=ids=>{
   const last=[...observations.entries()].filter(([id])=>id.startsWith('wait-')).at(-1)?.[1];
   if(!last||last.value.timed_out){tools([[`wait-${number}`,'wait_agent',{ids,timeout_ms:2000}]]);return false;}
   assert.equal(last.status,'success',JSON.stringify(last));return true;
  };
  if(prompt==='DELEGATE_ROOT'){
   const a=get('spawn-worker'),b=get('spawn-explorer');
   if(!a)tools([['spawn-worker','spawn_agent',{task:'WRITE_CHILD',role:'worker',context:'fork'}],['spawn-explorer','spawn_agent',{task:'INSPECT_CHILD',role:'explorer',context:'independent'}]]);
   else if(!b)throw Error('parallel spawn result missing');
   else if(!get('mail')){
    if(finalWait([a.id,b.id]))tools([['mail','send_message',{id:a.id,text:'ADDITIONAL_FACT'}]]);
   }else if(!get('followup'))tools([['followup','followup_agent',{id:a.id,text:'FOLLOWUP_CHILD'}]]);
   else{
    const afterFollow=tail.findLastIndex(m=>m.role==='tool'&&m.tool_call_id==='followup');
    const last=tail.slice(afterFollow+1).filter(m=>m.role==='tool').at(-1);
    if(!last||JSON.parse(JSON.parse(last.content).content).timed_out)tools([[`wait-${number}`,'wait_agent',{ids:[a.id],timeout_ms:2000}]]);
    else text('DELEGATE_DONE');
   }
  }else if(prompt==='WRITE_CHILD'){
   assert.equal(body.model,'worker-model');assert.ok(body.messages.some(m=>String(m.content).includes('PARENT_ONLY_FACT')));
   if(!get('write-child'))tools([['write-child','write',{path:'child-result.txt',content:'CHILD_FILE_OK'}]]);
   else text('CHILD_OK');
  }else if(prompt==='INSPECT_CHILD'){
   assert.equal(body.model,'parent-model','child inherits the pinned route rather than new global default');
   assert.ok(!body.messages.some(m=>String(m.content).includes('PARENT_ONLY_FACT')));
   assert.ok(!body.tools.some(t=>['write','edit','shell','spawn_agent'].includes(t.function.name)));
   text('INDEPENDENT_INSPECTION_OK');
  }else if(prompt==='FOLLOWUP_CHILD'){
   assert.equal(body.model,'worker-model');assert.ok(body.messages.some(m=>String(m.content).includes('ADDITIONAL_FACT')));
   assert.ok(body.messages.some(m=>m.role==='tool'&&String(m.content).includes('CHILD_FILE_OK'))||body.messages.some(m=>String(m.content).includes('CHILD_OK')));
   text('FOLLOWUP_OK');
  }else if(prompt.startsWith('CONTINUE_ROOT ')){
   const id=prompt.slice('CONTINUE_ROOT '.length);
   if(!get('continue-child'))tools([['continue-child','followup_agent',{id,text:'FOLLOWUP_CHILD'}]]);
   else if(finalWait([id]))text('CONTINUED_AFTER_RESTART');
  }else if(['STOP_CHILD_ROOT','PARENT_CANCEL_ROOT'].includes(prompt)){
   const spawnId=`spawn-block-${prompt}`,a=get(spawnId);
   if(!a)tools([[spawnId,'spawn_agent',{task:'BLOCK_CHILD'}]]);
   else if(finalWait([a.id]))text('STOP_COLLECTED');
  }else if(prompt==='PLAN_BOUNDARY'){
   assert.ok(!body.tools.some(t=>['spawn_agent','followup_agent','write','edit','shell'].includes(t.function.name)));text('PLAN_REMAINS_READONLY');
  }else if(prompt==='READONLY_ROOT'){
   const a=get('spawn-readonly');
   if(!a)tools([['spawn-readonly','spawn_agent',{task:'DENIED_WRITE_CHILD',context:'independent'}]]);
   else if(finalWait([a.id]))text('READONLY_DELEGATION_DONE');
  }else if(prompt==='DENIED_WRITE_CHILD'){
   assert.ok(body.messages.some(m=>String(m.content).includes('Current sandbox mode: read-only.')));
   if(!observations.has('denied-write'))tools([['denied-write','write',{path:'must-not-exist.txt',content:'DENIED'}]]);
   else if(!observations.has('denied-shell')){
    assert.notEqual(observations.get('denied-write').status,'success');
    tools([['denied-shell','shell',{command:process.platform==='win32'?"Set-Content -LiteralPath 'must-not-exist-shell.txt' -Value 'DENIED' -ErrorAction Stop":"printf DENIED > must-not-exist-shell.txt"}]]);
   }else{assert.notEqual(observations.get('denied-shell').status,'success');text('WRITE_DENIED');}
  }else text('ACK '+prompt);
  frame({choices:[],usage:{prompt_tokens:10,completion_tokens:5}});res.end('data: [DONE]\n\n');
 }catch(error){errors.push('model: '+String(error));res.destroy();}
});
await new Promise(r=>fixture.listen(0,'127.0.0.1',r));
async function api(path,body,expected=200,auth=true,method=body==null?'GET':'POST'){
 const r=await fetch(endpoint+path,{method,headers:{...(auth?{Authorization:`Bearer ${token}`} :{}),'Content-Type':'application/json'},body:body==null?undefined:JSON.stringify(body),signal:AbortSignal.timeout(30000)});
 const raw=await r.text();assert.equal(r.status,expected,`${path}: ${raw}`);return raw?JSON.parse(raw):null;
}
async function start(origin,extra={}){
 const env=Object.fromEntries(Object.entries(process.env).filter(([k])=>!k.startsWith('AGENT_')&&!k.startsWith('MONA_DEV_')));
 Object.assign(env,{MONA_DEV_STATE_DIR:state,HOME:home,USERPROFILE:home,TEMP:temp,TMP:temp,TMPDIR:temp,AGENT_SERVER_ADDR:`127.0.0.1:${hostPort}`,AGENT_SERVER_TOKEN:token,AGENT_UI_ORIGIN:origin,
 AGENT_MODEL_NAME:'parent-model',AGENT_MODEL_ENDPOINT:`http://127.0.0.1:${fixture.address().port}/v1/chat/completions`,AGENT_MODEL_SETTINGS_PATH:join(state,'models.enc'),AGENT_MODEL_STORE_KEY:'test-only-model-key-at-least-32-characters',AGENT_ALLOW_HTTP_LOOPBACK:'1',AGENT_CAPABILITY_STATE_PATH:join(state,'capabilities.json'),AGENT_INSTRUCTIONS:'0',AGENT_COMPACTION:'0',AGENT_SPILL:'0',...extra});
 hostLog='';host=spawn(resolve(process.env.MONA_TEST_SERVER||join(root,'target/debug',process.platform==='win32'?'server.exe':'server')),[],{cwd:scratch,env,stdio:['ignore','pipe','pipe']});
 host.stderr.on('data',c=>hostLog=(hostLog+c).slice(-20000));host.on('error',e=>errors.push(String(e)));
 await wait(async()=>{if(host.exitCode!=null)throw Error(hostLog);try{return(await api('/v1/info')).protocol_version===2;}catch{return false;}},'host ready');
}
function cdp(method,params={}){return new Promise((resolve,reject)=>{const id=++seq,timer=setTimeout(()=>{pending.delete(id);reject(Error('CDP '+method));},20000);pending.set(id,{resolve,reject,timer});socket.send(JSON.stringify({id,method,params}));});}
async function evaluate(expression){const r=await cdp('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(r.exceptionDetails)throw Error(r.exceptionDetails.exception?.description||'browser exception');return r.result.value;}
const pageWait=(exp,label)=>wait(()=>evaluate(exp),label);
async function click(selector){const p=await evaluate(`(()=>{const n=document.querySelector(${JSON.stringify(selector)});if(!n||n.disabled)throw Error('missing or disabled '+${JSON.stringify(selector)});n.scrollIntoView({block:'nearest'});const r=n.getBoundingClientRect();if(!r.width||!r.height)throw Error('hidden element '+${JSON.stringify(selector)});return{x:r.x+r.width/2,y:r.y+r.height/2};})()`);await cdp('Input.dispatchMouseEvent',{type:'mousePressed',button:'left',clickCount:1,...p});await cdp('Input.dispatchMouseEvent',{type:'mouseReleased',button:'left',clickCount:1,...p});}
async function fill(selector,value){await evaluate(`(()=>{const n=document.querySelector(${JSON.stringify(selector)});n.value=${JSON.stringify(value)};n.dispatchEvent(new Event('input',{bubbles:true}));n.dispatchEvent(new Event('change',{bubbles:true}));})()`);}
async function shot(name){const r=await cdp('Page.captureScreenshot',{format:'png',captureBeyondViewport:false});await writeFile(join(report,name+'.png'),Buffer.from(r.data,'base64'));}
const selectedId=()=>evaluate("location.hash.slice('#session='.length)");
async function begin(prompt){await pageWait("!document.querySelector('#prompt').disabled&&!document.querySelector('#sessions-refresh').disabled",'composer unlocked');await fill('#prompt',prompt);await pageWait("!document.querySelector('#send').disabled",'send ready');await click('#send');await pageWait("/^#session=[A-Za-z0-9_-]+$/.test(location.hash)",'session');return selectedId();}
async function completed(id){await wait(async()=>{const d=await api('/api/sessions/'+id);if(['failed','limited','interrupted'].includes(d.session.status))throw Error('parent failed '+JSON.stringify(d));return d.session.status==='completed';},'parent complete');await pageWait("document.querySelector('#cancel').hidden&&!document.querySelector('#prompt').disabled&&!document.querySelector('#sessions-refresh').disabled",'settled controls');}
async function openChildren(){await pageWait("(async()=>{const {uiRegistry}=await import('/apps/web/ui/registry.mjs');return uiRegistry.resolve('right.pane','subagents')?.available();})()",'child panel available for selected root');await evaluate("(async()=>{const {uiRegistry}=await import('/apps/web/ui/registry.mjs');return uiRegistry.resolve('right.pane','subagents').run();})()");}
try{
 ui=await startUiServer({host:'127.0.0.1',port:0,endpoint,token});const origin=`http://127.0.0.1:${ui.address().port}`;await start(origin);
 await api('/api/subagents',null,401,false);
 const m=await api('/api/model-settings');await api('/api/model-settings/providers',{revision:m.revision,id:'environment',name:'Controlled provider',protocol:'chat_completions',api_base:`http://127.0.0.1:${fixture.address().port}/v1/chat/completions`,models:['parent-model','worker-model','later-model'].map(id=>({id,enabled:true})),clear_key:false});
 const setting=await api('/api/subagents');setting.config.roles.worker.model={provider_id:'environment',model_id:'worker-model'};
 await api('/api/subagents',{revision:setting.revision,config:setting.config});check('role configuration saves without silently replacing active host settings',(await api('/api/subagents')).restart_required===true);
 await stop(host);await start(origin);check('role configuration becomes active after restart',(await api('/api/subagents')).active.roles.worker.model.model_id==='worker-model');
 const debug=await port();browser=spawn(process.env.BROWSER_BIN||(process.platform==='win32'?'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe':'chromium'),['--headless=new','--disable-gpu','--no-first-run','--no-default-browser-check',`--remote-debugging-port=${debug}`,`--user-data-dir=${join(scratch,'browser')}`,'about:blank'],{stdio:'ignore'});
 let page;await wait(async()=>{try{page=(await(await fetch(`http://127.0.0.1:${debug}/json/list`)).json()).find(p=>p.type==='page');return!!page;}catch{return false;}},'browser ready');
 socket=new WebSocket(page.webSocketDebuggerUrl);await once(socket,'open');socket.addEventListener('message',event=>{const m=JSON.parse(event.data),p=pending.get(m.id);if(p){pending.delete(m.id);clearTimeout(p.timer);m.error?p.reject(Error(JSON.stringify(m.error))):p.resolve(m.result);}if(m.method==='Runtime.exceptionThrown')errors.push(m.params.exceptionDetails.exception?.description||'browser exception');});
 await cdp('Page.enable');await cdp('Runtime.enable');await cdp('Emulation.setFocusEmulationEnabled',{enabled:true});await cdp('Emulation.setDeviceMetricsOverride',{width:1440,height:1000,deviceScaleFactor:1,mobile:false});await cdp('Page.navigate',{url:origin});
 await pageWait("document.querySelector('#settings-subagent-tab')&&document.querySelector('#model-label').textContent==='parent-model'&&!document.querySelector('#prompt').disabled",'subagent module');
 const id=await begin('PARENT_ONLY_FACT');await completed(id);
 let release;gate={promise:new Promise(r=>release=r)};gate.release=release;const before=calls.length;await begin('DELEGATE_ROOT');await wait(()=>spawnSeen,'parent bound before config change');
 const config=await api('/api/model-settings');await api('/api/model-settings/default',{revision:config.revision,provider_id:'environment',model_id:'later-model'});release();gate=null;await completed(id);
 const agents=(await api(`/api/sessions/${id}/agents`)).agents;assert.equal(agents.length,2);const worker=agents.find(a=>a.role==='worker'),explorer=agents.find(a=>a.role==='explorer');
 check('model-driven spawn, wait, message and follow-up complete with actual tools',worker.turns===2&&explorer.turns===1&&worker.status==='completed'&&explorer.status==='completed');
 const doc=await api('/api/sessions/'+id);check('child writes only the assigned actual workspace file',await readFile(join(doc.session.workspace,'child-result.txt'),'utf8')==='CHILD_FILE_OK');
 check('worker model is separately pinned while explorer and parent keep their inherited model',worker.model.model_id==='worker-model'&&explorer.model.model_id==='parent-model'&&calls.slice(before).filter(r=>r.messages.some(m=>m.content==='DELEGATE_ROOT')&&!r.messages.some(m=>m.content==='WRITE_CHILD')).some(r=>r.model==='parent-model'));
 check('delegation never changes the global default model',(await api('/api/model-settings')).default.model_id==='later-model');
 const turn=doc.turns.at(-1),history=await api(`/api/sessions/${id}/turns/${turn.id}`);
 check('parent accounting includes each child model attempt exactly once',history.snapshot.outcome.task_usage.model_calls===calls.length-before);
 check('child records do not pollute top-level conversation navigation',(await api('/api/sessions')).sessions.length===1);
 const other=await api('/api/sessions',{request_id:'other-root'});await api(`/api/sessions/${other.id}/agents/${worker.id}`,null,409);
 await api(`/api/sessions/${worker.id}/turns`,{request_id:'illegal-standalone',revision:worker.revision,prompt:'must not start'},400);
 check('cross-root access and direct child starts cannot bypass owned delegation',true);
 await openChildren();await pageWait("document.querySelectorAll('.subagent-row').length===2",'real child list');await click(`[data-child="${worker.id}"]`);
 await pageWait("document.querySelector('.subagent-history').textContent.includes('FOLLOWUP_OK')",'safe child history');check('Web displays the saved child result, actual model and prior activations',await evaluate("document.querySelector('.subagent-detail').textContent.includes('worker-model')&&document.querySelector('.subagent-turns').options.length===2"));await shot('subagent-desktop');
 const count=calls.length;await stop(host);await start(origin);await cdp('Page.reload',{ignoreCache:true});await pageWait("document.querySelector('#settings-subagent-tab')&&!document.querySelector('#prompt').disabled",'reopen');await openChildren();
 await pageWait("document.querySelectorAll('.subagent-row').length===2",'recovered list');check('restart restores child records without executing them',calls.length===count);
 await click('#files-close');await begin(`CONTINUE_ROOT ${worker.id}`);await completed(id);check('a new parent activation can continue the same persisted child',(await api(`/api/sessions/${id}/agents/${worker.id}`)).agent.turns===3);
 await click('#composer-add');await click('[data-command=plan]');await pageWait("!document.querySelector('#planner-mode-chip').hidden&&!document.querySelector('#planner-mode-chip').disabled",'planning');await begin('PLAN_BOUNDARY');await completed(id);check('Planner cannot delegate around its read-only tool ceiling',true);await click('#planner-mode-chip');await pageWait("document.querySelector('#planner-mode-chip').hidden",'normal mode');
 await click('#sandbox-permission');await click('[data-sandbox-mode="read-only"]');await pageWait("document.querySelector('#sandbox-permission .permission-trigger-label').textContent==='仅可查看'&&!document.querySelector('#sandbox-permission').disabled",'read-only');await begin('READONLY_ROOT');await completed(id);
 let exists=true;try{await access(join(doc.session.workspace,'must-not-exist.txt'));}catch{exists=false;}check('read-only sandbox remains enforced inside delegated tools',!exists);
 let shellExists=true;try{await access(join(doc.session.workspace,'must-not-exist-shell.txt'));}catch{shellExists=false;}check('native Shell inside the child uses the same read-only sandbox',!shellExists);
 await begin('STOP_CHILD_ROOT');let live;
 await wait(async()=>{live=(await api(`/api/sessions/${id}/agents`)).agents.find(a=>a.status==='running');return!!live&&blocked.size>0;},'blocking child');await openChildren();await pageWait(`document.querySelector('[data-child="${live.id}"]')`,'live row');await click(`[data-child="${live.id}"]`);
 await pageWait("document.querySelector('.subagent-actions button:last-child')&&!document.querySelector('.subagent-actions button:last-child').disabled",'stop child ready');await click('.subagent-actions button:last-child');await completed(id);
 check('stopping a child through Web settles it without cancelling the parent',(await api(`/api/sessions/${id}/agents/${live.id}`)).agent.status==='cancelled');await click('#files-close');
 await begin('PARENT_CANCEL_ROOT');await wait(async()=>{live=(await api(`/api/sessions/${id}/agents`)).agents.find(a=>a.status==='running');return!!live&&blocked.size>0;},'child before parent stop');await click('#cancel');await wait(async()=>{const d=await api('/api/sessions/'+id);return d.session.status==='cancelled';},'parent cancelled');
 await wait(()=>blocked.size===0,'model requests cancelled');check('parent cancellation stops owned child requests and leaves no running descendants',!(await api(`/api/sessions/${id}/agents`)).agents.some(a=>a.status==='running'));
 await pageWait("!document.querySelector('#prompt').disabled",'cancel settled');await openChildren();
 for(const width of [390,320]){await cdp('Emulation.setDeviceMetricsOverride',{width,height:900,deviceScaleFactor:1,mobile:true});check(`subagent panel fits ${width}px`,await evaluate('document.documentElement.scrollWidth<=innerWidth+1'));await shot('subagent-'+width);}
 await cdp('Emulation.setDeviceMetricsOverride',{width:1440,height:1000,deviceScaleFactor:1,mobile:false});await click('#files-close');await click('#settings-button');await click('#settings-subagent-tab');await pageWait("document.querySelector('#subagent-max_parallel').value==='4'&&!document.querySelector('#subagent-save').disabled",'config view');
 await fill('#subagent-role','explorer');await fill('#subagent-instructions','REVIEW_ONLY');await fill('#subagent-tools','');await fill('#subagent-role','worker');await fill('#subagent-role','explorer');
 check('switching role editors preserves unsaved fields and an explicit empty tool ceiling',await evaluate("document.querySelector('#subagent-instructions').value==='REVIEW_ONLY'&&!document.querySelector('#subagent-inherit-tools').checked&&document.querySelector('#subagent-tools').value===''") );
 await fill('#subagent-max_parallel','3');await click('#subagent-save');await pageWait("document.querySelector('#subagent-settings-notice').textContent.includes('重启')",'configuration persisted');
 check('settings UI saves pending configuration but keeps actual runtime limits',(await api('/api/subagents')).active.max_parallel===4&&(await api('/api/subagents')).config.max_parallel===3);await shot('subagent-settings');
 check('saving an empty tool whitelist never expands it to inherited tools',Array.isArray((await api('/api/subagents')).config.roles.explorer.tools)&&(await api('/api/subagents')).config.roles.explorer.tools.length===0);
 await click('#settings-appearance-tab');await click('#theme-mode-dark');await click('#settings-subagent-tab');await pageWait("document.documentElement.dataset.theme==='dark'&&!document.querySelector('#subagent-settings-section').hidden",'dark settings');check('child settings use the actual dark theme without losing saved state',await evaluate("document.querySelector('#subagent-max_parallel').value==='3'"));await shot('subagent-settings-dark');
 const beforeDisable=calls.length;await stop(host);await start(origin,{AGENT_SUBAGENT:'0'});await cdp('Page.navigate',{url:origin+'/#session='+id});await pageWait("document.querySelector('#settings-subagent-tab')&&!document.querySelector('#prompt').disabled",'disabled assembly');await openChildren();await pageWait("document.querySelector('.subagent-notice')?.textContent.includes('未启用')",'historical children');
 check('disabling Subagent retains read-only history and no background continuation',(await api(`/api/sessions/${id}/agents`)).enabled===false&&(await api(`/api/sessions/${id}/agents`)).agents.length>=4&&calls.length===beforeDisable);
 const settings=await api('/api/subagents');await api('/api/subagents',{revision:settings.revision,config:{...settings.config,max_parallel:0}},409);
 check('invalid extension configuration is rejected without overwrite',(await api('/api/subagents')).config.max_parallel===3);
 check('no unhandled browser or controlled-model errors',errors.length===0);
 await writeFile(join(report,'result.json'),JSON.stringify({passed:true,checks,model_requests:calls.length,real_provider:false,isolated:true},null,2));
}catch(error){console.error(error);console.error(hostLog);if(socket?.readyState===1){console.error(await evaluate("document.body.innerText.slice(-4000)").catch(()=>''));await shot('failure').catch(()=>{});}await writeFile(join(report,'result.json'),JSON.stringify({passed:false,checks,error:String(error),errors},null,2));process.exitCode=1;}
finally{gate?.release?.();for(const res of blocked)res.destroy();if(socket?.readyState===1){await cdp('Browser.close').catch(()=>{});socket.close();}for(const p of pending.values())clearTimeout(p.timer);await stop(browser);await stop(host);ui?.closeAllConnections();fixture.closeAllConnections();if(ui)await new Promise(r=>ui.close(r));await new Promise(r=>fixture.close(r));await rm(scratch,{recursive:true,force:true,maxRetries:8,retryDelay:200});}
