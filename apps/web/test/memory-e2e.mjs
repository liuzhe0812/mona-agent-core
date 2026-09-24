// Real Web, Rust Runtime/Memory/Sessions, private test state and controlled HTTP model only.
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
const scratch = await mkdtemp(join(tmpdir(), 'mona-memory-e2e-'));
const state = join(scratch, 'state'), home = join(scratch, 'home'), project = join(scratch, 'project');
for (const p of [state, home, project]) await mkdir(p, { recursive: true });
const report = resolve(process.env.MONA_TEST_REPORT_DIR || join(root, 'target/memory-validation/browser'));
await mkdir(report, { recursive: true });
const token = 'isolated-memory-host-token-at-least-32-chars';
const calls = [], checks = [], errors = []; let summaries = 0, host, browser, socket, ui, hostLog = '';
const sleep = ms => new Promise(r => setTimeout(r, ms));
function check(name, ok) { assert.ok(ok, name); checks.push(name); console.log(`PASS ${name}`); }
async function waitFor(fn, label, timeout = 30000) { const end = Date.now()+timeout; while (Date.now()<end) { if (await fn()) return; await sleep(60); } throw new Error(`Timeout: ${label}`); }
async function stop(child) { if (!child || child.exitCode != null) return; const closed = once(child, 'exit'); child.kill('SIGKILL'); await Promise.race([closed, sleep(5000)]); }
async function freePort() { const s = createServer(); await new Promise(r => s.listen(0,'127.0.0.1',r)); const p=s.address().port; await new Promise(r=>s.close(r));return p; }
const port=await freePort(),endpoint=`http://127.0.0.1:${port}`;
function latest(body) { return body.messages.filter(m=>m.role==='user' && typeof m.content==='string' && !m.content.startsWith('[Host context source:') && !m.content.startsWith('[Earlier conversation summary')).at(-1)?.content || ''; }
function toolOutput(body) { const last=body.messages.at(-1);if(last?.role!=='tool')return null;const envelope=JSON.parse(last.content);return {name:body.messages.findLast(m=>m.role==='assistant'&&m.tool_calls?.some(c=>c.id===last.tool_call_id))?.tool_calls.find(c=>c.id===last.tool_call_id)?.function.name,status:envelope.status,data:JSON.parse(envelope.content)}; }
const fixture=createServer(async(req,res)=>{
  try {
    let raw='';for await(const chunk of req){raw+=chunk;if(raw.length>4*1024*1024)throw Error('fixture body bound');}
    const body=JSON.parse(raw);calls.push(body);const prompt=latest(body),summary=body.messages[0]?.content?.startsWith('Summarize the earlier');
    res.writeHead(200,{'content-type':'text/event-stream','cache-control':'no-store'});
    const frame=value=>res.write(`data: ${JSON.stringify(value)}\n\n`);
    const text=value=>frame({choices:[{index:0,delta:{content:value},finish_reason:'stop'}]});
    const call=(name,args)=>{assert.ok(body.tools.some(t=>t.function.name===name),'requested tool must actually be offered');frame({choices:[{index:0,delta:{tool_calls:[{index:0,id:`memory-call-${calls.length}`,type:'function',function:{name,arguments:JSON.stringify(args)}}]},finish_reason:'tool_calls'}]});};
    if(summary){summaries++;text(JSON.stringify({goal:'Continue the test task',constraints:['Keep personal memory scoped'],corrections:[],decisions:[],completed:['Initial conversation recorded'],pending:[],references:[]}));}
    else if(prompt==='MEMORY_AGENT_SAVE'){
      const result=toolOutput(body);
      if(!result)call('memory_read',{scope:'personal'});
      else if(result.name==='memory_read')call('memory_update',{scope:'personal',revision:result.data.revision,operations:[{action:'add',text:'AGENT_CONFIRMED_LONG_TERM_27182'}]});
      else{assert.equal(result.status,'success');text('MEMORY_AGENT_SAVED');}
    }else if(prompt==='RECALL_ORIGINAL'){
      const result=toolOutput(body);
      if(!result)call('session_search',{query:'精确往事12345'});
      else if(result.name==='session_search'){
        const h=result.data.hits.find(h=>h.role==='user');assert.ok(h,'original must be searchable');
        call('session_read',{session_id:h.session_id,revision:h.revision,message_index:h.message_index,message_hash:h.message_hash,max_bytes:8192});
      }else{text('RECALL_OK '+result.data.text.slice(0,120));}
    }else if(prompt==='CHECK_PROJECT_ISOLATION'){
      const result=toolOutput(body);if(!result)call('session_search',{query:'PROJECT_EXCLUSIVE_16180'});else{assert.equal(result.data.hits.length,0);text('PROJECT_NOT_EXPOSED');}
    }else if(prompt==='PRIVATE_NATIVE'){
      frame({choices:[{index:0,delta:{content:'PRIVATE_RECORDED',reasoning_content:'HIDDEN_REASONING_NEVER_INDEX_999'},finish_reason:'stop'}]});
    }else{text(`ACK ${prompt.slice(0,40)}`);}
    frame({choices:[],usage:{prompt_tokens:Math.ceil(Buffer.byteLength(raw)/3),completion_tokens:30}});res.end('data: [DONE]\n\n');
  }catch(e){errors.push(String(e));res.destroy();}
});
await new Promise(r=>fixture.listen(0,'127.0.0.1',r));
async function api(path,body,expected=200,auth=true,method=body==null?'GET':'POST'){
  const response=await fetch(endpoint+path,{method,headers:{...(auth?{Authorization:`Bearer ${token}`} : {}),'Content-Type':'application/json'},body:body==null?undefined:JSON.stringify(body),signal:AbortSignal.timeout(20000)});
  const text=await response.text();if(response.status!==expected)throw Error(`${path}: expected ${expected}, got ${response.status}: ${text}`);return text?JSON.parse(text):null;
}
async function startHost(origin){
  const env=Object.fromEntries(Object.entries(process.env).filter(([k])=>!k.startsWith('AGENT_')&&!k.startsWith('MONA_DEV_')));
  Object.assign(env,{MONA_DEV_STATE_DIR:state,USERPROFILE:home,HOME:home,AGENT_SERVER_ADDR:`127.0.0.1:${port}`,AGENT_SERVER_TOKEN:token,AGENT_UI_ORIGIN:origin,
    AGENT_MODEL_ENDPOINT:`http://127.0.0.1:${fixture.address().port}/chat/completions`,AGENT_MODEL_NAME:'memory-fixture',AGENT_MODEL_CONTEXT_TOKENS:'16384',AGENT_MODEL_MANAGEMENT:'0',AGENT_ALLOW_HTTP_LOOPBACK:'1',AGENT_SKILLS:'0',AGENT_SPILL_DIR:join(state,'spill'),AGENT_CAPABILITY_STATE_PATH:join(state,'capabilities.json')});
  const binary=resolve(process.env.MONA_TEST_SERVER||join(root,'target/context-validation/debug',process.platform==='win32'?'server.exe':'server'));await access(binary);
  hostLog='';host=spawn(binary,[],{cwd:scratch,env,stdio:['ignore','pipe','pipe']});host.stderr.on('data',chunk=>hostLog=(hostLog+chunk).slice(-12000));host.on('error',e=>errors.push(String(e)));
  await waitFor(async()=>{if(host.exitCode!=null)throw Error(hostLog);try{return(await api('/v1/info')).protocol_version===2}catch{return false}},'host ready');
}
let seq=0;const pending=new Map();
function cdp(method,params={}){return new Promise((resolve,reject)=>{const id=++seq,timer=setTimeout(()=>{pending.delete(id);reject(Error(`CDP ${method}`))},20000);pending.set(id,{resolve,reject,timer});socket.send(JSON.stringify({id,method,params}));});}
async function evaluate(expression){const r=await cdp('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(r.exceptionDetails)throw Error(r.exceptionDetails.exception?.description||'page failed');return r.result.value;}
const waitPage=(s,label)=>waitFor(()=>evaluate(s),label);
async function click(selector){const p=await evaluate(`(()=>{const n=document.querySelector(${JSON.stringify(selector)});if(!n)throw Error('missing element');n.scrollIntoView({block:'nearest'});const r=n.getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2}})()`);await cdp('Input.dispatchMouseEvent',{type:'mouseMoved',...p});await cdp('Input.dispatchMouseEvent',{type:'mousePressed',button:'left',clickCount:1,...p});await cdp('Input.dispatchMouseEvent',{type:'mouseReleased',button:'left',clickCount:1,...p});}
async function fill(selector,value){await evaluate(`(()=>{const n=document.querySelector(${JSON.stringify(selector)});n.value=${JSON.stringify(value)};n.dispatchEvent(new Event('input',{bubbles:true}));n.dispatchEvent(new Event('change',{bubbles:true}));})()`);}
async function shot(name){const r=await cdp('Page.captureScreenshot',{format:'png',captureBeyondViewport:false});await writeFile(join(report,name+'.png'),Buffer.from(r.data,'base64'));}
async function submit(prompt,turns){await fill('#prompt',prompt);await click('#send');await waitFor(async()=>{const sessions=(await api('/api/sessions')).sessions;const h=sessions[0];if(h?.status==='failed')throw Error('task failed '+JSON.stringify(await api('/api/sessions/'+h.id)));return h?.turn_count===turns&&h?.status==='completed'},'turn complete');await waitPage(`document.querySelectorAll('#timeline .turn').length===${turns}&&document.querySelector('#cancel').hidden`,'settled UI');}
async function memoryPage(){await click('#settings-button');await click('#settings-memory-tab');await waitPage("!document.querySelector('#memory-refresh').disabled&&!document.querySelector('#memory-add').disabled",'memory loaded');}
async function addFact(text){await click('#memory-add');await fill('#memory-text',text);await click('#memory-save');await waitPage("!document.querySelector('#memory-dialog').open&&!document.querySelector('#memory-refresh').disabled",'memory saved');}
try{
  ui=await startUiServer({host:'127.0.0.1',port:0,endpoint,token});const origin=`http://127.0.0.1:${ui.address().port}`;await startHost(origin);
  check('fresh host exposes independent empty memory and history search', (await api('/api/memory')).enabled&&(await api('/api/memory')).history_enabled&&(await api('/api/memory')).scopes[0].entries.length===0);
  await api('/api/memory',null,401,false);await api('/api/history/search',{query:'x'},401,false);check('memory and history management require host authorization',true);
  const debugPort=await freePort(),exe=process.env.BROWSER_BIN||(process.platform==='win32'?'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe':'chromium');
  browser=spawn(exe,['--headless=new','--disable-gpu','--no-first-run','--no-default-browser-check','--edge-skip-compat-layer-relaunch',`--remote-debugging-port=${debugPort}`,`--user-data-dir=${join(scratch,'browser')}`,'--window-size=1440,1000','about:blank'],{stdio:'ignore'});browser.on('error',e=>errors.push(String(e)));
  let page;await waitFor(async()=>{try{page=(await(await fetch(`http://127.0.0.1:${debugPort}/json/list`,{signal:AbortSignal.timeout(800)})).json()).find(p=>p.type==='page');return!!page}catch{return false}},'browser ready');
  socket=new WebSocket(page.webSocketDebuggerUrl);await once(socket,'open');socket.addEventListener('message',e=>{const v=JSON.parse(e.data),p=pending.get(v.id);if(p){pending.delete(v.id);clearTimeout(p.timer);v.error?p.reject(Error(JSON.stringify(v.error))):p.resolve(v.result)}if(v.method==='Runtime.exceptionThrown')errors.push(v.params.exceptionDetails.exception?.description||'browser exception')});
  await cdp('Page.enable');await cdp('Runtime.enable');await cdp('Page.navigate',{url:origin});await waitPage("document.querySelector('#sessions-notice')?.textContent.includes('已保存在宿主本地')",'connected page');
  await memoryPage();await addFact('个人偏好 MEMORY_FACT_31415：使用中文，不要兼容旧接口。');
  check('real Web saves curated Markdown without issuing a model call', calls.length===0&&(await readFile(join(state,'memory/personal/MEMORY.md'),'utf8')).includes('MEMORY_FACT_31415'));
  const before=(await api('/api/memory')).scopes[0];await api('/api/memory/update',{scope:'../../unauthorized',revision:before.revision,operations:[{action:'add',text:'bad'}]},400);
  await api('/api/memory/update',{scope:'personal',revision:'0'.repeat(64),operations:[{action:'add',text:'bad'}]},409);
  check('scope injection and stale revisions fail without changing saved memory',(await api('/api/memory')).scopes[0].revision===before.revision);
  await cdp('Emulation.setDeviceMetricsOverride',{width:390,height:900,deviceScaleFactor:1,mobile:true});await shot('memory-narrow');
  check('memory page stays within narrow viewport',await evaluate('document.documentElement.scrollWidth<=innerWidth+1'));
  await cdp('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:'dark'}]});await shot('memory-dark');
  await cdp('Emulation.setDeviceMetricsOverride',{width:1440,height:1000,deviceScaleFactor:1,mobile:false});await click('#settings-back');
  await submit('精确往事12345 决策：部署只在内网。',1);let first=(await api('/api/sessions')).sessions[0];
  check('memory is injected but write tool remains disabled by default',JSON.stringify(calls.at(-1)).includes('MEMORY_FACT_31415')&&!calls.at(-1).tools.some(t=>t.function.name==='memory_update'));
  await memoryPage();await fill('#memory-scope','workspace');await addFact('WORKSPACE_ONLY_16180');await click('#settings-back');await submit('CHECK_WORKSPACE_SOURCE',2);
  check('working memory uses the actual session workspace',JSON.stringify(calls.at(-1)).includes('WORKSPACE_ONLY_16180'));
  await submit('LONG_ONE '+'a'.repeat(18000),3);await submit('LONG_TWO '+'b'.repeat(14000),4);
  check('history compaction is independent of curated memory and keeps canonical originals',summaries>0&&(await api(`/api/sessions/${first.id}`)).turns[0].prompt.includes('精确往事12345'));
  await click('#new-chat');await submit('NEW_SESSION',1);const second=(await api('/api/sessions')).sessions[0];
  check('new session inherits personal facts but not another workspace memory',JSON.stringify(calls.at(-1)).includes('MEMORY_FACT_31415')&&!JSON.stringify(calls.at(-1)).includes('WORKSPACE_ONLY_16180'));
  const modelCount=calls.length;await submit('RECALL_ORIGINAL',2);
  check('Agent can search and read original compressed history through existing tools',calls.length-modelCount===3&&JSON.stringify(calls.at(-1)).includes('精确往事12345'));
  await memoryPage();await fill('#history-query','精确往事12345');await click('#history-search-button');await waitPage("document.querySelectorAll('.memory-history-hit').length>0&&!document.querySelector('#history-search-button').disabled",'history results');
  await click('.memory-history-hit .memory-entry-actions button');await waitPage("!document.querySelector('#history-original').hidden&&document.querySelector('#history-read-text').textContent.includes('精确往事12345')",'original text');await shot('history-original');
  check('Web history search reads original text, not a generated summary',true);
  await click('#settings-tools-tab');await waitPage("Array.from(document.querySelectorAll('#tool-list .capability-row')).some(r=>r.textContent.includes('Agent 更新记忆'))",'write permission setting');
  await evaluate("Array.from(document.querySelectorAll('#tool-list .capability-row')).find(r=>r.textContent.includes('Agent 更新记忆')).querySelector('button').click()");
  await waitPage("Array.from(document.querySelectorAll('#tool-list .capability-row')).find(r=>r.textContent.includes('Agent 更新记忆'))?.textContent.includes('重启后启用')",'permission saved');
  await stop(host);await startHost(origin);await cdp('Page.reload',{ignoreCache:true});await waitPage("document.querySelectorAll('#timeline .turn').length===2&&!document.querySelector('#sessions-refresh').disabled",'restarted conversation');
  await submit('MEMORY_AGENT_SAVE',3);
  const updated=(await api('/api/memory')).scopes[0];check('authorized Agent read/update uses current revision and saves actual provenance',updated.entries.some(e=>e.text==='AGENT_CONFIRMED_LONG_TERM_27182'&&e.origin.actor==='agent'&&e.origin.call_id));
  await memoryPage();await fill('#memory-scope','personal');await click('.memory-entry .memory-entry-actions button');await fill('#memory-text','UPDATED_FACT_31415：只在内网 Linux 虚拟机部署。');await click('#memory-save');await waitPage("!document.querySelector('#memory-dialog').open&&!document.querySelector('#memory-refresh').disabled",'edited memory');await click('#settings-back');await submit('AFTER_MEMORY_EDIT',4);
  const source=calls.at(-1).messages.filter(m=>String(m.content).startsWith('[Host context source: memory.'));check('edits refresh the next context source without old fixed-prefix memory',JSON.stringify(source).includes('UPDATED_FACT_31415')&&!JSON.stringify(source).includes('MEMORY_FACT_31415'));
  await submit('PRIVATE_NATIVE',5);
  check('hidden reasoning is neither indexed nor returned by history search',(await api('/api/history/search',{query:'HIDDEN_REASONING_NEVER_INDEX_999'})).hits.length===0);
  let registry=await api('/api/projects');registry=await api('/api/projects',{request_id:'private-project',revision:registry.revision,name:'Memory isolation',path:project});const p=registry.projects.find(p=>p.name==='Memory isolation');
  const ps=await api('/api/sessions',{request_id:'project-session',project_id:p.id});await api(`/api/sessions/${ps.id}/turns`,{request_id:'project-turn',revision:ps.revision,prompt:'PROJECT_EXCLUSIVE_16180'});await waitFor(async()=> (await api(`/api/sessions/${ps.id}`)).session.status==='completed','project turn');
  await submit('CHECK_PROJECT_ISOLATION',6);check('ordinary Agent cannot search project-only history',true);
  // Owner management can inspect its own project records; this does not widen model-tool scope.
  check('owner management retains explicit access to its project history',(await api('/api/history/search',{query:'PROJECT_EXCLUSIVE_16180'})).hits.some(h=>h.session_id===ps.id));
  const entry=(await api('/api/memory')).scopes[0],removed=entry.entries.find(e=>e.text.startsWith('UPDATED_FACT_31415'));
  await api('/api/memory/update',{scope:'personal',revision:entry.revision,operations:[{action:'remove',id:removed.id}]});await submit('AFTER_MEMORY_DELETE',7);
  check('deleted memory does not return through a stale injection cache',!JSON.stringify(calls.at(-1).messages.filter(m=>String(m.content).startsWith('[Host context source: memory.'))).includes('UPDATED_FACT_31415'));
  await stop(host);await startHost(origin);const persisted=(await api('/api/memory')).scopes[0];check('real host restart retains updates and deletions',persisted.entries.some(e=>e.text==='AGENT_CONFIRMED_LONG_TERM_27182')&&!persisted.entries.some(e=>e.text.startsWith('UPDATED_FACT_31415')));
  const current=await api(`/api/sessions/${first.id}`);await api(`/api/sessions/${first.id}/delete`,{revision:current.session.revision},204);
  const remaining=await api('/api/history/search',{query:'精确往事12345'});check('deleted session is excluded after index refresh',remaining.hits.every(h=>h.session_id!==first.id));
  for (const id of ['memory', 'history-search']) {
    const settings = await api('/api/capabilities');
    await api(`/api/capabilities/${id}`, { revision: settings.revision, enabled: false }, 200, true, 'PUT');
  }
  await stop(host); await startHost(origin); await cdp('Page.reload',{ignoreCache:true});
  await waitPage("document.querySelectorAll('#timeline .turn').length===7&&!document.querySelector('#sessions-refresh').disabled",'conversation without memory extensions');
  const disabled = await api('/api/memory');
  check('memory and search can be disabled without deleting their saved records', !disabled.enabled&&!disabled.history_enabled&&disabled.scopes.length===0&&(await readFile(join(state,'memory/personal/MEMORY.md'),'utf8')).includes('AGENT_CONFIRMED_LONG_TERM_27182'));
  await submit('WITHOUT_OPTIONAL_MEMORY',8);
  check('base conversation continues with prior history when both memory capabilities are disabled', !calls.at(-1).tools.some(t=>['memory_read','memory_update','session_search','session_read'].includes(t.function.name))&&!calls.at(-1).messages.some(m=>String(m.content).startsWith('[Host context source: memory.'))&&JSON.stringify(calls.at(-1)).includes('NEW_SESSION'));
  await api('/api/history/search',{query:'fact'},404);
  await click('#settings-button'); await click('#settings-memory-tab');
  await waitPage("document.querySelector('#memory-notice').textContent.includes('未启用')",'disabled memory UI');
  check('disabled product controls report the real assembly state', await evaluate("document.querySelector('#memory-add').disabled&&document.querySelector('#history-search-button').disabled"));
  check('no browser or model fixture exceptions',errors.length===0);
  await writeFile(join(report,'result.json'),JSON.stringify({passed:true,checks,model_requests:calls.length,summaries,real_provider:false,isolated:true},null,2));console.log(`PASS ${checks.length} memory end-to-end assertions`);
}catch(e){console.error(e);console.error(hostLog);if(socket?.readyState===WebSocket.OPEN){console.error(await evaluate("JSON.stringify({memory:document.querySelector('#memory-notice').textContent,form:document.querySelector('#memory-form-error').textContent,history:document.querySelector('#history-notice').textContent,tail:document.querySelector('#timeline').innerText.slice(-1000)})").catch(()=>''));await shot('failure').catch(()=>{});}await writeFile(join(report,'result.json'),JSON.stringify({passed:false,checks,error:String(e),errors},null,2));process.exitCode=1;}
finally{
  if(socket?.readyState===WebSocket.OPEN){await cdp('Browser.close').catch(()=>{});socket.close();}
  for(const p of pending.values()){clearTimeout(p.timer);p.reject(Error('test cleanup'));}pending.clear();
  await stop(browser);await stop(host);ui?.closeAllConnections?.();if(ui)await new Promise(r=>ui.close(r));fixture.closeAllConnections();await new Promise(r=>fixture.close(r));
  await rm(scratch,{recursive:true,force:true,maxRetries:8,retryDelay:200});
}
