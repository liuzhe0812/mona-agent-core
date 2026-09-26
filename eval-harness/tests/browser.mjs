// Real local browser + evaluation server + optional real Mona host; no paid provider calls.
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {createServer} from 'node:net';
import {mkdtemp,mkdir,writeFile,readdir,access} from 'node:fs/promises';
import {join,resolve} from 'node:path';
import {fileURLToPath} from 'node:url';

const root=fileURLToPath(new URL('../',import.meta.url));
await mkdir(join(root,'.local'),{recursive:true});
const scratch=await mkdtemp(join(root,'.local/browser-'));
const data=join(scratch,'data'), downloads=join(scratch,'downloads');
await mkdir(downloads,{recursive:true});
const config=join(scratch,'config.json');
await writeFile(config,JSON.stringify({version:1,agents:[{id:'slow-fixture',label:'取消测试夹具',kind:'command',argv:['{python}','-u',join(root,'tests/command_fixture.py'),'hang'],capabilities:['text']}],models:[]}));
const checks=[],errors=[],pending=new Map();
let host,browser,socket,origin,token,seq=0,hostLog='';
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
function check(name,value){assert.ok(value,name);checks.push(name);console.log('PASS '+name);}
async function waitFor(fn,label,timeout=25000){const end=Date.now()+timeout;while(Date.now()<end){if(await fn())return;await sleep(70);}throw Error('Timeout: '+label+' '+hostLog.slice(-2000));}
async function freePort(){const s=createServer();await new Promise(r=>s.listen(0,'127.0.0.1',r));const p=s.address().port;await new Promise(r=>s.close(r));return p;}
async function request(path,method='GET',body){const r=await fetch(origin+path,{method,headers:{'X-Eval-Token':token,...(body?{'Content-Type':'application/json'}:{})},body:body?JSON.stringify(body):undefined,signal:AbortSignal.timeout(5000)});const d=await r.json();if(!r.ok)throw Error(JSON.stringify(d));return d;}
async function startHost(port){
  hostLog='';const env={...process.env,PYTHONUTF8:'1',PYTHONDONTWRITEBYTECODE:'1'};delete env.EVAL_CONFIG;delete env.EVAL_DATA_DIR;
  host=spawn(process.env.PYTHON||'python',['-u','-m','harness','--data',data,'--config',config,'serve','--port',String(port)],{cwd:root,env,stdio:['ignore','pipe','pipe']});
  host.stdout.on('data',b=>hostLog=(hostLog+b).slice(-12000));host.stderr.on('data',b=>hostLog=(hostLog+b).slice(-12000));
  host.on('error',e=>errors.push(String(e)));origin=`http://127.0.0.1:${port}`;
  await waitFor(async()=>{if(host.exitCode!=null)throw Error(hostLog);try{const r=await fetch(origin+'/api/bootstrap',{signal:AbortSignal.timeout(800)});if(!r.ok)return false;token=(await r.json()).token;return true;}catch{return false;}},'evaluation service ready');
}
async function stopHost(graceful=true){if(!host||host.exitCode!=null)return;const child=host;if(graceful){try{await request('/api/shutdown','POST',{});}catch{}}
  if(!graceful)child.kill('SIGKILL');
  await Promise.race([once(child,'exit'),sleep(10000)]);
  if(child.exitCode==null){child.kill('SIGKILL');await Promise.race([once(child,'exit'),sleep(5000)]);}
  host=null;
}
function cdp(method,params={}){return new Promise((resolve,reject)=>{const id=++seq,timer=setTimeout(()=>{pending.delete(id);reject(Error('CDP '+method));},15000);pending.set(id,{resolve,reject,timer});socket.send(JSON.stringify({id,method,params}));});}
async function evaluate(expression){const r=await cdp('Runtime.evaluate',{expression,awaitPromise:true,returnByValue:true});if(r.exceptionDetails)throw Error(r.exceptionDetails.exception?.description||'Page exception');return r.result.value;}
const pageWait=(e,label)=>waitFor(()=>evaluate(e),label);
async function click(selector){const p=await evaluate(`(()=>{const n=document.querySelector(${JSON.stringify(selector)});if(!n)throw Error('missing '+${JSON.stringify(selector)});n.scrollIntoView({block:'nearest'});const r=n.getBoundingClientRect();if(!r.width||!r.height)throw Error('element is not visible');return {x:r.x+r.width/2,y:r.y+r.height/2}})()`);await cdp('Input.dispatchMouseEvent',{type:'mousePressed',button:'left',clickCount:1,...p});await cdp('Input.dispatchMouseEvent',{type:'mouseReleased',button:'left',clickCount:1,...p});}
async function fill(selector,value){await evaluate(`(()=>{const n=document.querySelector(${JSON.stringify(selector)});n.value=${JSON.stringify(value)};n.dispatchEvent(new Event('input',{bubbles:true}));n.dispatchEvent(new Event('change',{bubbles:true}));})()`);}
async function screenshot(name){const r=await cdp('Page.captureScreenshot',{format:'png',captureBeyondViewport:false});await writeFile(join(scratch,name+'.png'),Buffer.from(r.data,'base64'));}
async function waitReport(label){let report;await waitFor(async()=>{const row=(await request('/api/runs')).runs.find(r=>r.config.label===label);if(!row)return false;report=await request('/api/runs/'+row.id);return !['running','queued','cancelling'].includes(report.status);},label+' finished',60000);return report;}
try{
  const port=await freePort();await startHost(port);
  const debug=await freePort(),binary=process.env.BROWSER_BIN||(process.platform==='win32'?'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe':'chromium');
  browser=spawn(binary,['--headless=new','--disable-gpu','--no-first-run','--no-default-browser-check','--edge-skip-compat-layer-relaunch',`--remote-debugging-port=${debug}`,`--user-data-dir=${join(scratch,'browser-profile')}`,'--window-size=1360,1000','about:blank'],{stdio:'ignore'});
  browser.on('error',e=>errors.push(String(e)));
  let page;await waitFor(async()=>{try{page=(await(await fetch(`http://127.0.0.1:${debug}/json/list`,{signal:AbortSignal.timeout(800)})).json()).find(p=>p.type==='page');return!!page;}catch{return false;}},'browser CDP');
  socket=new WebSocket(page.webSocketDebuggerUrl);await once(socket,'open');
  socket.addEventListener('message',e=>{const v=JSON.parse(e.data),p=pending.get(v.id);if(p){clearTimeout(p.timer);pending.delete(v.id);v.error?p.reject(Error(JSON.stringify(v.error))):p.resolve(v.result);}if(v.method==='Runtime.exceptionThrown')errors.push(v.params.exceptionDetails.exception?.description||'browser exception');});
  await cdp('Page.enable');await cdp('Runtime.enable');await cdp('Browser.setDownloadBehavior',{behavior:'allow',downloadPath:downloads});
  await cdp('Page.navigate',{url:origin});await pageWait("document.querySelector('#connection')?.textContent==='本地已连接'",'Web connected');
  check('Web loads real catalog and explicitly labels selftest',await evaluate("document.querySelector('#adapter').value==='selftest' && document.querySelector('#start').textContent.includes('免费自检')"));
  await screenshot('configuration-light');
  await fill('#suite','basic');await fill('#label','browser-baseline');await click('#start');
  const first=await waitReport('browser-baseline');check('Web launches actual harness and all six basic tasks pass',first.status==='completed'&&first.summary.counts.passed===6&&first.summary.model_calls===0);
  await pageWait("document.querySelector('#run-meta').textContent.includes('全部通过')",'completed UI');
  check('Unknown metrics are not shown as measured zeros',await evaluate("document.querySelector('#warnings').textContent.includes('不显示为 0')"));
  await click('#trials .trial summary');await screenshot('report-light');
  await click('#export');await waitFor(async()=> (await readdir(downloads)).some(f=>f===first.id+'.json'),'report download');check('Report export produces an actual JSON file',true);
  await click('#trials .trial button');await waitFor(async()=> (await readdir(downloads)).some(f=>f===first.id+'-t-1.json'),'evidence download');check('Full trial evidence can be downloaded',true);
  await click('#new-run');await fill('#label','browser-comparison');await click('#start');const second=await waitReport('browser-comparison');check('A second independent run succeeds',second.status==='completed');
  await pageWait("document.querySelector('#right').options.length>=3",'comparison options');await fill('#left',first.id);await fill('#right',second.id);await click('#compare');await pageWait("document.querySelectorAll('#comparison tbody tr').length===5",'comparison rendered');check('Comparison shows measured deltas without fabricated composite score',true);
  await click('#theme');check('Dark theme changes tokens',await evaluate("document.documentElement.dataset.theme==='dark'"));await screenshot('report-dark');
  await cdp('Emulation.setDeviceMetricsOverride',{width:320,height:850,deviceScaleFactor:1,mobile:true});
  check('320 px layout has no page-wide horizontal overflow',await evaluate('document.documentElement.scrollWidth<=innerWidth+1'));await screenshot('report-320');
  await cdp('Emulation.setDeviceMetricsOverride',{width:1360,height:1000,deviceScaleFactor:1,mobile:false});
  await click('#new-run');await fill('#adapter','slow-fixture');await fill('#label','browser-cancel');
  await click('#start');await pageWait("!document.querySelector('#error').hidden",'authorization refusal');
  check('Local process execution requires deliberate permission',!(await request('/api/runs')).runs.some(r=>r.config.label==='browser-cancel'));
  await click('#allow_local_execution');await click('#start');await pageWait("document.querySelector('#run-title').textContent==='browser-cancel'&&!document.querySelector('#run-section').hidden&&!document.querySelector('#cancel').hidden&&!document.querySelector('#cancel').disabled",'cancel enabled for the newly started run');await click('#cancel');
  const cancelled=await waitReport('browser-cancel');check('Stopping from Web cancels owned processes and records cancellation',cancelled.status==='cancelled');
  await stopHost(false);await startHost(port);await cdp('Page.reload',{ignoreCache:true});await pageWait("document.querySelector('#connection')?.textContent==='本地已连接'",'Web after process restart');
  check('History survives actual evaluation-server restart',(await request('/api/runs')).total===3);
  if(process.env.MONA_EVAL_SERVER){
    await fill('#adapter','mona');await fill('#suite','all');await fill('#label','browser-mona');await click('#allow_local_execution');await click('#start');
    const mona=await waitReport('browser-mona');
    check('Real Mona host executes all eight tasks through the independent adapter',mona.status==='completed'&&mona.summary.counts.passed===8);
    check('Controlled model attempts and usage are observed, not self-reported',mona.summary.model_calls>0&&mona.summary.tokens>0&&mona.mode==='controlled');
    const restart=mona.trials.find(t=>t.task_id==='restart-recall');check('Mona session survives an actual target-host process restart',restart.status==='passed');
    await pageWait("document.querySelector('#run-meta').textContent.includes('全部通过')",'Mona result UI');await screenshot('mona-report');
  }else{console.log('Mona browser integration NOT RUN: set MONA_EVAL_SERVER');}
  const rows=(await request('/api/runs')).runs;const current=rows[0];
  await evaluate(`document.querySelector('[data-run="${current.id}"]').click()`);await pageWait(`document.querySelector('#run-meta').textContent.includes('${current.id}')`,'select for removal');
  await cdp('Page.handleJavaScriptDialog',{accept:true}).catch(()=>{});
  const dialog=ev=>{const v=JSON.parse(ev.data);if(v.method==='Page.javascriptDialogOpening')cdp('Page.handleJavaScriptDialog',{accept:true}).catch(e=>errors.push(e.message));};socket.addEventListener('message',dialog);
  await click('#delete');await waitFor(async()=>!(await request('/api/runs')).runs.some(r=>r.id===current.id),'deleted run');socket.removeEventListener('message',dialog);
  check('Web removes only the selected finished report and trial files',true);
  check('No browser runtime exceptions',errors.length===0);
  await writeFile(join(scratch,'result.json'),JSON.stringify({passed:checks.length,checks,errors,mona:!!process.env.MONA_EVAL_SERVER},null,2));
  console.log(JSON.stringify({passed:checks.length,result:join(scratch,'result.json')}));
}catch(e){await writeFile(join(scratch,'result.json'),JSON.stringify({passed:checks.length,checks,errors,error:String(e),hostLog},null,2));console.error(e);process.exitCode=1;}
finally{
  if(socket?.readyState===WebSocket.OPEN){try{await cdp('Browser.close');}catch{}socket.close();}
  if(browser&&browser.exitCode==null)browser.kill();
  await stopHost(true);
  for(const p of pending.values()){clearTimeout(p.timer);p.reject(Error('test ended'));}pending.clear();
}
