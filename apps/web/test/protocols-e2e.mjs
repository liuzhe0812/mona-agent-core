// Formal browser + real Rust host/tools; isolated protocol fixtures, no user's secrets or state.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, mkdir, writeFile, readFile, readdir, rm, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startUiServer } from '../../../scripts/dev-web.mjs';
const root = fileURLToPath(new URL('../../../', import.meta.url));
const report = resolve(process.env.MONA_TEST_REPORT_DIR || join(root, 'target/browser-reports/protocols'));
const scratch = await mkdtemp(join(tmpdir(), 'mona-native-protocols-'));
const workspace = join(scratch, 'workspace'), state = join(scratch, 'state');
await mkdir(workspace); await mkdir(state); await mkdir(report, { recursive:true });
await writeFile(join(workspace, 'fixture.txt'), 'NATIVE_TOOL_RESULT_31415\n');
await writeFile(join(workspace, 'agent.toml'), 'version = 1\n');
const token = 'isolated-protocol-test-host-token-123456789';
const key = 'isolated-provider-key-not-real';
const calls = [], checks = [], errors = [], catalogs = [];
let sequence = 0, host, browser, socket, ui, hostLog = '';
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
function check(name, condition) { assert.ok(condition, name); checks.push(name); console.log(`PASS ${name}`); }
async function waitFor(fn, label, timeout = 30000) { const end = Date.now()+timeout; while(Date.now()<end) { if(await fn())return; await sleep(60); } throw new Error(`Timed out: ${label}`); }
async function stop(child) { if(!child || child.exitCode!=null)return; const exited=once(child,'exit');child.kill('SIGKILL');await Promise.race([exited,sleep(5000)]); }
function blocksText(content) { return typeof content==='string' ? content : (content||[]).filter(b=>b.type==='input_text'||b.type==='text').map(b=>b.text).join('\n'); }
function inspect(protocol, body) {
  const messages = protocol==='responses' ? body.input : body.messages;
  const users = messages.filter(m=>m.role==='user').flatMap(m=>typeof m.content==='string'?[m.content]:(m.content||[]).filter(b=>b.type==='input_text'||b.type==='text').map(b=>b.text)).filter(s=>s&&!s.startsWith('[Host context source:')&&!s.startsWith('[Earlier conversation summary'));
  const system = protocol==='responses' ? messages.filter(m=>m.role==='system').map(m=>blocksText(m.content)).join('\n') : blocksText(body.system);
  const last = messages.at(-1);
  const result = protocol==='responses' ? last?.type==='function_call_output' : last?.role==='user' && last?.content?.at(-1)?.type==='tool_result';
  return { latest:users.at(-1)||'', summary:system.startsWith('Summarize the earlier'), result, messages };
}
function streamResponse(res, protocol, id, { text='', call=null, privateState=false, slow=false }) {
  const inputTokens = Math.ceil(Buffer.byteLength(JSON.stringify(calls.at(-1).body))/3);
  res.writeHead(200, {'content-type':'text/event-stream','cache-control':'no-store'});
  const emit = value => res.write(`event: ${value.type}\ndata: ${JSON.stringify(value)}\n\n`);
  if(protocol==='responses') {
    emit({type:'response.created',response:{id:`resp_${id}`,status:'in_progress',output:[]}});
    const output=[];
    if(privateState) {
      emit({type:'response.output_item.added',output_index:0,item:{type:'reasoning',id:`rs_${id}`,summary:[]}});
      const item={type:'reasoning',id:`rs_${id}`,summary:[{type:'summary_text',text:'PRIVATE_NATIVE_REASONING'}],encrypted_content:'PRIVATE_NATIVE_SIGNATURE'};
      emit({type:'response.output_item.done',output_index:0,item});output.push(item);
    }
    const index=output.length;
    let item;
    if(call) {
      const start={type:'function_call',id:`fc_${id}`,call_id:`call_${id}`,name:call.name,arguments:'',status:'in_progress'};
      emit({type:'response.output_item.added',output_index:index,item:start});
      const args=JSON.stringify(call.arguments), mid=Math.floor(args.length/2);
      for(const delta of [args.slice(0,mid),args.slice(mid)])emit({type:'response.function_call_arguments.delta',output_index:index,item_id:start.id,delta});
      item={...start,arguments:args,status:'completed'};
    } else {
      const start={type:'message',id:`msg_${id}`,role:'assistant',content:[],status:'in_progress'};
      emit({type:'response.output_item.added',output_index:index,item:start});
      for(const delta of [text.slice(0,2),text.slice(2)].filter(Boolean))emit({type:'response.output_text.delta',output_index:index,item_id:start.id,content_index:0,delta});
      if(slow)return;
      item={...start,content:[{type:'output_text',text,annotations:[]}],status:'completed'};
    }
    emit({type:'response.output_item.done',output_index:index,item});output.push(item);
    emit({type:'response.completed',response:{id:`resp_${id}`,status:'completed',output,usage:{input_tokens:inputTokens,output_tokens:12}}});
  } else {
    emit({type:'message_start',message:{type:'message',id:`msg_${id}`,role:'assistant',model:'messages-fixture',content:[],usage:{input_tokens:inputTokens,cache_read_input_tokens:0,output_tokens:1}}});
    let index=0;
    if(privateState) {
      emit({type:'content_block_start',index,content_block:{type:'thinking',thinking:''}});
      emit({type:'content_block_delta',index,delta:{type:'thinking_delta',thinking:'PRIVATE_NATIVE_REASONING'}});
      emit({type:'content_block_delta',index,delta:{type:'signature_delta',signature:'PRIVATE_NATIVE_SIGNATURE'}});
      emit({type:'content_block_stop',index});index++;
    }
    if(call) {
      emit({type:'content_block_start',index,content_block:{type:'tool_use',id:`call_${id}`,name:call.name,input:{}}});
      const args=JSON.stringify(call.arguments), mid=Math.floor(args.length/2);
      for(const partial_json of [args.slice(0,mid),args.slice(mid)])emit({type:'content_block_delta',index,delta:{type:'input_json_delta',partial_json}});
    } else {
      emit({type:'content_block_start',index,content_block:{type:'text',text:''}});
      for(const part of [text.slice(0,2),text.slice(2)].filter(Boolean))emit({type:'content_block_delta',index,delta:{type:'text_delta',text:part}});
      if(slow)return;
    }
    emit({type:'content_block_stop',index});
    emit({type:'message_delta',delta:{stop_reason:call?'tool_use':'end_turn',stop_sequence:null},usage:{output_tokens:12}});
    emit({type:'message_stop'});
  }
  res.end();
}
const fixture = createServer(async(req,res)=>{
  try {
    const protocol=req.url.startsWith('/responses-api/')?'responses':'messages';
    assert.equal(req.headers[protocol==='responses'?'authorization':'x-api-key'],protocol==='responses'?`Bearer ${key}`:key);
    if(protocol==='messages')assert.equal(req.headers['anthropic-version'],'2023-06-01');
    if(req.method==='GET') { assert.ok(req.url.includes('/models'));catalogs.push(protocol);res.setHeader('content-type','application/json');res.end(JSON.stringify({data:[{id:`${protocol}-fixture`}],has_more:false}));return; }
    assert.equal(req.url,`/${protocol}-api/${protocol}`);
    let raw='';for await(const chunk of req){raw+=chunk;if(raw.length>4*1024*1024)throw new Error('fixture request cap');}
    const body=JSON.parse(raw), info=inspect(protocol,body);calls.push({protocol,body,summary:info.summary});
    assert.equal(body.stream,true);assert.ok(!raw.includes('"route"'));assert.ok(!raw.includes('"namespace"'));
    if(protocol==='responses'){assert.equal(body.store,false);assert.ok(!body.previous_response_id&&!body.conversation&&!body.background);}
    const id=++sequence;
    if(info.summary) {
      assert.ok(!body.tools?.length);
      streamResponse(res,protocol,id,{text:JSON.stringify({goal:'Continue the task',constraints:['Do not replay completed actions'],corrections:[],decisions:['decision 31415'],completed:['fixture read confirmed'],pending:['Continue requested work'],references:['fixture.txt']})});
    } else if(info.latest.startsWith('SLOW')) { streamResponse(res,protocol,id,{text:'STREAM_STARTED',slow:true}); }
    else if(info.latest.startsWith('READ_FIXTURE')&&!info.result) { streamResponse(res,protocol,id,{call:{name:'read',arguments:{path:'fixture.txt'}}}); }
    else if(info.result) { assert.ok(raw.includes('NATIVE_TOOL_RESULT_31415'));streamResponse(res,protocol,id,{text:'READ_CONFIRMED'}); }
    else { streamResponse(res,protocol,id,{text:`ACK ${info.latest.slice(0,32)}`,privateState:info.latest.startsWith('PRIVATE')}); }
  } catch(e) { errors.push(e.message); res.destroy(); }
});
await new Promise(resolve=>fixture.listen(0,'127.0.0.1',resolve));
const fixtureBase=`http://127.0.0.1:${fixture.address().port}`;
async function unusedPort(){const s=createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const p=s.address().port;await new Promise(r=>s.close(r));return p;}
const apiPort=await unusedPort(),endpoint=`http://127.0.0.1:${apiPort}`;
async function api(path){const r=await fetch(endpoint+path,{headers:{Authorization:`Bearer ${token}`},cache:'no-store',signal:AbortSignal.timeout(10000)});if(!r.ok)throw new Error(`API ${path} ${r.status}`);return r.json();}
async function startHost(origin){
  const env=Object.fromEntries(Object.entries(process.env).filter(([k])=>!k.startsWith('AGENT_')));
  Object.assign(env,{AGENT_SERVER_ADDR:`127.0.0.1:${apiPort}`,AGENT_SERVER_TOKEN:token,AGENT_UI_ORIGIN:origin,AGENT_ALLOW_HTTP_LOOPBACK:'1',AGENT_MODEL_STORE_KEY:'isolated-native-storage-key-not-real',AGENT_MODEL_SETTINGS_PATH:join(state,'models.enc'),AGENT_CAPABILITY_STATE_PATH:join(state,'capabilities.json'),AGENT_SESSIONS_DIR:join(state,'sessions'),AGENT_SPILL_DIR:join(state,'spill'),AGENT_WORKSPACE_DIR:workspace,USERPROFILE:join(state,'user'),HOME:join(state,'user'),LOCALAPPDATA:state,XDG_STATE_HOME:state});
  const binary=resolve(process.env.MONA_TEST_SERVER||join(root,'target/context-validation/debug',process.platform==='win32'?'server.exe':'server'));await access(binary);
  host=spawn(binary,['--config',join(workspace,'agent.toml')],{cwd:scratch,env,stdio:['ignore','pipe','pipe']});host.stderr.on('data',chunk=>{hostLog=(hostLog+chunk).slice(-16000);});host.on('error',e=>errors.push(e.message));
  await waitFor(async()=>{if(host.exitCode!=null)throw new Error(`host exited: ${hostLog}`);try{return(await api('/v1/info')).protocol_version===2;}catch{return false;}},'host ready');
}
let next=0;const pending=new Map();
function cdp(method,params={}){return new Promise((resolve,reject)=>{const id=++next;const timer=setTimeout(()=>{pending.delete(id);reject(new Error(`CDP ${method} timeout`));},20000);pending.set(id,{resolve,reject,timer});socket.send(JSON.stringify({id,method,params}));});}
async function evaluate(expression){const r=await cdp('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(r.exceptionDetails)throw new Error(r.exceptionDetails.exception?.description||'page script failed');return r.result.value;}
const waitPage=(e,label)=>waitFor(()=>evaluate(e),label);
const click=selector=>evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
const fill=(selector,value)=>evaluate(`(()=>{const n=document.querySelector(${JSON.stringify(selector)});n.value=${JSON.stringify(value)};n.dispatchEvent(new Event('input',{bubbles:true}));n.dispatchEvent(new Event('change',{bubbles:true}));})()`);
async function screenshot(name){const {data}=await cdp('Page.captureScreenshot',{format:'png'});await writeFile(join(report,`${name}.png`),Buffer.from(data,'base64'));}
async function submit(value,count){
  await fill('#prompt',value);await waitPage("!document.querySelector('#send').disabled",'composer ready');await click('#send');
  await waitFor(async()=>{const entries=(await api('/api/sessions')).sessions;const entry=entries[0];if(entry&&['failed','limited'].includes(entry.status)){throw new Error(`turn failed: ${JSON.stringify(entry)} ${await evaluate('document.querySelector("#timeline").innerText.slice(-1000)')}`);}return entry?.turn_count===count&&entry.status==='completed';},`turn ${count}`);
  await waitPage("document.querySelector('#cancel').hidden && !document.querySelector('#sessions-refresh').disabled",'turn displayed');
}
async function select(protocol){
  await click('#model-label');
  await waitPage("document.querySelector('#model-picker-dialog').open",'model picker');
  await evaluate(`Array.from(document.querySelectorAll('.model-picker-row')).find(n=>n.querySelector('strong')?.textContent===${JSON.stringify(`${protocol}-fixture`)}).click()`);
  await waitFor(async()=> (await api('/api/model-settings')).default?.model_id===`${protocol}-fixture`,'default changed');
  await waitPage("!document.querySelector('#model-picker-dialog').open",'picker closed');
}
try{
  ui=await startUiServer({host:'127.0.0.1',port:0,endpoint,token});const origin=`http://127.0.0.1:${ui.address().port}`;await startHost(origin);
  check('clean host starts without pre-existing model or session data',(await api('/api/model-settings')).providers.length===0&&(await api('/api/sessions')).sessions.length===0);
  const debugPort=await unusedPort();const executable=process.env.BROWSER_BIN||(process.platform==='win32'?'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe':'chromium');
  browser=spawn(executable,['--headless=new','--disable-gpu','--no-first-run','--no-default-browser-check','--edge-skip-compat-layer-relaunch',`--remote-debugging-port=${debugPort}`,`--user-data-dir=${join(scratch,'browser')}`,'--window-size=1440,1000','about:blank'],{stdio:'ignore'});browser.on('error',e=>errors.push(e.message));
  let page;await waitFor(async()=>{try{page=(await(await fetch(`http://127.0.0.1:${debugPort}/json/list`,{signal:AbortSignal.timeout(800)})).json()).find(p=>p.type==='page');return !!page;}catch{return false;}},'browser ready');
  socket=new WebSocket(page.webSocketDebuggerUrl);await once(socket,'open');socket.addEventListener('message',event=>{const value=JSON.parse(event.data),p=pending.get(value.id);if(p){pending.delete(value.id);clearTimeout(p.timer);value.error?p.reject(new Error(JSON.stringify(value.error))):p.resolve(value.result);}if(value.method==='Runtime.exceptionThrown')errors.push(value.params.exceptionDetails.exception?.description||'browser exception');});
  await cdp('Page.enable');await cdp('Runtime.enable');await cdp('Page.navigate',{url:origin});
  await waitPage("document.querySelector('#sessions-notice')?.textContent.includes('已保存在宿主本地')",'connected UI');
  for(const protocol of ['responses','messages']){
    await click('#settings-button');await click('#settings-models-tab');await click('#add-provider');
    await fill('#provider-name-input',`${protocol} test`);await fill('#provider-protocol-input',protocol);await fill('#provider-api-base-input',`${fixtureBase}/${protocol}-api`);await fill('#provider-key-input',key);
    await click('#provider-generation > summary');
    await fill('#provider-generation-input', protocol === 'responses' ? '{"reasoning":{"effort":"low"}}' : '{"thinking":{"type":"adaptive"}}');
    await click('#provider-discover');await waitPage("!document.querySelector('#provider-discover').disabled && document.querySelector('#provider-models-input').value.includes('fixture')",'native model discovery');
    await click('#provider-save');await waitPage("!document.querySelector('#provider-dialog').open",'provider saved');
    const provider=(await api('/api/model-settings')).providers.find(p=>p.protocol===protocol);
    check(`${protocol}: real settings preserve protocol and API Key without exposing it`,!!provider&&provider.has_key&&!JSON.stringify(provider).includes(key));
    await click(`[data-model-context="${protocol}-fixture"]`);await fill('#model-context-tokens','16384');
    await click('#model-capabilities > summary');await fill('[data-model-cap="tools"]','true');await fill('[data-model-cap="images"]','true');await fill('#model-max-output','8192');
    await cdp('Emulation.setDeviceMetricsOverride',{width:390,height:900,deviceScaleFactor:1,mobile:true});
    check(`${protocol}: capability editor stays inside the narrow viewport`,await evaluate("(()=>{const d=document.querySelector('#model-dialog'),r=d.getBoundingClientRect();return r.left>=0&&r.right<=innerWidth+1&&d.scrollWidth<=r.width+1;})()"));await screenshot(`${protocol}-settings-narrow`);
    await cdp('Emulation.setDeviceMetricsOverride',{width:1440,height:1000,deviceScaleFactor:1,mobile:false});await click('#model-save');await waitPage("!document.querySelector('#model-dialog').open",'capabilities saved');
    await click('#discover-models');await waitPage("!document.querySelector('#discover-models').disabled",'catalog refresh');
    const model=(await api('/api/model-settings')).providers.find(p=>p.protocol===protocol).models[0];
    check(`${protocol}: discovery keeps configured capabilities and model window`,model.capabilities.tools===true&&model.capabilities.images===true&&model.context_window_tokens===16384&&model.capabilities.max_output_tokens===8192);
    await click('#settings-back');
  }
  check('catalogs use the corresponding native authentication',catalogs.includes('responses')&&catalogs.includes('messages'));
  for(const protocol of ['responses','messages']){
    await select(protocol);await click('#new-chat');const start=calls.length;
    await submit('READ_FIXTURE once',1);
    check(`${protocol}: real Rust read executes and its result returns to the native model`,calls.slice(start).filter(c=>!c.summary).length===2&&JSON.stringify(calls.at(-1).body).includes('NATIVE_TOOL_RESULT_31415'));
    await submit('LONG_ONE decision 31415 '+ 'a'.repeat(18000),2);await submit('LONG_TWO '+ 'b'.repeat(14000),3);
    const summaries=calls.slice(start).filter(c=>c.summary);
    check(`${protocol}: compaction uses the same protocol and selected model`,summaries.length>0&&summaries.every(c=>c.protocol===protocol&&c.body.model===`${protocol}-fixture`));
    check(`${protocol}: saved reasoning configuration reaches both main and summary calls`, calls.slice(start).every(c=>protocol==='responses' ? c.body.reasoning?.effort==='low' : c.body.thinking?.type==='adaptive'));
    const id=(await api('/api/sessions')).sessions[0].id;
    await stop(host);await startHost(origin);await cdp('Page.reload',{ignoreCache:true});await waitPage("document.querySelectorAll('#timeline .turn').length===3 && !document.querySelector('#sessions-refresh').disabled",'history restored');
    await submit('AFTER_RESTART',4);
    check(`${protocol}: summary survives actual host restart without repeat work`,calls.slice(start).filter(c=>c.summary).length===summaries.length&&JSON.stringify(calls.at(-1).body).includes('Earlier conversation summary'));
    await submit('PRIVATE keep route identity',5);
    const before=await api(`/api/sessions/${id}`),callCount=calls.length;
    await select(protocol==='responses'?'messages':'responses');await fill('#prompt','BLOCK_INCOMPATIBLE');await click('#send');
    await waitPage("document.querySelector('#timeline').innerText.includes('请新建会话')",'cross-protocol rejection');
    const after=await api(`/api/sessions/${id}`);
    check(`${protocol}: cross-protocol private history is rejected before saving or calling a model`,calls.length===callCount&&after.session.revision===before.session.revision&&after.turns.length===before.turns.length);
    check(`${protocol}: private reasoning and signatures stay out of the browser`,!(await evaluate('document.querySelector("#timeline").innerText')).includes('PRIVATE_NATIVE_'));
    await select(protocol);await stop(host);await startHost(origin);await cdp('Page.reload',{ignoreCache:true});await waitPage("document.querySelectorAll('#timeline .turn').length===5 && !document.querySelector('#sessions-refresh').disabled",'private history restored');
    await submit('REPLAY_SAME_ROUTE',6);
    check(`${protocol}: exact private protocol state can be replayed after process restart`,JSON.stringify(calls.at(-1).body).includes('PRIVATE_NATIVE_SIGNATURE'));
    await screenshot(`${protocol}-resumed`);
    await click('#new-chat');await fill('#prompt','SLOW cancel this run');await click('#send');await waitPage("document.querySelector('#timeline').innerText.includes('STREAM_STARTED')",'partial output');await click('#cancel');
    await waitFor(async()=> (await api('/api/sessions')).sessions[0]?.status==='cancelled','cancel settled');await waitPage("document.querySelector('#cancel').hidden",'cancel visible');
    check(`${protocol}: cancellation settles the real run without pretending partial output completed`,true);
  }
  await select('responses');await click('#new-chat');await submit('READ_FIXTURE ordinary cross-protocol task',1);
  await select('messages');await submit('CONTINUE_ORDINARY_HISTORY',2);
  check('ordinary history with complete local tool results can cross native protocols', calls.at(-1).protocol==='messages' && JSON.stringify(calls.at(-1).body).includes('NATIVE_TOOL_RESULT_31415'));
  check('no browser or endpoint fixture exceptions',errors.length===0);
  await writeFile(join(report,'result.json'),JSON.stringify({passed:true,checks,model_requests:calls.length,summary_calls:calls.filter(c=>c.summary).length,real_provider:false,isolated:true},null,2));
  console.log(`PASS ${checks.length} native protocol end-to-end assertions`);
}catch(error){
  console.error(error);console.error(hostLog);if(socket?.readyState===WebSocket.OPEN){console.error(await evaluate("JSON.stringify({url:location.href,provider:document.querySelector('#provider-form-error').textContent,model:document.querySelector('#model-form-error').textContent,notice:document.querySelector('#model-settings-notice').textContent,tail:document.querySelector('#timeline').innerText.slice(-800)})").catch(()=>''));await screenshot('failure').catch(()=>{});}
  await writeFile(join(report,'result.json'),JSON.stringify({passed:false,checks,error:String(error),errors},null,2));process.exitCode=1;
}finally{
  if(socket?.readyState===WebSocket.OPEN){await cdp('Browser.close').catch(()=>{});socket.close();}
  for(const p of pending.values()){clearTimeout(p.timer);p.reject(new Error('test cleanup'));}pending.clear();
  await stop(browser);await stop(host);ui?.closeAllConnections?.();if(ui)await new Promise(r=>ui.close(r));fixture.closeAllConnections();await new Promise(r=>fixture.close(r));await rm(scratch,{recursive:true,force:true,maxRetries:10,retryDelay:200});
}
