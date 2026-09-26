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
  if (selector.startsWith('.file-entry')) {
    if (!await evaluate("Boolean(document.querySelector('.workspace-files:not([hidden])'))")) await click('#workspace-files-open');
    selector = '#right-content > .workspace-files:not([hidden]) ' + selector;
    await waitPage(`Boolean(document.querySelector(${JSON.stringify(selector)}))`, 'visible file page control');
  }
  const point = await evaluate(`(() => {const n=document.querySelector(${JSON.stringify(selector)});if(!n)throw new Error('missing element');n.scrollIntoView({block:'nearest'});const r=n.getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2};})()`);
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...point });
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...point });
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, ...point });
}
async function hover(selector) {
  const point = await evaluate(`(()=>{const r=document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2}})()`);
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...point });
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
  await waitPage("!document.querySelector('#workspace-files-open').hidden && !document.querySelector('#sessions-refresh').disabled",'workspace UI ready');
  await click('#workspace-files-open');
  await waitPage("document.querySelector('.workspace-files-path')?.textContent.length>0",'workspace files ready');
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
    check('workspace file view follows the selected persisted session',await evaluate(`document.querySelector('.workspace-files-path').textContent===${JSON.stringify(a.workspace)}`));
    await waitPage("document.querySelector('.file-entry[data-path=\"result.txt\"]')",'files listed');await click('.file-entry[data-path="result.txt"]');
    await waitPage("document.querySelector('#right-content > :not([hidden])').textContent.includes('ORDINARY_A_CHANGED')",'text preview');
    check('real file preview appears without sending another Agent task',await evaluate("document.querySelector('.workspace-source-preview code')!==null"));
    await shot('workspace-light');
    await click('.file-entry[data-path="docs"]');await waitPage("document.querySelector('.file-entry[data-path=\"docs/example.rs\"]')",'nested directory');
    await click('.file-entry[data-path="docs/example.rs"]');await waitPage("document.querySelector('#right-content > :not([hidden])').textContent.includes('fn main()')",'code preview');
    await click('.file-entry[data-path="pixel.png"]');
    await waitPage("document.querySelector('#right-content > :not([hidden]) img')?.naturalWidth>0",'image decoded');
    check('directory navigation and raster preview work through host authorization',true);
    await click('.file-entry[data-path="long.txt"]');await waitPage("!document.querySelector('#right-content > :not([hidden]) .right-file-more').hidden",'long file page');
    const textLength=await evaluate("document.querySelector('#right-content > :not([hidden]) code').textContent.length");await click('#right-content > :not([hidden]) .right-file-more');
    await waitPage(`document.querySelector('#right-content > :not([hidden]) code').textContent.length>${textLength}`,'next text page');check('long files are paged instead of loading an unlimited response',true);
    await click('#settings-button');await click('#settings-workspace-tab');
    await waitPage(`!document.querySelector('#workspace-root').disabled && document.querySelector('#workspace-root').value===${JSON.stringify(changed.default_root)}`,'workspace settings');
    await fill('#workspace-root',projectTwo);await click('#workspace-root-save');await waitPage("document.querySelector('#workspace-notice').textContent.includes('已保存')",'root saved from real form');
    check('default path is configurable from the Web UI without restart',normalizePath((await request('/api/workspace-settings')).default_root)===normalizePath(projectTwo));
    await click('#settings-appearance-tab');await click('label.theme-mode:has(#theme-mode-dark)');await waitPage("document.documentElement.dataset.theme==='dark'",'dark theme applied');await click('#settings-back');await shot('workspace-dark');
    await click('#files-close');check('right pane can be collapsed without changing the session',await evaluate("document.querySelector('#files-panel').hidden && location.hash.includes('ordinary-a')"));
    if(!noProjects){
      const realNameCreate = (await request('/api/workspace-settings')).project_create_mode === 'name';
      if(!realNameCreate){
        // Older fixture binaries only support explicit paths; the browser form still gets covered.
        await evaluate(`(async()=>{const {WorkspaceUI}=await import('/apps/web/workspace-ui.mjs');const {WorkspaceClient}=await import('/apps/web/workspace.mjs');const apply=WorkspaceUI.prototype.applySettings;WorkspaceUI.prototype.applySettings=function(value){return apply.call(this,{...value,project_create_mode:'name'})};WorkspaceClient.prototype.createProject=function(requestId,revision,name){return this.addProject({request_id:requestId,revision,name,path:${JSON.stringify(join(a.workspace,'docs'))}})}})()`);
        await click('#settings-button');await click('#settings-workspace-tab');await click('#workspace-settings-refresh');await click('#settings-back');
      }
      await waitPage("!document.querySelector('#project-add').disabled",'name creation available');
      await click('#project-add');await waitPage("document.querySelector('#project-dialog').open",'project name dialog');
      check('web project form asks only for a name',await evaluate("!!document.querySelector('#project-name')&&!document.querySelector('#project-path')"));
      await fill('#project-name','UI Project');await click('#project-save');
      await waitPage("document.querySelector('#project-list').textContent.includes('UI Project')",'project UI registration');
      check('name creation registers a real project', (await request('/api/projects')).projects.some(p=>p.name==='UI Project'));
      const uiProject=(await request('/api/projects')).projects.find(p=>p.name==='UI Project');
      const id=uiProject.id;
      if(realNameCreate) check('Web creates the named directory under the saved default root',normalizePath(uiProject.path)===normalizePath(join(projectTwo,'UI Project')));
      check('project navigation matches the reference controls without a file button or status log',await evaluate("(()=>{const heading=document.querySelector('.projects-heading'),group=[...document.querySelectorAll('.project-group')].find(item=>item.querySelector('.project-row')?.textContent.includes('UI Project'));return heading.querySelector('#projects-collapse')?.getAttribute('aria-expanded')==='true'&&heading.querySelector('#projects-sort')&&heading.querySelector('#project-add')&&group?.querySelector('.project-options')&&group?.querySelector('.project-new')&&!group?.querySelector('.project-files')&&!document.querySelector('#chat-sidebar #projects-notice')})()"));
      check('project and task headers align at both edges and expose matching section grips',await evaluate("(()=>{const p=document.querySelector('#projects-collapse span').getBoundingClientRect(),t=document.querySelector('#sessions-section-title').getBoundingClientRect(),pa=document.querySelector('#project-add').getBoundingClientRect(),ta=document.querySelector('#sessions-new').getBoundingClientRect();return Math.abs(p.left-t.left)<1&&Math.abs(pa.right-ta.right)<1&&document.querySelector('#projects-sort').draggable&&document.querySelector('#sessions-options').draggable&&document.querySelector('#projects-sort svg').innerHTML===document.querySelector('#sessions-options svg').innerHTML})()"));
      await evaluate("(()=>{const source=document.querySelector('#projects-sort'),target=document.querySelector('.sessions-heading'),transfer=new DataTransfer();source.dispatchEvent(new DragEvent('dragstart',{bubbles:true,dataTransfer:transfer}));target.dispatchEvent(new DragEvent('dragover',{bubbles:true,dataTransfer:transfer}));target.dispatchEvent(new DragEvent('drop',{bubbles:true,dataTransfer:transfer}));source.dispatchEvent(new DragEvent('dragend',{bubbles:true,dataTransfer:transfer}));})()");
      check('dragging a section grip swaps project and task sections and saves their order',await evaluate("document.querySelector('.sidebar-navigation').firstElementChild.classList.contains('sessions-region')&&JSON.stringify(JSON.parse(localStorage.getItem('mona.web.sidebar.sections.v1')))==='[\"tasks\",\"projects\"]'"));
      await hover('.sessions-heading');
      await waitPage("getComputedStyle(document.querySelector('.sessions-heading-actions')).opacity==='1'",'task section controls visible');
      await shot('sidebar-sections-reordered');
      await evaluate("(()=>{const handle=document.querySelector('#sessions-options');handle.focus();handle.dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowDown',altKey:true,bubbles:true,cancelable:true}));})()");
      check('keyboard section move restores the order without opening the task menu',await evaluate("document.querySelector('.sidebar-navigation').firstElementChild.id==='projects-region'&&document.querySelector('#sessions-options-menu').hidden&&JSON.stringify(JSON.parse(localStorage.getItem('mona.web.sidebar.sections.v1')))==='[\"projects\",\"tasks\"]'"));
      await hover('#new-chat');
      await waitPage("getComputedStyle(document.querySelector('.projects-heading-actions')).opacity==='0'&&getComputedStyle(document.querySelector('.project-group:last-child .project-actions')).opacity==='0'",'project actions hidden after pointer leaves');
      check('project header and row actions are hidden until hover',await evaluate("getComputedStyle(document.querySelector('.projects-heading-actions')).opacity==='0'&&getComputedStyle(document.querySelector('.project-group:last-child .project-actions')).opacity==='0'"));
      await hover('.project-group:last-child .project-row');
      await waitPage("getComputedStyle(document.querySelector('.projects-heading-actions')).opacity==='1'&&getComputedStyle(document.querySelector('.project-group:last-child .project-actions')).opacity==='1'",'project hover actions');
      check('hover reveals project controls without a row background',await evaluate("getComputedStyle(document.querySelector('.project-group:last-child .project-row')).backgroundColor==='rgba(0, 0, 0, 0)'"));
      await click('#projects-sort');
      await evaluate("(()=>{const groups=[...document.querySelectorAll('#project-list .project-group')],source=groups.at(-1).querySelector('.project-item'),target=groups[0].querySelector('.project-item'),dataTransfer=new DataTransfer(),y=target.getBoundingClientRect().top+1;source.dispatchEvent(new DragEvent('dragstart',{bubbles:true,dataTransfer}));target.dispatchEvent(new DragEvent('dragover',{bubbles:true,dataTransfer,clientY:y}));target.dispatchEvent(new DragEvent('drop',{bubbles:true,dataTransfer,clientY:y}));source.dispatchEvent(new DragEvent('dragend',{bubbles:true,dataTransfer}));})()");
      check('drag sorting moves a project and saves its order',await evaluate("(()=>{const key=Object.keys(localStorage).find(key=>key.startsWith('mona.web.projects.order.v1:'));return document.querySelector('#project-list .project-row')?.textContent.includes('UI Project')&&JSON.parse(localStorage.getItem(key)||'[]').length===2})()"));
      await evaluate("(()=>{const row=document.querySelector('#project-list .project-row');row.focus();row.dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowDown',bubbles:true,cancelable:true}));})()");
      check('keyboard sorting moves the focused project',await evaluate("document.querySelector('#project-list .project-row')?.textContent.includes('Project Two')"));
      await click('#projects-sort');await click('#projects-collapse');
      check('project heading collapses the project list',await evaluate("document.querySelector('#projects-collapse').getAttribute('aria-expanded')==='false'&&getComputedStyle(document.querySelector('#project-list')).display==='none'"));
      await click('#projects-collapse');
      await click('.project-group:last-child .project-options');
      check('project options contain the removal action',await evaluate("!document.querySelector('.project-options-menu').hidden&&document.querySelector('.project-options-menu').textContent.includes('移除项目登记')"));
      await send('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
      await click('.project-group:last-child .project-new');
      await waitPage("document.querySelector('#project-context').textContent==='UI Project'",'project task created from row action');
      check('project new-task action binds the project',true);
      await click('#settings-button');await click('#settings-appearance-tab');await click('label.theme-mode:has(#theme-mode-light)');await click('#settings-back');
      await click('#new-chat');
      await waitPage("!document.querySelector('#project-picker').hidden && document.querySelector('#project-context').textContent===''",'unselected project picker');
      await shot('new-task-project-empty');
      await click('#project-picker');
      await waitPage("!document.querySelector('#composer-project-menu').hidden && [...document.querySelectorAll('#composer-project-menu button')].some(button=>button.textContent.includes('UI Project'))",'real project options');
      check('project menu opens above the trigger with focused search and actual actions',await evaluate("(()=>{const menu=document.querySelector('#composer-project-menu'),trigger=document.querySelector('#project-picker');return menu.getBoundingClientRect().bottom<trigger.getBoundingClientRect().top && document.activeElement.id==='project-menu-search' && menu.querySelector('#project-menu-actions').textContent.includes('添加项目') && menu.querySelector('#project-menu-actions').textContent.includes('不在项目中工作') && !menu.textContent.includes('远程连接')})()"));
      await fill('#project-menu-search','Project Two');
      check('search filters actual projects and keeps the outside-project action',await evaluate("(()=>{const rows=[...document.querySelectorAll('#project-menu-list button')];return rows.length===1 && rows[0].textContent.includes('Project Two') && document.querySelector('#project-menu-actions').textContent.includes('不在项目中工作')})()"));
      await fill('#project-menu-search','');
      await send('Input.dispatchKeyEvent',{type:'keyDown',key:'ArrowDown',code:'ArrowDown',windowsVirtualKeyCode:40});
      check('arrow key moves from search to the first project',await evaluate("document.activeElement===document.querySelector('#project-menu-list button')"));
      await send('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
      check('Escape closes the menu and restores focus',await evaluate("document.querySelector('#composer-project-menu').hidden && document.activeElement.id==='project-picker'"));
      await click('#project-picker');
      await click('.composer-project-add');
      await waitPage("document.querySelector('#project-dialog').open",'project name dialog from composer');
      await shot('project-name-dialog');
      await click('#project-cancel');
      await waitPage("!document.querySelector('#project-dialog').open && document.activeElement.id==='project-picker'",'cancelled project form returns focus');
      check('cancelled project creation leaves projects unchanged',await evaluate("document.querySelectorAll('.project-group').length===2"));
      await click('#project-picker');
      await shot('new-task-project-menu');
      await evaluate("[...document.querySelectorAll('#composer-project-menu button')].find(button=>button.textContent.includes('UI Project')).click()");
      await waitPage("document.querySelector('#project-context').textContent==='UI Project' && document.querySelector('#composer-project-menu').hidden",'project selected from composer');
      await click('#project-picker');
      check('selected project and outside-project row show opposite checked states',await evaluate("(()=>{const project=[...document.querySelectorAll('#project-menu-list button')].find(button=>button.textContent.includes('UI Project'));return project?.getAttribute('aria-checked')==='true' && document.querySelector('.composer-project-outside')?.getAttribute('aria-checked')==='false' && !document.querySelector('#project-detach').hidden})()"));
      await click('.composer-project-outside');
      await waitPage("document.querySelector('#project-context').textContent==='' && document.querySelector('#composer-project-menu').hidden",'outside project selected');
      await click('#project-picker');
      await evaluate("[...document.querySelectorAll('#project-menu-list button')].find(button=>button.textContent.includes('UI Project')).click()");
      await waitPage("document.querySelector('#project-context').textContent==='UI Project' && document.querySelector('#composer-project-menu').hidden",'project reselected from composer');
      await shot('new-task-project-selected');
      await click('#project-detach');
      await waitPage("document.querySelector('#project-context').textContent===''",'selected project detached with chip action');
      await click('#project-picker');
      await evaluate("[...document.querySelectorAll('#project-menu-list button')].find(button=>button.textContent.includes('UI Project')).click()");
      await waitPage("document.querySelector('#project-context').textContent==='UI Project'",'project rebound after shortcut detach');
      await fill('#prompt','WRITE:WEB_PROJECT');await click('#send');await waitPage("document.querySelector('#timeline').textContent.includes('已完成 WRITE:WEB_PROJECT') && document.querySelector('#cancel').hidden",'project task settled');
      check('creating a project task from Web uses the selected project cwd',await readFile(join(uiProject.path,'result.txt'),'utf8')==='WEB_PROJECT\n');
      await waitPage("[...document.querySelectorAll('.project-group')].some(group=>group.querySelector('.project-row')?.textContent.includes('UI Project')&&group.querySelector('.project-sessions')?.textContent.includes('WRITE:WEB_PROJECT'))",'project task grouped in sidebar');
      check('sidebar places project tasks below their folder and ordinary tasks in the task section',await evaluate("(()=>{const group=[...document.querySelectorAll('.project-group')].find(group=>group.querySelector('.project-row')?.textContent.includes('UI Project'));return group?.querySelector('.project-sessions .session-row')?.textContent.includes('WRITE:WEB_PROJECT')&&document.querySelector('#sessions-list')?.textContent.includes('WRITE:ORDINARY_A')})()"));
      check('selected project and task stay flat with their titles aligned',await evaluate("(()=>{const group=[...document.querySelectorAll('.project-group')].find(group=>group.querySelector('.project-row')?.textContent.includes('UI Project')),project=group?.querySelector('.project-row'),task=group?.querySelector('.project-sessions .session-row');return project&&task&&getComputedStyle(project).backgroundColor==='rgba(0, 0, 0, 0)'&&getComputedStyle(task).backgroundColor==='rgba(0, 0, 0, 0)'&&Math.abs(project.querySelector('span').getBoundingClientRect().left-task.querySelector('.session-title').getBoundingClientRect().left)<3})()"));
      await hover('.project-group:last-child .project-row');
      await waitPage("getComputedStyle(document.querySelector('.project-group:last-child .project-actions')).opacity==='1'",'reference project controls visible');
      await shot('sidebar-project-groups-light');
      await evaluate("document.querySelector('#project-list .project-group:first-child').hidden=true");
      await hover('.project-group:last-child .project-row');
      await waitPage("getComputedStyle(document.querySelector('.project-group:last-child .project-actions')).opacity==='1'&&getComputedStyle(document.querySelector('.project-group:last-child .session-action')).opacity==='0'",'reference hover after project layout shift');
      const projectCrop=await send('Page.captureScreenshot',{format:'png',captureBeyondViewport:false,clip:{x:0,y:119,width:294,height:118,scale:2}});
      await writeFile(join(output,'sidebar-project-reference-scale.png'),Buffer.from(projectCrop.data,'base64'));
      await evaluate("document.querySelector('#project-list .project-group:first-child').hidden=false");
      await hover('.project-group:last-child .project-sessions .session-row');
      check('project task hover has no background block',await evaluate("getComputedStyle(document.querySelector('.project-group:last-child .project-sessions .session-row')).backgroundColor==='rgba(0, 0, 0, 0)'"));
      await hover('#sessions-list .session-row');
      check('ordinary task hover has no background block',await evaluate("getComputedStyle(document.querySelector('#sessions-list .session-row')).backgroundColor==='rgba(0, 0, 0, 0)'"));
      await click('#settings-button');await click('#settings-appearance-tab');await click('label.theme-mode:has(#theme-mode-dark)');await waitPage("document.documentElement.dataset.theme==='dark'",'dark sidebar theme');await click('#settings-back');
      await shot('sidebar-project-groups-dark');
      await click('#settings-button');await click('#settings-appearance-tab');await click('label.theme-mode:has(#theme-mode-light)');await waitPage("document.documentElement.dataset.theme==='light'",'light sidebar theme');await click('#settings-back');
      for(let index=0;index<5;index++)await request('/api/sessions',{request_id:`sidebar-extra-${index}`,project_id:id});
      await evaluate("document.querySelector('#sessions-refresh').click()");
      const projectGroup = `.project-group[data-project-id="${id}"]`;
      await waitPage(`document.querySelector(${JSON.stringify(projectGroup+' .project-more')})?.hidden===false&&document.querySelectorAll(${JSON.stringify(projectGroup+' .session-row')}).length===5`,'project group initial limit');
      await click(`${projectGroup} .project-more`);
      await waitPage(`document.querySelectorAll(${JSON.stringify(projectGroup+' .session-row')}).length===6`,'project group expanded');
      check('show more expands only the chosen project group',await evaluate(`document.querySelectorAll(${JSON.stringify(projectGroup+' .session-row')}).length===6&&document.querySelectorAll('#sessions-list .session-row').length===4`));
      await click('#new-chat');await waitPage("document.querySelector('#project-context').textContent===''",'ordinary new task');
      check('global new-task entry does not inherit the last project',true);
    }else check('base-only UI hides project management while retaining files and settings',await evaluate("document.querySelector('#projects-region').hidden && !document.querySelector('#files-toggle').hidden"));
    await waitPage(`document.querySelector('#sessions-list button[data-session-id="${a.id}"]') && !document.querySelector('#sessions-refresh').disabled`,'ordinary history available');
    await click(`#sessions-list button[data-session-id="${a.id}"]`);
    await waitPage("location.hash.includes('ordinary-a') && !document.querySelector('#sessions-refresh').disabled && !document.querySelector('#files-toggle').hidden",'saved session selected for mobile preview');
    await send('Emulation.setDeviceMetricsOverride',{width:390,height:900,deviceScaleFactor:1,mobile:false});
    if(!noProjects){
      await waitPage("document.querySelector('#sidebar-toggle').parentElement===document.querySelector('.thread-heading')&&document.querySelector('#chat-sidebar').inert",'mobile sidebar toggle settled');
      await click('#sidebar-toggle');await waitPage("document.querySelector('.shell').classList.contains('sidebar-open')",'mobile project drawer');
      check('mobile project drawer keeps grouped tasks within the viewport',await evaluate("document.documentElement.scrollWidth<=390&&document.querySelector('#chat-sidebar').getBoundingClientRect().right<=390&&document.querySelector('.project-group .project-sessions .session-row')!==null"));
      await shot('sidebar-project-groups-mobile');
      await send('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27});
      await waitPage("!document.querySelector('.shell').classList.contains('sidebar-open')",'mobile drawer closed');
    }
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
