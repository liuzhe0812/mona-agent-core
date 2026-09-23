// Real Rust host, real file tools, real browser. All directories, ports and processes belong to this test.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, mkdir, readFile, writeFile, rm, rename, access, symlink, open } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startUiServer } from '../../../scripts/dev-web.mjs';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const scratch = await mkdtemp(join(tmpdir(), 'mona-workspaces-e2e-'));
const state = join(scratch, 'state'), home = join(scratch, 'home'), projectOne = join(scratch, 'project-one'), projectTwo = join(scratch, 'project-two'), nextRoot = join(scratch, 'next-root');
for (const path of [state, home, projectOne, projectTwo, nextRoot]) await mkdir(path, { recursive: true });
const output = resolve(process.env.MONA_TEST_REPORT_DIR || join(root, '.tmp-verify/workspace-browser-report'));
await mkdir(output, { recursive: true });
const token = 'isolated-workspace-test-token-at-least-32-characters';
const checks = [], modelRequests = [], errors = [];
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const normalizePath = value => process.platform === 'win32' ? String(value).replace(/^\\\\\?\\/, '').replaceAll('\\', '/').toLowerCase() : String(value);
function check(name, ok) { assert.ok(ok, name); checks.push(name); console.log(`PASS ${name}`); }
async function waitFor(fn, label, timeout = 20000) { const end = Date.now() + timeout; while (Date.now() < end) { if (await fn()) return; await sleep(60); } throw new Error(`Timeout: ${label}`); }
async function stop(child) { if (!child || child.exitCode != null) return; const done = once(child, 'exit'); child.kill('SIGKILL'); await Promise.race([done, sleep(5000)]); }
async function freePort() { const server = createServer(); await new Promise(resolve => server.listen(0, '127.0.0.1', resolve)); const port = server.address().port; await new Promise(resolve => server.close(resolve)); return port; }
let host, browser, ui, socket, hostLog = '', browserPort;
const port = await freePort(), endpoint = `http://127.0.0.1:${port}`;
const binary = resolve(process.env.MONA_TEST_SERVER || join(root, 'target/debug', process.platform === 'win32' ? 'server.exe' : 'server'));
const noProjects = process.env.MONA_TEST_NO_PROJECTS === '1';
const fixture = createServer(async (req, res) => {
  try {
    let raw = ''; for await (const chunk of req) { raw += chunk; if (raw.length > 4 * 1024 * 1024) throw new Error('model request cap'); }
    const body = JSON.parse(raw); modelRequests.push(body);
    const latest = body.messages.filter(m => m.role === 'user').at(-1)?.content || '';
    res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-store' });
    const frame = value => res.write(`data: ${JSON.stringify(value)}\n\n`);
    if (body.messages.at(-1).role === 'user' && /^(WRITE|READ|SHELL):/.test(latest)) {
      const [op, ...parts] = latest.split(':'); const label = parts.join(':');
      const name = op === 'WRITE' ? 'write' : op === 'READ' ? 'read' : 'shell';
      const args = op === 'WRITE' ? { path: 'result.txt', content: label + '\n' } : op === 'READ' ? { path: 'result.txt' }
        : { command: process.platform === 'win32' ? '(Get-Location).Path' : 'pwd' };
      frame({ choices: [{ index: 0, delta: { tool_calls: [{ index: 0, id: `tool-${modelRequests.length}`, type: 'function', function: { name, arguments: JSON.stringify(args) } }] }, finish_reason: 'tool_calls' }] });
    } else frame({ choices: [{ index: 0, delta: { content: `已完成 ${latest}` }, finish_reason: 'stop' }] });
    frame({ choices: [], usage: { prompt_tokens: 30, completion_tokens: 10 } }); res.end('data: [DONE]\n\n');
  } catch (e) { errors.push(e.message); res.destroy(); }
});
await new Promise(resolve => fixture.listen(0, '127.0.0.1', resolve));
async function request(path, body, method = body == null ? 'GET' : 'POST', expected = 200, auth = true) {
  const response = await fetch(endpoint + path, { method, headers: { ...(auth ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json' }, body: body == null ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(15000) });
  const text = await response.text(); if (response.status !== expected) throw new Error(`${method} ${path}: expected ${expected}, got ${response.status}: ${text}`);
  return text ? JSON.parse(text) : null;
}
async function startHost(origin) {
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('AGENT_') && !key.startsWith('MONA_DEV_')));
  Object.assign(env, { MONA_DEV_STATE_DIR: state, USERPROFILE: home, HOME: home, AGENT_SERVER_ADDR: `127.0.0.1:${port}`, AGENT_SERVER_TOKEN: token,
    AGENT_UI_ORIGIN: origin, AGENT_MODEL_ENDPOINT: `http://127.0.0.1:${fixture.address().port}/chat/completions`, AGENT_MODEL_NAME: 'workspace-local-test',
    AGENT_ALLOW_HTTP_LOOPBACK: '1', AGENT_MODEL_MANAGEMENT: '0', AGENT_SKILLS: '0',
    AGENT_SPILL_DIR: join(state, 'spill'), AGENT_CAPABILITY_STATE_PATH: join(state, 'capabilities.json') });
  if (noProjects) env.AGENT_PROJECTS = '0';
  await access(binary); hostLog = ''; host = spawn(binary, [], { cwd: scratch, env, stdio: ['ignore', 'pipe', 'pipe'] });
  host.stderr.on('data', data => { hostLog = (hostLog + data).slice(-12000); });
  host.on('error', e => errors.push(e.message));
  await waitFor(async () => { if (host.exitCode != null) throw new Error(`host exited: ${hostLog}`); try { return (await request('/v1/info')).protocol_version === 2; } catch { return false; } }, 'host ready');
}
async function turn(header, prompt, key = crypto.randomUUID()) {
  const current = (await request(`/api/sessions/${header.id}`)).session;
  const start = await request(`/api/sessions/${header.id}/turns`, { request_id: key, revision: current.revision, prompt });
  await waitFor(async () => { const value = (await request(`/api/sessions/${header.id}`)).session; if (value.status === 'failed') throw new Error('task failed'); return value.status === 'completed'; }, 'persisted turn');
  return start;
}
let sequence = 0; const pending = new Map();
async function send(method, params = {}) { return new Promise((resolve, reject) => {
  const id = ++sequence; const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 15000);
  pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params }));
}); }
async function evaluate(expression) { const v = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }); if (v.exceptionDetails) throw new Error(v.exceptionDetails.exception?.description || 'page failed'); return v.result.value; }
const waitPage = (expression, name) => waitFor(() => evaluate(expression), name);
async function click(selector) {
  const point = await evaluate(`(() => {const n=document.querySelector(${JSON.stringify(selector)});if(!n)throw new Error('missing element');n.scrollIntoView({block:'nearest'});const r=n.getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2};})()`);
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...point });
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...point });
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, ...point });
}
async function fill(selector, value) { await evaluate(`(() => {const n=document.querySelector(${JSON.stringify(selector)});n.value=${JSON.stringify(value)};n.dispatchEvent(new Event('input',{bubbles:true}));})()`); }
async function shot(name) { const v = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false }); await writeFile(join(output, name + '.png'), Buffer.from(v.data, 'base64')); }
async function launchBrowser(origin, session) {
  browserPort = await freePort();
  const executable = process.env.BROWSER_BIN || (process.platform === 'win32' ? 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe' : 'chromium');
  browser = spawn(executable, ['--headless=new','--disable-gpu','--no-first-run','--no-default-browser-check',`--remote-debugging-port=${browserPort}`,`--user-data-dir=${join(scratch,'browser')}`,'--window-size=1440,1000','about:blank'], {stdio:'ignore'});
  browser.on('error', e => errors.push(e.message));
  await waitFor(async () => { try { return (await fetch(`http://127.0.0.1:${browserPort}/json/list`)).ok; } catch { return false; } }, 'browser ready');
  const pages = await (await fetch(`http://127.0.0.1:${browserPort}/json/list`)).json(); socket = new WebSocket(pages.find(p=>p.type==='page').webSocketDebuggerUrl); await once(socket,'open');
  socket.addEventListener('message', event => { const v=JSON.parse(event.data), p=pending.get(v.id); if(p){pending.delete(v.id);clearTimeout(p.timer);v.error?p.reject(new Error(JSON.stringify(v.error))):p.resolve(v.result);} if(v.method==='Runtime.exceptionThrown')errors.push(v.params.exceptionDetails.exception?.description || 'browser exception'); if(v.method==='Page.javascriptDialogOpening')void send('Page.handleJavaScriptDialog',{accept:true}); });
  await send('Page.enable');await send('Runtime.enable');await send('Emulation.setFocusEmulationEnabled',{enabled:true});
  await send('Emulation.setDeviceMetricsOverride',{width:1440,height:1000,deviceScaleFactor:1,mobile:false});
  await send('Page.navigate',{url:`${origin}/#session=${session}`});
  await waitPage("document.querySelector('#workspace-path')?.textContent.length>0 && !document.querySelector('#sessions-refresh').disabled",'workspace UI ready');
}
try {
  ui=await startUiServer({host:'127.0.0.1',port:0,endpoint,token});const origin=`http://127.0.0.1:${ui.address().port}`;
  await startHost(origin);
  const initial=await request('/api/workspace-settings');
  check('fresh host has a configurable dedicated default root, not the executable cwd',normalizePath(initial.default_root).endsWith('/home/.mona-agent/workspaces')&&!initial.locked);
  check('project availability reflects the host build/deployment',initial.projects_enabled===!noProjects);
  if(noProjects){
    const unavailable=await fetch(endpoint+'/api/projects',{headers:{Authorization:`Bearer ${token}`},signal:AbortSignal.timeout(5000)});
    check('base build omits project routes rather than merely hiding the menu',unavailable.status===404);
    await unavailable.body?.cancel();
  }
  await request('/api/workspace-settings',undefined,'GET',401,false);
  const a=await request('/api/sessions',{request_id:'ordinary-a'}), b=await request('/api/sessions',{request_id:'ordinary-b'});
  check('ordinary sessions allocate separate directories once and carry no project identity',a.workspace!==b.workspace&&a.workspace.endsWith('s-ordinary-a')&&!a.metadata['project.id']);
  const a1=await turn(a,'WRITE:ORDINARY_A');await turn(b,'WRITE:ORDINARY_B');
  check('real write tools use the pinned session cwd without overwriting another session',await readFile(join(a.workspace,'result.txt'),'utf8')==='ORDINARY_A\n'&&await readFile(join(b.workspace,'result.txt'),'utf8')==='ORDINARY_B\n');
  await turn(a,'SHELL:cwd');
  // Provider tool messages carry a JSON result envelope, not plain shell stdout.
  const shellResult = JSON.parse(modelRequests.at(-1).messages.filter(m=>m.role==='tool').at(-1).content);
  check('shell uses the same actual cwd as file tools', shellResult.status==='success'
    && normalizePath(shellResult.content.trim())===normalizePath(a.workspace));
  const beforeFiles=modelRequests.length;
  const list=await request(`/api/sessions/${a.id}/files?path=`);
  const page=await request(`/api/sessions/${a.id}/file?path=result.txt`);
  check('file browsing is authenticated and does not create model calls',page.text==='ORDINARY_A\n'&&list.entries.some(e=>e.name==='result.txt')&&modelRequests.length===beforeFiles);
  await request(`/api/sessions/${a.id}/file?path=..%2Fsecret`,undefined,'GET',400);
  await request(`/api/sessions/${a.id}/file?path=C%3A%2Fsecret`,undefined,'GET',400);
  await request(`/api/sessions/${a.id}/file?path=result.txt`,undefined,'GET',401,false);
  await writeFile(join(a.workspace,'result.txt'),'ORDINARY_A_CHANGED\n');
  await request(`/api/sessions/${a.id}/file?path=result.txt&revision=${page.revision}`,undefined,'GET',409);
  check('path escape, unauthorized read and mixed-version pagination are rejected',true);
  const outside=join(scratch,'outside');await mkdir(outside);await writeFile(join(outside,'keep.txt'),'not inside workspace');
  await symlink(outside,join(a.workspace,'external-link'),process.platform==='win32'?'junction':'dir');
  await request(`/api/sessions/${a.id}/file?path=external-link%2Fkeep.txt`,undefined,'GET',403);
  check('symlink or Windows junction targets cannot be opened by the file service',(await request(`/api/sessions/${a.id}/files?path=`)).entries.some(e=>e.name==='external-link'&&e.kind==='link'));
  const huge=await open(join(a.workspace,'too-large.bin'),'w');await huge.truncate(16*1024*1024+1);await huge.close();
  await request(`/api/sessions/${a.id}/file?path=too-large.bin`,undefined,'GET',413);
  await request(`/api/sessions/${a.id}/files?path=&limit=201`,undefined,'GET',400);
  await request(`/api/sessions/${a.id}/files?path=&revision=${list.revision}`,undefined,'GET',409);
  check('file capacity and directory-page limits fail explicitly instead of dropping data',true);
  const changed=await request('/api/workspace-settings',{revision:initial.revision,default_root:nextRoot},'PUT');
  await request('/api/workspace-settings',{revision:changed.revision,default_root:join(scratch,'absent')},'PUT',404);
  check('a rejected default path does not publish or persist a new settings revision',(await request('/api/workspace-settings')).revision===changed.revision);
  await request('/api/workspace-settings',{revision:initial.revision,default_root:projectOne},'PUT',409);
  const repeated=await request('/api/sessions',{request_id:'ordinary-a'}),c=await request('/api/sessions',{request_id:'ordinary-c'});
  check('default-root changes affect only new sessions; create retries keep the original binding',repeated.workspace===a.workspace&&normalizePath(c.workspace).startsWith(normalizePath(nextRoot)));
  let registered,projectSession,otherProject;
  if(!noProjects){
    let registry=await request('/api/projects');registry=await request('/api/projects',{request_id:'project-one',revision:registry.revision,name:'Project One',path:projectOne});registered=registry.projects[0];
    registry=await request('/api/projects',{request_id:'project-two',revision:registry.revision,name:'Project Two',path:projectTwo});otherProject=registry.projects.find(p=>p.name==='Project Two');
    await request('/api/projects',{request_id:'private',revision:registry.revision,name:'Private',path:state},'POST',403);
    projectSession=await request('/api/sessions',{request_id:'project-a',project_id:registered.id});const same=await request('/api/sessions',{request_id:'project-b',project_id:registered.id});
    check('project sessions share the explicitly registered cwd while histories remain independent',projectSession.workspace===same.workspace&&projectSession.id!==same.id);
    await turn(projectSession,'WRITE:PROJECT_ONE');check('project tool writes land in the project rather than the default root',await readFile(join(projectOne,'result.txt'),'utf8')==='PROJECT_ONE\n');
    const scoped=await request(`/api/sessions?project_id=${registered.id}`);const ordinary=await request('/api/sessions?project_id=');
    check('project and ordinary session lists are separate',scoped.sessions.length===2&&!ordinary.sessions.some(h=>h.id===projectSession.id));
    registry=await request(`/api/projects/${registered.id}/remove`,{revision:registry.revision});
    check('removing registration preserves project files and makes detached history reachable',await readFile(join(projectOne,'result.txt'),'utf8')==='PROJECT_ONE\n'&&(await request('/api/sessions?project_id=')).sessions.some(h=>h.id===projectSession.id));
    await turn(projectSession,'READ:still here');
    await request('/api/sessions',{request_id:'must-not-fallback',project_id:'p-missing'},'POST',404);
  } else await request('/api/sessions',{request_id:'reject-project',project_id:'p-missing'},'POST',400);
  const bHeader=(await request(`/api/sessions/${b.id}`)).session;await request(`/api/sessions/${b.id}/delete`,{revision:bHeader.revision},'POST',204);
  check('conversation deletion preserves user work files',await readFile(join(b.workspace,'result.txt'),'utf8')==='ORDINARY_B\n');
  await stop(host);await startHost(origin);
  check('restart preserves default-root settings and old session cwd',(await request('/api/workspace-settings')).revision===changed.revision&&(await request(`/api/sessions/${a.id}`)).session.workspace===a.workspace);
  await turn(a,'READ:after restart');
  check('real multi-turn continuation reads original files after a root change and restart',modelRequests.at(-1).messages.some(m=>m.role==='tool'&&String(m.content).includes('ORDINARY_A_CHANGED')));
  await mkdir(join(a.workspace,'docs'));await writeFile(join(a.workspace,'docs','example.rs'),'fn main() { println!("workspace"); }\n');
  await writeFile(join(a.workspace,'long.txt'),'分页内容\n'.repeat(12000));
  const png='iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=';
  await writeFile(join(a.workspace,'pixel.png'),Buffer.from(png,'base64'));
  if(process.env.MONA_TEST_SKIP_BROWSER!=='1'){
    await launchBrowser(origin,a.id);
    check('right file panel follows the selected persisted session',await evaluate(`document.querySelector('#workspace-path').textContent===${JSON.stringify(a.workspace)}`));
    await waitPage("document.querySelector('.file-entry[data-path=\"result.txt\"]')",'files listed');await click('.file-entry[data-path="result.txt"]');
    await waitPage("document.querySelector('#file-content').textContent.includes('ORDINARY_A_CHANGED')",'text preview');
    check('real file preview appears without sending another Agent task',await evaluate("document.querySelector('.workspace-file-text')!==null"));
    await shot('workspace-light');
    await click('.file-entry[data-path="docs"]');await waitPage("document.querySelector('.file-entry[data-path=\"docs/example.rs\"]')",'nested directory');
    await click('.file-entry[data-path="docs/example.rs"]');await waitPage("document.querySelector('#file-content').textContent.includes('fn main()')",'code preview');
    await click('#files-parent');await waitPage("document.querySelector('.file-entry[data-path=\"pixel.png\"]')",'root directory');await click('.file-entry[data-path="pixel.png"]');
    await waitPage("document.querySelector('#file-content img')?.naturalWidth>0",'image decoded');
    check('directory navigation and raster preview work through host authorization',true);
    await click('.file-entry[data-path="long.txt"]');await waitPage("!document.querySelector('#file-more').hidden",'long file page');
    const textLength=await evaluate("document.querySelector('#file-content').textContent.length");await click('#file-more');
    await waitPage(`document.querySelector('#file-content').textContent.length>${textLength}`,'next text page');check('long files are paged instead of loading an unlimited response',true);
    await click('#settings-button');await click('#settings-workspace-tab');
    await waitPage(`!document.querySelector('#workspace-root').disabled && document.querySelector('#workspace-root').value===${JSON.stringify(changed.default_root)}`,'workspace settings');
    await fill('#workspace-root',projectTwo);await click('#workspace-root-save');await waitPage("document.querySelector('#workspace-notice').textContent.includes('已保存')",'root saved from real form');
    check('default path is configurable from the Web UI without restart',normalizePath((await request('/api/workspace-settings')).default_root)===normalizePath(projectTwo));
    await click('#settings-appearance-tab');await click('label.theme-mode:has(#theme-mode-dark)');await waitPage("document.documentElement.dataset.theme==='dark'",'dark theme applied');await click('#settings-back');await shot('workspace-dark');
    await click('#files-close');check('right pane can be collapsed without changing the session',await evaluate("document.querySelector('#files-panel').hidden && location.hash.includes('ordinary-a')"));
    if(!noProjects){
      await click('#project-add');await fill('#project-path',join(a.workspace,'docs'));await fill('#project-name','UI Project');await click('#project-save');
      await waitPage("!document.querySelector('#project-dialog').open && document.querySelector('#project-list').textContent.includes('UI Project')",'project UI registration');
      check('project registration has a real persistent UI path', (await request('/api/projects')).projects.some(p=>p.name==='UI Project'));
      const id=(await request('/api/projects')).projects.find(p=>p.name==='UI Project').id;
      await click(`.project-item[data-project-id="${id}"] .project-row`);await waitPage("document.querySelector('#project-context').textContent==='UI Project'",'project selected');
      await fill('#prompt','WRITE:WEB_PROJECT');await click('#send');await waitPage("document.querySelector('#timeline').textContent.includes('已完成 WRITE:WEB_PROJECT') && document.querySelector('#cancel').hidden",'project task settled');
      check('creating a project task from Web uses the selected project cwd',await readFile(join(a.workspace,'docs','result.txt'),'utf8')==='WEB_PROJECT\n');
      await click('#new-chat');await waitPage("document.querySelector('#project-context').textContent===''",'ordinary new task');
      check('global new-task entry does not inherit the last project',true);
    }else check('base-only UI hides project management while retaining files and settings',await evaluate("document.querySelector('#projects-region').hidden && !document.querySelector('#files-toggle').hidden"));
    await send('Emulation.setDeviceMetricsOverride',{width:390,height:900,deviceScaleFactor:1,mobile:false});
    await click('#files-toggle');await waitPage("!document.querySelector('#files-panel').hidden",'mobile files');
    check('390px right panel is an independent bounded overlay',await evaluate("document.querySelector('#files-panel').getBoundingClientRect().width<=390 && document.documentElement.scrollWidth<=390 && document.querySelector('#files-panel').getAttribute('aria-modal')==='true'"));
    await shot('workspace-mobile');await send('Emulation.setDeviceMetricsOverride',{width:320,height:800,deviceScaleFactor:1,mobile:false});
    check('320px file pane does not overflow the viewport',await evaluate("document.documentElement.scrollWidth<=320 && document.querySelector('#files-panel').getBoundingClientRect().width<=320"));
    await click('#files-close');
  }
  if(projectSession){
    await rename(projectOne,projectOne+'-moved');
    check('history remains readable after its directory goes missing',(await request(`/api/sessions/${projectSession.id}`)).session.id===projectSession.id);
    check('file service reports missing directories without switching roots',!(await request(`/api/sessions/${projectSession.id}/workspace`)).available);
    const h=(await request(`/api/sessions/${projectSession.id}`)).session;
    await request(`/api/sessions/${h.id}/turns`,{request_id:'missing-root',revision:h.revision,prompt:'WRITE:WRONG'},'POST',400);
  }
  check('no browser or fixture exceptions were left unhandled',errors.length===0);
  await writeFile(join(output,'report.json'),JSON.stringify({checks,modelCalls:modelRequests.length,noProjects},null,2));
  console.log(`PASS ${checks.length} workspace integration assertions; screenshots: ${output}`);
}catch(e){
  console.error(e);console.error(hostLog);process.exitCode=1;
  const page = socket ? await evaluate("({text:document.body.innerText.slice(-6000),path:document.querySelector('#workspace-root')?.value,notice:document.querySelector('#workspace-notice')?.textContent})").catch(()=>null) : null;
  await writeFile(join(output,'failure.json'),JSON.stringify({error:e.stack,page,checks},null,2));
  if(socket)await shot('failure').catch(()=>{});
}
finally{
  // Close our DevTools browser before tearing down its socket, then reap only our children.
  if(socket?.readyState===1)await send('Browser.close').catch(()=>{});
  for(const p of pending.values()){clearTimeout(p.timer);p.reject(new Error('test finished'));}pending.clear();socket?.close();
  await stop(browser);await stop(host);
  const closeServer=async(server)=>{ if(!server)return; const closing=new Promise(r=>server.close(r));server.closeAllConnections();await Promise.race([closing,sleep(5000)]); };
  await closeServer(ui);await closeServer(fixture);
  await rm(scratch,{recursive:true,force:true,maxRetries:5,retryDelay:100}).catch(()=>{});
  process.exit(process.exitCode || 0);
}
