// Real isolated Rust host + built Web modules + deterministic model + actual file tool.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, mkdir, readFile, writeFile, rm, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startUiServer } from '../../../scripts/dev-web.mjs';
const root = fileURLToPath(new URL('../../../', import.meta.url));
const scratch = await mkdtemp(join(tmpdir(), 'mona-ui-modules-'));
const state = join(scratch,'state'), home = join(scratch,'home');
for (const p of [state,home]) await mkdir(p,{recursive:true});
const report = resolve(process.env.MONA_TEST_REPORT_DIR || join(root,'target/ui-extensions-validation/browser'));
await mkdir(report,{recursive:true});
const token='ui-extensions-isolated-test-credentials-32chars';
const checks=[],errors=[],calls=[];let host,browser,ui,socket,hostLog='';
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
function check(name,value){assert.ok(value,name);checks.push(name);console.log('PASS '+name);}
async function waitFor(fn,label,timeout=35000){const end=Date.now()+timeout;while(Date.now()<end){if(await fn())return;await sleep(80);}throw Error('Timeout: '+label);}
async function stop(child){if(!child||child.exitCode!=null)return;const exit=once(child,'exit');child.kill('SIGKILL');await Promise.race([exit,sleep(5000)]);}
async function port(){const server=createServer();await new Promise(r=>server.listen(0,'127.0.0.1',r));const p=server.address().port;await new Promise(r=>server.close(r));return p;}
const hostPort=await port(), endpoint=`http://127.0.0.1:${hostPort}`;
const proposal='# 执行方案\n\n只在当前工作目录写入 result.txt，正文为 RESULT_CONFIRMED。不得运行 Shell 或更改其他文件。';
let writeCalls=0;
const fixture=createServer(async(req,res)=>{
  try{
    let raw='';for await(const chunk of req){raw+=chunk;if(raw.length>4*1024*1024)throw Error('fixture request bound');}
    const body=JSON.parse(raw);calls.push(body);
    const sources=body.messages.filter(m=>m.role==='user'&&typeof m.content==='string'&&m.content.startsWith('[Host context source:'));
    const encoded=sources.find(m=>m.content.includes('planner.state'))?.content;
    const plan=encoded?JSON.parse(encoded.slice(encoded.indexOf('{'))):null;
    const anchor=body.messages.findLastIndex(m=>m.role==='user'&&typeof m.content==='string'&&!m.content.startsWith('[Host context source:')&&!m.content.startsWith('[Earlier conversation summary'));
    const prompt=body.messages[anchor]?.content||'', tail=body.messages.slice(anchor+1);
    const outputs=tail.filter(m=>m.role==='tool').map(m=>({id:m.tool_call_id,...JSON.parse(m.content)}));
    res.writeHead(200,{'content-type':'text/event-stream','cache-control':'no-store'});
    const frame=value=>res.write(`data: ${JSON.stringify(value)}\n\n`);
    const text=value=>frame({choices:[{index:0,delta:{content:value},finish_reason:'stop'}]});
    const tools=items=>frame({choices:[{index:0,delta:{tool_calls:items.map(([name,args],index)=>({index,id:`ui-call-${calls.length}-${index}`,type:'function',function:{name,arguments:JSON.stringify(args)}}))},finish_reason:'tool_calls'}]});
    if(prompt==='PLAN_TASK'){
      assert.equal(plan?.mode,'plan_only');assert.ok(!body.tools.some(t=>['write','edit','shell'].includes(t.function.name)));
      if(!plan.steps.length)tools([['plan_update',{revision:plan.revision,goal:'受限写入任务',steps:[{id:'write',text:'写入并核对 result.txt',status:'pending'}]}]]);
      else if(!plan.proposal)tools([['plan_submit',{revision:plan.revision,plan:proposal}],['read',{path:'must-not-read.txt'}]]);
      else{assert.ok(outputs.some(o=>o.status==='denied'),'post-submit call must be denied');text(plan.proposal);}
    }else if(prompt==='按此计划继续执行。'){
      assert.equal(plan?.mode,'normal');assert.equal(plan.proposal,proposal);
      if(!outputs.length){writeCalls++;tools([['write',{path:'result.txt',content:'RESULT_CONFIRMED'}]]);}
      else if(plan.steps.some(s=>s.status!=='completed')){
        assert.equal(outputs[0].status,'success');
        tools([['plan_update',{revision:plan.revision,goal:plan.goal,steps:plan.steps.map(s=>({...s,status:'completed'}))}]]);
      }else text('EXECUTION_FINISHED');
    }else text('ACK '+prompt);
    frame({choices:[],usage:{prompt_tokens:Math.ceil(Buffer.byteLength(raw)/3),completion_tokens:32}});res.end('data: [DONE]\n\n');
  }catch(error){errors.push('model: '+String(error));res.destroy();}
});
await new Promise(r=>fixture.listen(0,'127.0.0.1',r));
async function api(path,body,expected=200,auth=true,method=body==null?'GET':'POST'){
  const response=await fetch(endpoint+path,{method,headers:{...(auth?{Authorization:`Bearer ${token}`} :{}),'Content-Type':'application/json'},body:body==null?undefined:JSON.stringify(body),signal:AbortSignal.timeout(20000)});
  const content=await response.text();assert.equal(response.status,expected,path+' '+content);return content?JSON.parse(content):null;
}
async function startHost(origin){
  const env=Object.fromEntries(Object.entries(process.env).filter(([key])=>!key.startsWith('AGENT_')&&!key.startsWith('MONA_DEV_')));
  Object.assign(env,{MONA_DEV_STATE_DIR:state,HOME:home,USERPROFILE:home,AGENT_SERVER_ADDR:`127.0.0.1:${hostPort}`,AGENT_SERVER_TOKEN:token,AGENT_UI_ORIGIN:origin,
    AGENT_MODEL_MANAGEMENT:'0',AGENT_MODEL_NAME:'ui-fixture',AGENT_MODEL_ENDPOINT:`http://127.0.0.1:${fixture.address().port}/v1/chat/completions`,
    AGENT_ALLOW_HTTP_LOOPBACK:'1',AGENT_CAPABILITY_STATE_PATH:join(state,'capabilities.json'),AGENT_SPILL_DIR:join(state,'spill')});
  const binary=resolve(process.env.MONA_TEST_SERVER||join(root,'target/debug',process.platform==='win32'?'server.exe':'server'));await access(binary);
  hostLog='';host=spawn(binary,[],{cwd:scratch,env,stdio:['ignore','pipe','pipe']});host.stderr.on('data',c=>hostLog=(hostLog+c).slice(-16000));host.on('error',e=>errors.push(String(e)));
  await waitFor(async()=>{if(host.exitCode!=null)throw Error(hostLog);try{return(await api('/v1/info')).protocol_version===2;}catch{return false;}},'host ready');
}
let sequence=0;const pending=new Map();
function cdp(method,params={}){return new Promise((resolve,reject)=>{const id=++sequence,timer=setTimeout(()=>{pending.delete(id);reject(Error('CDP '+method));},20000);pending.set(id,{resolve,reject,timer});socket.send(JSON.stringify({id,method,params}));});}
async function evaluate(expression){const result=await cdp('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(result.exceptionDetails)throw Error(result.exceptionDetails.exception?.description||'browser expression failed');return result.result.value;}
const waitPage=(expression,label)=>waitFor(()=>evaluate(expression),label);
async function click(selector){const p=await evaluate(`(()=>{const n=document.querySelector(${JSON.stringify(selector)});if(!n)throw Error('missing element');n.scrollIntoView({block:'nearest'});const r=n.getBoundingClientRect();if(!r.width||!r.height)throw Error('hidden element');return{x:r.x+r.width/2,y:r.y+r.height/2}})()`);await cdp('Input.dispatchMouseEvent',{type:'mousePressed',button:'left',clickCount:1,...p});await cdp('Input.dispatchMouseEvent',{type:'mouseReleased',button:'left',clickCount:1,...p});}
async function fill(selector,value){await evaluate(`(()=>{const n=document.querySelector(${JSON.stringify(selector)});n.value=${JSON.stringify(value)};n.dispatchEvent(new Event('input',{bubbles:true}));n.dispatchEvent(new Event('change',{bubbles:true}));})()`);}
async function shot(name){const image=await cdp('Page.captureScreenshot',{format:'png',captureBeyondViewport:false});await writeFile(join(report,name+'.png'),Buffer.from(image.data,'base64'));}
async function current(){const id=await evaluate("location.hash.slice('#session='.length)");return api('/api/sessions/'+id);}
async function submit(text){await fill('#prompt',text);await click('#send');await waitPage("/^#session=[A-Za-z0-9_-]+$/.test(location.hash)",'session admitted');await waitFor(async()=>{const doc=await current();if(doc.session.status==='failed')throw Error(JSON.stringify(doc));return doc.session.status==='completed';},'completed turn');await waitPage("document.querySelector('#cancel').hidden&&!document.querySelector('#sessions-refresh').disabled",'settled view');}
try{
  ui=await startUiServer({host:'127.0.0.1',port:0,endpoint,token});const origin=`http://127.0.0.1:${ui.address().port}`;await startHost(origin);
  await api('/api/ui',null,401,false);const manifest=await api('/api/ui');check('host advertises IDs and actual active capability, never script URLs',manifest.version===1&&manifest.modules.includes('planner')&&manifest.capabilities.planner.active&&!JSON.stringify(manifest).includes(token));
  const debugPort=await port(),executable=process.env.BROWSER_BIN||(process.platform==='win32'?'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe':'chromium');
  browser=spawn(executable,['--headless=new','--disable-gpu','--no-first-run','--no-default-browser-check','--edge-skip-compat-layer-relaunch',`--remote-debugging-port=${debugPort}`,`--user-data-dir=${join(scratch,'browser')}`,'--window-size=1440,1000','about:blank'],{stdio:'ignore'});
  let page;await waitFor(async()=>{try{page=(await(await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json()).find(p=>p.type==='page');return!!page;}catch{return false;}},'browser ready');
  socket=new WebSocket(page.webSocketDebuggerUrl);await once(socket,'open');socket.addEventListener('message',event=>{const message=JSON.parse(event.data),p=pending.get(message.id);if(p){pending.delete(message.id);clearTimeout(p.timer);message.error?p.reject(Error(JSON.stringify(message.error))):p.resolve(message.result);}if(message.method==='Runtime.exceptionThrown')errors.push(message.params.exceptionDetails.exception?.description||'browser exception');});
  await cdp('Page.enable');await cdp('Runtime.enable');await cdp('Page.navigate',{url:origin});
  await waitPage("document.querySelector('#sessions-notice')?.textContent.includes('已保存在宿主本地')&&document.querySelector('#composer-add')&&!document.querySelector('#composer-add').hidden&&document.querySelector('#sandbox-permission')&&!document.querySelector('#sandbox-permission').disabled",'composed host UI');
  check('ordinary mode is the default without approval controls',await evaluate("document.querySelector('#planner-mode-chip')?.hidden===true&&!document.querySelector('#model-button')"));
  const menuChecks = await evaluate("(async()=>{const m=await import('/apps/web/test/command-menu-checks.mjs');return m.commandMenuChecks(['plan','permission']);})()");
  for (const name of menuChecks) check(name, true);
  await click('#composer-add'); await shot('command-menu-light'); await click('[data-command=permission]');
  await waitPage("!document.querySelector('#sandbox-permission-menu').hidden", 'shared permission picker');
  check('permission command opens the existing picker without switching mode or starting a Run', calls.length === 0 && (await api('/api/sessions')).sessions.length === 0 && (await api('/api/sandbox')).mode === 'workspace-write');
  await evaluate("document.querySelector('#sandbox-permission').click()");
  await submit('TRIVIAL');check('ordinary task needs one call, not an extra planning or approval Run',calls.length===1&&writeCalls===0);
  await click('#new-chat');await click('#composer-add');await waitPage("!document.querySelector('#composer-command-menu').hidden&&!document.querySelector('[data-command=plan]').disabled",'plan command menu');await click('[data-command=plan]');
  await waitFor(async()=>{const docs=(await api('/api/sessions')).sessions;return docs.some(d=>d.status==='idle');},'empty planned session');
  await waitPage("document.querySelector('#planner-mode-chip')&&!document.querySelector('#planner-mode-chip').hidden&&!document.querySelector('#planner-mode-chip').disabled",'server-confirmed mode');
  let doc=await current(),id=doc.session.id;check('selecting mode creates durable session state without calling the model',doc.session.turn_count===0&&calls.length===1);
  await click('#composer-add'); await click('[data-command=plan]');
  await waitPage("document.querySelector('#planner-mode-chip').hidden", 'plan menu toggles off');
  check('plan menu exits empty planning mode without starting a Run', (await api(`/api/sessions/${id}/plan`)).plan.mode === 'normal' && calls.length === 1);
  await click('#composer-add'); await click('[data-command=plan]');
  await waitPage("!document.querySelector('#planner-mode-chip').hidden&&!document.querySelector('#planner-mode-chip').disabled", 'plan menu toggles on');
  await cdp('Page.reload',{ignoreCache:true});await waitPage("document.querySelector('#planner-mode-chip')&&!document.querySelector('#planner-mode-chip').hidden&&!document.querySelector('#planner-mode-chip').disabled",'mode survives reload');
  await submit('PLAN_TASK');const saved=await api(`/api/sessions/${id}/plan`);
  check('plan submission is durable and no business write has run',saved.plan.proposal===proposal&&writeCalls===0&&saved.plan.mode==='plan_only');
  await api(`/api/sessions/${id}/plan`,{revision:saved.revision,plan_revision:saved.plan.revision-1,action:'resume'},409);
  await api(`/api/sessions/${id}/plan`,{revision:saved.revision,plan_revision:saved.plan.revision,action:'normal'},400);
  await api(`/api/sessions/${id}/plan`,{revision:saved.revision,plan_revision:saved.plan.revision,action:'resume',seed:{}},400);
  check('stale approvals, generic mode bypass and arbitrary seed fields are rejected',true);
  await waitPage("!document.querySelector('#composer-ui-dock button').hidden",'plan dock');await click('#composer-ui-dock button');
  await waitPage("document.querySelector('.planner-panel')?.textContent.includes('按此计划执行')",'plan panel');
  check('registered pane displays exact persisted proposal and structured steps',await evaluate("document.querySelector('.planner-panel').textContent.includes('不得运行 Shell')&&document.querySelectorAll('.planner-panel [data-plan-step]').length===1"));
  await shot('plan-desktop');
  await click('#files-close');
  await stop(host);await startHost(origin);await cdp('Page.reload',{ignoreCache:true});
  await waitPage("document.querySelector('#planner-mode-chip')&&!document.querySelector('#planner-mode-chip').hidden&&!document.querySelector('#planner-mode-chip').disabled",'host restart plan');
  check('host restart restores submitted plan without executing tools',writeCalls===0&&(await api(`/api/sessions/${id}/plan`)).plan.proposal===proposal);
  await click('#composer-ui-dock button');await waitPage("Array.from(document.querySelectorAll('.planner-panel button')).some(b=>b.textContent==='按此计划执行'&&!b.disabled)",'resume enabled');
  await fill('#prompt','保留这段草稿');
  await waitPage("document.querySelector('#prompt').value==='保留这段草稿'&&Array.from(document.querySelectorAll('.planner-panel button')).some(b=>b.textContent==='按此计划执行'&&!b.disabled)",'draft resume action ready');
  await evaluate("Array.from(document.querySelectorAll('.planner-panel button')).find(b=>b.textContent==='按此计划执行').click()");
  await waitPage("document.querySelector('.planner-notice')?.textContent.includes('草稿')",'draft protected');
  check('resume with an existing draft leaves the real saved plan in plan-only mode', (await api(`/api/sessions/${id}/plan`)).plan.mode==='plan_only'&&writeCalls===0);
  await fill('#prompt','');
  await evaluate("Array.from(document.querySelectorAll('.planner-panel button')).find(b=>b.textContent==='按此计划执行').click()");
  await waitFor(async()=>{const value=await api(`/api/sessions/${id}/plan`);return value.status==='completed'&&value.plan.steps[0].status==='completed';},'execution with same session');
  await waitPage("document.querySelector('#cancel').hidden&&!document.querySelector('#sessions-refresh').disabled",'execution settled');
  doc=await current();check('explicit resume uses the same session and performs the actual write once',doc.session.id===id&&doc.session.turn_count===2&&writeCalls===1&&(await readFile(join(doc.session.workspace,'result.txt'),'utf8'))==='RESULT_CONFIRMED');
  check('completed plan retains the exact submitted baseline',(await api(`/api/sessions/${id}/plan`)).plan.proposal===proposal);
  await evaluate(`document.querySelector('#transport').value='http';document.querySelector('#endpoint').value=${JSON.stringify(endpoint)};document.querySelector('#token').value=${JSON.stringify(token)};document.querySelector('#connection-form').requestSubmit();`);
  await waitPage("document.querySelector('#sessions-notice')?.textContent.includes('已保存在宿主本地')&&document.querySelector('#planner-mode-chip')?.hidden===true&&!document.querySelector('#connect').disabled",'same-page reconnection');
  check('same-page reconnection remounts settings and controls without duplicate DOM or lost session',await evaluate(`document.querySelectorAll('#settings-memory-tab').length===1&&document.querySelectorAll('#memory-settings-section').length===1&&document.querySelectorAll('#planner-mode-chip').length===1&&location.hash==='#session=${id}'`));
  await click('#files-close');await click('#settings-button');await click('#settings-memory-tab');
  await waitPage("!document.querySelector('#memory-add').disabled",'memory companion');await click('#memory-add');await fill('#memory-text','UI_MODULE_MEMORY_FACT');await click('#memory-save');
  await waitPage("!document.querySelector('#memory-dialog').open&&document.querySelector('#memory-entries').textContent.includes('UI_MODULE_MEMORY_FACT')",'memory saved');
  check('existing memory management survives module migration',(await readFile(join(state,'memory/personal/MEMORY.md'),'utf8')).includes('UI_MODULE_MEMORY_FACT'));
  await click('#settings-components-tab');await waitPage("Array.from(document.querySelectorAll('.capability-row')).some(n=>n.textContent.includes('计划管理'))",'planner setting');
  await evaluate("Array.from(document.querySelectorAll('#component-list .capability-row')).find(n=>n.textContent.includes('计划管理')).querySelector('button').click()");
  await waitFor(async()=>{const value=await api('/api/ui');return value.capabilities.planner.enabled===false;},'disable desired');
  check('desired disable does not pretend the active host already changed',(await api('/api/ui')).capabilities.planner.active===true);
  await stop(host);await startHost(origin);await cdp('Page.reload',{ignoreCache:true});
  await waitPage("document.querySelector('#sessions-notice')?.textContent.includes('已保存在宿主本地')&&(!document.querySelector('#planner-mode-chip')||document.querySelector('#planner-mode-chip').hidden)",'disabled planner module');
  check('disabled planner preserves old plan and hides new mode operations',!(await api('/api/ui')).capabilities.planner.active&&(await api(`/api/sessions/${id}/plan`)).plan.steps[0].status==='completed');
  // Capability visibility can settle before the independent persisted-plan read.
  await waitPage("document.querySelector('.planner-dock button')?.getClientRects().length>0",'persisted historical plan control ready');
  await click('#composer-ui-dock button');await waitPage("document.querySelector('.planner-panel')?.textContent.includes('历史仅供查看')",'history read only');
  check('historical plan renderer remains available independently of active execution',true);
  await click('#files-close');
  for(const width of [390,320]){
    await cdp('Emulation.setDeviceMetricsOverride',{width,height:900,deviceScaleFactor:1,mobile:true});
    check(`module composer fits ${width}px`,await evaluate('document.documentElement.scrollWidth<=innerWidth+1'));
    await click('#composer-add');
    check(`command menu fits ${width}px and excludes disabled Planner`, await evaluate("(()=>{const r=document.querySelector('#composer-command-menu').getBoundingClientRect();return r.width>0&&r.left>=0&&r.right<=innerWidth&&r.top>=0&&r.bottom<=innerHeight&&!document.querySelector('#composer-command-menu [data-command=plan]');})()"));
    await shot('commands-'+width); await click('#composer-add');
  }
  await cdp('Emulation.setDeviceMetricsOverride',{width:1440,height:1000,deviceScaleFactor:1,mobile:false});
  check('slot removal is owned and a failing renderer safely falls back',await evaluate(`(async()=>{
    const {uiRegistry:r}=await import('/apps/web/ui/registry.mjs');const {structuredDetailsBlock}=await import('/apps/web/content-renderer.mjs');
    const before=r.entries('tool.view').length;const dispose=r.register('test','tool.view',{id:'test.bad',value:()=>{throw Error('controlled renderer failure');}});
    const node=structuredDetailsBlock({'test.bad':{safe:'<script>never execute</script>'}});dispose();dispose();
    return !!node.querySelector('.structured-json')&&!node.querySelector('script')&&r.entries('tool.view').length===before;
  })()`));
  const lifecycleChecks=await evaluate("(async()=>{const m=await import('/apps/web/test/ui-module-checks.mjs');return m.runUiModuleChecks();})()");
  for(const name of lifecycleChecks)check(name,true);
  check('no browser or model fixture exceptions',errors.length===0);
  await writeFile(join(report,'result.json'),JSON.stringify({passed:true,checks,model_requests:calls.length,write_calls:writeCalls,real_provider:false,isolated:true},null,2));
}catch(error){console.error(error);console.error(hostLog);if(socket?.readyState===WebSocket.OPEN){console.error(await evaluate("JSON.stringify({notice:document.querySelector('#sessions-notice')?.textContent,body:document.body.innerText.slice(-2500)})").catch(()=>''));await shot('failure').catch(()=>{});}await writeFile(join(report,'result.json'),JSON.stringify({passed:false,checks,error:String(error),errors},null,2));process.exitCode=1;}
finally{
  if(socket?.readyState===WebSocket.OPEN){await cdp('Browser.close').catch(()=>{});socket.close();}
  for(const request of pending.values()){clearTimeout(request.timer);request.reject(Error('test cleanup'));}pending.clear();
  await stop(browser);await stop(host);ui?.closeAllConnections?.();if(ui)await new Promise(r=>ui.close(r));fixture.closeAllConnections();await new Promise(r=>fixture.close(r));
  await rm(scratch,{recursive:true,force:true,maxRetries:8,retryDelay:200});
}
