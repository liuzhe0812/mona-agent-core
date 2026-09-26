// Real Rust app + PTY/Git/files + formal browser UI. No paid model, user directories or existing services.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn, execFileSync } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { startUiServer } from '../../../scripts/dev-web.mjs';
import { docx, xlsx, pptx } from './workbench-fixtures.mjs';
const scratch = await mkdtemp(join(tmpdir(), 'mona-right-pane-'));
const state = join(scratch, 'state'), home = join(scratch, 'home'); await mkdir(state); await mkdir(home);
const reportDir = resolve(process.env.MONA_TEST_REPORT_DIR || '.tmp-verify/right-pane-browser-report'); await mkdir(reportDir, { recursive: true });
const token = 'isolated-right-pane-test-token-4187629001'; const checks = [], errors = [], modelRequests = [];
let host, ui, browser, socket, logs = '', seq = 0; const pending = new Map();
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const check = (name, value) => { assert.ok(value, name); checks.push(name); console.log('PASS ' + name); };
async function wait(fn, label, time = 30000) { const end = Date.now() + time; while (Date.now() < end) { if (await fn()) return; await sleep(60); } throw new Error('Timeout: ' + label); }
async function freePort() { const server = createServer(); await new Promise(r => server.listen(0, '127.0.0.1', r)); const port = server.address().port; await new Promise(r => server.close(r)); return port; }
async function stop(child) { if (!child || child.exitCode != null) return; const promise = once(child, 'exit'); child.kill('SIGKILL'); await Promise.race([promise, sleep(3000)]); }
const model = createServer(async (req, res) => {
  let text = ''; for await (const part of req) text += part; const body = JSON.parse(text); modelRequests.push(body);
  res.writeHead(200, { 'Content-Type': 'text/event-stream' });
  const prompt = body.messages.filter(m => m.role === 'user').at(-1)?.content || '';
  res.end('data: ' + JSON.stringify({ choices: [{ index: 0, delta: { content: '已完成：' + prompt }, finish_reason: 'stop' }], usage: { prompt_tokens: 1000, completion_tokens: 100, prompt_tokens_details: { cached_tokens: 800 } } }) + '\n\ndata: [DONE]\n\n');
});
await new Promise(r => model.listen(0, '127.0.0.1', r));
const port = await freePort(), base = `http://127.0.0.1:${port}`;
async function api(path, body, method = body == null ? 'GET' : 'POST', status = 200, auth = true) {
  const response = await fetch(base + path, { method, headers: { ...(auth ? { Authorization: `Bearer ${token}` } : {}), 'Content-Type': 'application/json' }, body: body == null ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(20000) });
  const text = await response.text(); assert.equal(response.status, status, path + ': ' + text.slice(0, 200)); return text ? JSON.parse(text) : null;
}
async function send(method, params = {}, sessionId) { return new Promise((resolve, reject) => {
  const id = ++seq; const timer = setTimeout(() => { pending.delete(id); reject(new Error('CDP timeout ' + method)); }, 20000); pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
}); }
async function evaluate(expression, contextId) { const value = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true, ...(contextId ? { contextId } : {}) }); if (value.exceptionDetails) throw new Error(value.exceptionDetails.exception?.description || 'Browser expression failed'); return value.result.value; }
const pageWait = (expression, label, time) => wait(() => evaluate(expression), label, time);
async function click(selector, button = 'left') {
  if (selector.startsWith('.file-entry')) {
    const visible = await evaluate("document.querySelector('#right-content > .workspace-files:not([hidden])')");
    if (!visible) {
      if (await evaluate("Boolean(document.querySelector('.right-tab[data-kind=files]'))")) await click('.right-tab[data-kind=files]');
      else await click('#workspace-files-open');
    }
    selector = '#right-content > .workspace-files:not([hidden]) ' + selector;
    await pageWait(`Boolean(document.querySelector(${JSON.stringify(selector)}))`, 'file page control');
  }
  const point = await evaluate(`(async () => {
    const n=document.querySelector(${JSON.stringify(selector)}); if(!n) throw Error('missing '+${JSON.stringify(selector)});
    n.scrollIntoView({block:'nearest'}); let previous, stable=0;
    for(let attempt=0;attempt<45;attempt++) {
      await new Promise(resolve=>requestAnimationFrame(resolve));
      const r=n.getBoundingClientRect();
      const points=[{x:r.x+r.width/2,y:r.y+r.height/2},{x:r.x+Math.min(12,r.width/4),y:r.y+r.height/2}];
      const point=points.find(p=>n.contains(document.elementFromPoint(p.x,p.y)))||points[0],hit=document.elementFromPoint(point.x,point.y);
      if(r.width>0&&r.height>0&&n.contains(hit)&&previous&&Math.abs(previous.x-point.x)<0.5&&Math.abs(previous.y-point.y)<0.5) stable++; else stable=0;
      if(stable>=2)return point;
      previous=point;
      if(!n.contains(hit)&&attempt%10===9)n.scrollIntoView({block:'center'});
    }
    const r=n.getBoundingClientRect(); throw Error('control did not become stable and clickable: '+${JSON.stringify(selector)}+' '+JSON.stringify({width:r.width,height:r.height,x:r.x,y:r.y,hit:document.elementFromPoint(r.x+r.width/2,r.y+r.height/2)?.className}));
  })()`);
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...point });
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', button, clickCount: 1, ...point }); await send('Input.dispatchMouseEvent', { type: 'mouseReleased', button, clickCount: 1, ...point });
}
async function textButton(container, label) { const selector = await evaluate(`(() => {const b=[...document.querySelectorAll(${JSON.stringify(container)}+' button')].find(b=>b.textContent.trim()===${JSON.stringify(label)});if(!b)throw Error('button '+${JSON.stringify(label)});b.id||=( 'test-control-'+Math.random().toString(36).slice(2));return '#'+b.id;})()`); await click(selector); }
async function shot(name) { const image = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false }); await writeFile(join(reportDir, name + '.png'), Buffer.from(image.data, 'base64')); }
async function officeText(name) {
  const url = await evaluate("document.querySelector('#right-content iframe').src");
  const targets = await send('Target.getTargets'); const target = targets.targetInfos.find(info => info.type === 'iframe' && info.url === url);
  if (target) {
    const { sessionId } = await send('Target.attachToTarget', { targetId: target.targetId, flatten: true });
    try { const value = await send('Runtime.evaluate', { expression: 'document.body.textContent', returnByValue: true }, sessionId); if (value.exceptionDetails) throw Error(name + ' renderer read failed'); return value.result.value; }
    finally { await send('Target.detachFromTarget', { sessionId }); }
  }
  const tree = (await send('Page.getFrameTree')).frameTree; const find = t => { if (t.frame.url.includes('/document-preview.html')) return t.frame.id; for (const child of t.childFrames || []) { const v = find(child); if (v) return v; } };
  const frame = find(tree); assert.ok(frame, name + ' frame'); const world = await send('Page.createIsolatedWorld', { frameId: frame, worldName: 'Mona verifier' }); return evaluate('document.body.textContent', world.executionContextId);
}
try {
  ui = await startUiServer({ host: '127.0.0.1', port: 0, endpoint: base, token }); const origin = `http://127.0.0.1:${ui.address().port}`;
  const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !k.startsWith('AGENT_') && !k.startsWith('MONA_DEV_')));
  Object.assign(env, { MONA_DEV_STATE_DIR: state, HOME: home, USERPROFILE: home, AGENT_SERVER_ADDR: `127.0.0.1:${port}`, AGENT_SERVER_TOKEN: token, AGENT_UI_ORIGIN: origin, AGENT_MODEL_CONTEXT_TOKENS: '32000',
    AGENT_MODEL_ENDPOINT: `http://127.0.0.1:${model.address().port}/chat/completions`, AGENT_MODEL_NAME: 'right-pane-fixture', AGENT_ALLOW_HTTP_LOOPBACK: '1', AGENT_MODEL_MANAGEMENT: '0', AGENT_SKILLS: '0', AGENT_INSTRUCTIONS: '0', AGENT_MEMORY: '0', AGENT_HISTORY_SEARCH: '0' });
  host = spawn(resolve(process.env.MONA_TEST_SERVER || (process.platform === 'win32' ? 'target/debug/server.exe' : 'target/debug/server')), [], { cwd: scratch, env, stdio: ['ignore', 'pipe', 'pipe'] }); host.stderr.on('data', d => { logs = (logs + d).slice(-6000); });
  await wait(async () => { if (host.exitCode != null) throw Error(logs); try { return (await api('/v1/info')).protocol_version === 2; } catch { return false; } }, 'host');
  const caps = await api('/api/workbench/capabilities'); check('application capabilities expose real file/review/PTY services and not a pretend browser', caps.terminal && caps.review && caps.bytes && !caps.embedded_browser);
  await api('/api/workbench/capabilities', undefined, 'GET', 401, false);
  const a = await api('/api/sessions', { request_id: 'pane-a' }), b = await api('/api/sessions', { request_id: 'pane-b' });
  check('new ordinary sessions use the hidden root and independent working directories', a.workspace.includes('.mona-agent') && a.workspace !== b.workspace);
  await api(`/api/sessions/${a.id}/turns`, { request_id: 'first', revision: a.revision, prompt: '主任务原文' });
  await wait(async () => (await api(`/api/sessions/${a.id}`)).session.status === 'completed', 'main task');
  const files = { 'notes.md': '# MONA_MARKDOWN\n\n**preview**\n', 'code.rs': 'fn main() { println!("MONA_CODE"); }\n', 'safe.html': '<h1>MONA_HTML</h1><script>parent.__unsafe=true</script><img src="https://example.invalid/tracker">', 'long.txt': '长文件分页\n'.repeat(14000) };
  await mkdir(join(a.workspace, 'docs')); for (const [name, text] of Object.entries(files)) await writeFile(join(a.workspace, name), text);
  await writeFile(join(a.workspace, 'docs', 'nested.txt'), 'MONA_NESTED'); await writeFile(join(a.workspace, 'test.docx'), docx()); await writeFile(join(a.workspace, 'test.xlsx'), xlsx()); await writeFile(join(a.workspace, 'test.pptx'), pptx());
  const git = args => execFileSync('git', args, { cwd: a.workspace, stdio: 'pipe' }); git(['init']); git(['add', 'notes.md']); git(['-c', 'user.name=Mona Test', '-c', 'user.email=mona-test@example.invalid', 'commit', '-m', 'fixture']); await writeFile(join(a.workspace, 'notes.md'), files['notes.md'] + '\nMODIFIED_FOR_REVIEW\n');
  const root = `/api/workbench/session/${a.id}`;
  const metadata = await api(root + '/stat?path=notes.md');
  check('file cards use authorized metadata without downloading bodies', metadata.kind === 'file' && metadata.path === 'notes.md' && metadata.bytes > 0 && metadata.text === undefined);
  await api(root + '/stat?path=..%2Fsecret', undefined, 'GET', 400);
  await writeFile(join(a.workspace, 'pixel.png'), Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=', 'base64'));
  const changes = await api(root + '/review'); check('review reads actual Git state from the pinned workspace', changes.available && changes.entries.some(e => e.path === 'notes.md'));
  const diff = await api(root + '/diff?path=notes.md'); check('review returns real patch contents without staging or changing files', diff.text.includes('+MODIFIED_FOR_REVIEW'));
  await api(root + '/bytes?path=..%2Fsecret', undefined, 'GET', 400);
  const terminal = await api(root + '/terminals', { request_id: 'terminal-api', rows: 24, cols: 80 });
  let terminalText = '', after = 0, inputSequence = 0;
  const input = data => api(root + `/terminals/${terminal.id}/input`, { sequence: ++inputSequence, data }, 'POST', 204);
  const pump = async () => {
    const output = await api(root + `/terminals/${terminal.id}/output?after=${after}`); after = output.next;
    const text = Buffer.from(output.base64, 'base64').toString('utf8'); terminalText += text;
    // ConPTY asks the terminal emulator for a cursor report before starting the shell.
    // The browser's xterm handles this; this protocol-level test must answer it too.
    if (text.includes('\x1b[6n')) await input('\x1b[1;1R');
  };
  await wait(async () => { await pump(); return /PS |> |\$ |# /.test(terminalText); }, 'PTY shell prompt');
  await input(process.platform === 'win32' ? "[Console]::WriteLine(('PTY_'+'CONFIRMED')); (Get-Location).Path\r" : "printf 'PTY_%s\\n' CONFIRMED; pwd\r");
  await wait(async () => { await pump(); return terminalText.includes('PTY_CONFIRMED') && terminalText.includes('s-pane-a'); }, 'real PTY output').catch(error => { console.error('PTY diagnostic', JSON.stringify(terminalText)); throw error; });
  check('terminal is an interactive PTY with the correct cwd, not a model/shell-tool simulation', terminalText.includes('s-pane-a'));
  await api(`/api/workbench/session/${b.id}/terminals/${terminal.id}/output`, undefined, 'GET', 403);
  await api(root + `/terminals/${terminal.id}/size`, { rows: 31, cols: 100 }, 'POST', 204);
  await api(root + `/terminals/${terminal.id}`, undefined, 'DELETE', 204); await api(root + `/terminals/${terminal.id}/output`, undefined, 'GET', 404);
  check('terminal resize, target ownership and explicit close are enforced', true);
  for (let index = 1; index <= 12; index++) {
    const current = (await api(`/api/sessions/${a.id}`)).session;
    const prompt = index === 12 ? '最新回答\n\n[查看代码](code.rs#L1)\n\n[打开文档](test.docx)\n\n![附件图](pixel.png)' : `历史验证第 ${index} 轮：${'保留阅读位置。'.repeat(25)}`;
    await api(`/api/sessions/${a.id}/turns`, { request_id: 'history-' + index, revision: current.revision, prompt });
    await wait(async () => (await api(`/api/sessions/${a.id}`)).session.status === 'completed', 'history fixture ' + index);
  }
  const statsCalls = modelRequests.length;
  const stats = await api(`/api/workbench/session/${a.id}/statistics`);
  check('statistics cover persisted turns and keep provider input/output totals unchanged', stats.turns === 13 && stats.steps === 13 && stats.reported_tokens === 14300 && stats.input_tokens === 13000 && stats.output_tokens === 1300 && stats.usage_complete);
  check('context is tied to the actual run and bound capacity, not the current UI model', stats.context?.capacity === 32000 && stats.context.tokens > 0 && stats.context.run_id === stats.latest_run_id);
  check('provider cache usage survives parsing and session aggregation without extra model calls', stats.cache_read_tokens === 10400 && stats.cache_write_tokens === 0 && stats.cache_hit_percent === 80 && statsCalls === modelRequests.length);
  await api(`/api/workbench/session/${a.id}/statistics`, undefined, 'GET', 401, false);
  const countBefore = (await api(`/api/sessions/${a.id}`)).session.turn_count;
  await api(`/api/sessions/${a.id}/side`, { window_id: 'side-test' });
  const starts = await Promise.all([api(`/api/sessions/${a.id}/side/turns`, { window_id: 'side-test', request_id: 'one', prompt: '侧边问题' }), api(`/api/sessions/${a.id}/side/turns`, { window_id: 'side-test', request_id: 'one', prompt: '侧边问题' })]);
  check('concurrent side requests are admitted once', starts[0].run_id === starts[1].run_id);
  await api(`/api/sessions/${a.id}/side/turns`, { window_id: 'side-test', request_id: 'one', prompt: 'different' }, 'POST', 409);
  await wait(async () => (await api(`/v1/runs/${starts[0].run_id}/result`)).outcome != null, 'side result');
  check('side conversation does not add turns to the main history', (await api(`/api/sessions/${a.id}`)).session.turn_count === countBefore);
  await api(`/api/sessions/${a.id}/side/side-test`, undefined, 'DELETE', 204);
  const browserPort = await freePort();
  browser = spawn(process.env.BROWSER_BIN || 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe', ['--headless=new', '--disable-gpu', '--no-first-run', `--remote-debugging-port=${browserPort}`, `--user-data-dir=${join(scratch, 'browser')}`, 'about:blank'], { stdio: 'ignore' });
  let pages; await wait(async () => { try { pages = await (await fetch(`http://127.0.0.1:${browserPort}/json/list`)).json(); return pages.some(p => p.type === 'page'); } catch { return false; } }, 'browser');
  socket = new WebSocket(pages.find(p => p.type === 'page').webSocketDebuggerUrl); await once(socket, 'open');
  socket.addEventListener('message', event => { const m = JSON.parse(event.data), p = pending.get(m.id); if (p) { pending.delete(m.id); clearTimeout(p.timer); m.error ? p.reject(Error(JSON.stringify(m.error))) : p.resolve(m.result); } if (m.method === 'Runtime.exceptionThrown') errors.push(m.params.exceptionDetails.exception?.description || 'JS exception'); });
  await send('Page.enable'); await send('Runtime.enable'); await send('Emulation.setFocusEmulationEnabled', { enabled: true }); await send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false });
  await send('Page.navigate', { url: `${origin}/#session=${a.id}` });
  await pageWait("document.querySelector('#workspace-files-open') && !document.querySelector('#workspace-files-open').hidden && !document.querySelector('#sessions-refresh').disabled", 'formal UI');
  check('web task header keeps the title order and hides desktop-only system openers', await evaluate("(()=>{const h=document.querySelector('.thread-heading'),a=document.querySelector('.topbar-actions');return !document.querySelector('#header-workspace-context').hidden&&!document.querySelector('#thread-more').hidden&&document.querySelector('#header-open-group').hidden&&!document.querySelector('#header-terminal').hidden&&h.children[0].id==='header-workspace-context'&&h.children[1].id==='thread-title'&&h.children[2].id==='thread-more'&&a.lastElementChild.id==='files-toggle'&&!document.querySelector('.topbar').textContent.includes('分享')&&!document.querySelector('.topbar').textContent.includes('帮助')})()"));
  await click('#header-workspace-context');
  check('workspace icon reveals the actual path and last activity without switching projects', await evaluate("!document.querySelector('#header-workspace-popover').hidden&&document.querySelector('#header-workspace-popover').textContent.includes('right-pane-')&&document.querySelector('#header-workspace-popover').textContent.includes('最近活动')"));
  await send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
  check('Escape closes workspace context and returns focus', await evaluate("document.querySelector('#header-workspace-popover').hidden&&document.activeElement===document.querySelector('#header-workspace-context')"));
  await click('#thread-more');
  check('task title menu exposes real task and workspace actions in ZCode order', await evaluate("(()=>{const menu=document.querySelector('#session-menu'),items=[...menu.querySelectorAll('button')].map(x=>x.textContent.trim());return !menu.hidden&&menu.classList.contains('is-header')&&items[0]==='置顶聊天'&&items[1]==='重命名任务'&&items[2]==='归档'&&items[3]==='标记为未读'&&items.includes('打开任务文件夹')&&items.includes('复制工作区路径')&&items.includes('复制会话 ID')&&!items.includes('删除任务')})()"));
  await shot('topbar-task-menu');
  await send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
  check('Escape closes the task menu and returns focus to its trigger', await evaluate("document.querySelector('#session-menu').hidden&&document.activeElement===document.querySelector('#thread-more')"));
  await click('#thread-more'); await textButton('#session-menu', '归档');
  check('archive requests confirmation in the shared dialog without mutating the task yet', await evaluate("document.querySelector('#header-archive-dialog').open&&document.querySelector('#header-archive-description').textContent.includes('任务原文')"));
  await click('#header-archive-dialog [value=cancel]');
  await pageWait("!document.querySelector('#header-archive-dialog').open&&document.activeElement===document.querySelector('#thread-more')", 'archive cancellation focus');
  check('cancelling archive keeps the current task and returns focus', await evaluate("!document.querySelector('#header-archive-dialog').open&&document.querySelector('#thread-more')===document.activeElement&&!document.querySelector('#thread-more').hidden"));
  await click('#header-terminal');
  await pageWait("document.querySelector('.xterm-screen')&&!document.querySelector('#files-panel').hidden", 'header terminal action');
  await click('#header-terminal');
  check('terminal toolbar button collapses the pane while retaining its terminal tab', await evaluate("document.querySelector('#files-panel').hidden&&document.querySelector('.terminal-pane')&&document.querySelector('#header-terminal').getAttribute('aria-pressed')==='false'"));
  await click('#header-terminal');
  check('terminal toolbar button reopens the same live terminal', await evaluate("!document.querySelector('#files-panel').hidden&&document.querySelectorAll('.terminal-pane').length===1&&document.querySelector('#header-terminal').getAttribute('aria-pressed')==='true'"));
  await evaluate("document.querySelector('#header-terminal').focus()");
  await send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'j', code: 'KeyJ', windowsVirtualKeyCode: 74, modifiers: 2 });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'j', code: 'KeyJ', windowsVirtualKeyCode: 74, modifiers: 2 });
  check('Ctrl+J collapses the live terminal without closing its tab', await evaluate("document.querySelector('#files-panel').hidden&&document.querySelectorAll('.terminal-pane').length===1"));
  await send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'j', code: 'KeyJ', windowsVirtualKeyCode: 74, modifiers: 2 });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'j', code: 'KeyJ', windowsVirtualKeyCode: 74, modifiers: 2 });
  check('Ctrl+J restores the terminal tab', await evaluate("!document.querySelector('#files-panel').hidden&&document.querySelectorAll('.terminal-pane').length===1"));
  await click('.right-tab-wrap:has(.right-tab[data-kind=terminal]) .right-tab-close');
  const legacyPath = join(state, 'sessions', 'legacy.jsonl');
  await writeFile(legacyPath, 'old-format\n');
  check('the host reports the isolated unreadable file without changing valid sessions', (await api('/api/sessions')).unreadable === 1);
  await evaluate("document.querySelector('#sessions-refresh').click()"); await sleep(250);
  check('unreadable background files do not show a red composer notice', await evaluate("document.querySelector('#sessions-notice').textContent==='已保存在宿主本地' && !document.querySelector('#sessions-notice').classList.contains('error')"));
  await rm(legacyPath);
  await pageWait("document.querySelectorAll('#timeline .turn').length===10 && document.querySelector('#timeline .message-file-link[data-file-path=\"code.rs#L1\"]')", 'history presentation context');
  await pageWait("document.querySelector('#timeline .message-file-card button:not(:disabled)') && document.querySelector('#timeline .message-media img')?.naturalWidth>0", 'file card metadata and authorized image');
  check('conversation images and file cards use the actual session workspace', true);
  await pageWait("document.querySelector('[data-metric=tokens]')?.textContent.includes('14K')&&document.querySelector('[data-metric=tokens]')?.textContent.includes('80%')", 'whole session metrics');
  check('statistics slot stays inside the composer footer, not outside the application shell', await evaluate("document.querySelector('#conversation-status-slots').parentElement===document.querySelector('#chat-workspace > .composer-wrap') && document.querySelector('#chat-workspace').closest('.shell') && document.querySelector('#files-panel').parentElement===document.querySelector('.shell')"));
  const statisticsVisible = "(()=>{const composer=document.querySelector('.composer').getBoundingClientRect();const pills=[...document.querySelectorAll('.conversation-stat')];return pills.length===3&&pills.every(n=>{const r=n.getBoundingClientRect();return r.width>0&&r.height>0&&r.top>=composer.bottom-1&&r.left>=0&&r.right<=innerWidth&&r.bottom<=innerHeight&&n.contains(document.elementFromPoint(r.x+r.width/2,r.y+r.height/2))})})()";
  await pageWait(statisticsVisible, 'visible statistics below the composer');
  check('performance, token usage and context gauge are visible below the input', await evaluate(statisticsVisible));
  const hoverPoint = await evaluate("(()=>{const r=document.querySelector('[data-metric=tokens]').getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2}})()");
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...hoverPoint }); await sleep(450);
  check('statistics show no hover tooltip', await evaluate("document.querySelector('#mona-tooltip')?.hidden && [...document.querySelectorAll('.conversation-stat')].every(n=>!n.hasAttribute('data-tooltip'))"));
  for (const width of [390,320]) {
    await send('Emulation.setDeviceMetricsOverride', { width, height: 900, deviceScaleFactor: 1, mobile: false }); await sleep(200);
    check(`statistics remain below the input and inside the ${width}px viewport`, await evaluate(statisticsVisible));
    await shot(`composer-statistics-${width}`);
  }
  await send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false }); await sleep(150);
  check('composer statistics are independent of the ten-turn loaded history window', await evaluate("document.querySelectorAll('#timeline .turn').length===10 && document.querySelectorAll('.conversation-stat').length===3 && document.querySelector('[data-metric=tokens]').textContent.includes('14K')&&document.querySelector('[data-metric=tokens]').textContent.includes('80%')"));
  await click('[data-metric=context]');
  check('context gauge shows the estimated total and colored prompt/tool/message breakdown', await evaluate("(()=>{const p=document.querySelector('.conversation-stat-popover'),bar=p.querySelector('.conversation-context-bar');return !p.hidden&&p.classList.contains('is-context')&&p.textContent.includes('32K')&&bar.getAttribute('role')==='progressbar'&&Number(bar.getAttribute('aria-valuenow'))>0&&p.querySelectorAll('.context-segment').length===3&&p.textContent.includes('系统提示词')&&p.textContent.includes('工具定义')&&p.textContent.includes('对话消息')&&p.getBoundingClientRect().width<=264})()"));
  await click('[data-metric=performance]'); check('performance popover displays the execution metrics', await evaluate("(()=>{const p=document.querySelector('.conversation-stat-popover');return !p.hidden&&p.classList.contains('is-performance')&&p.textContent.includes('模型用时')&&p.textContent.includes('工具调用用时')&&p.textContent.includes('首 token 平均')&&p.textContent.includes('输出速度')})()"));
  await click('[data-metric=tokens]'); check('token popover shows exact input/cache/output breakdown and the real cache-hit rate', await evaluate("(()=>{const p=document.querySelector('.conversation-stat-popover'),r=p.getBoundingClientRect();return document.querySelectorAll('.conversation-stat[aria-expanded=true]').length===1&&p.querySelector('#conversation-stat-title').textContent==='Token 用量'&&p.querySelector('.conversation-stat-value').textContent==='14,300 tok'&&p.textContent.includes('缓存命中80%')&&p.textContent.includes('未缓存输入2,600 tok')&&p.textContent.includes('缓存读取10,400 tok')&&p.textContent.includes('输出1,300 tok')&&!p.textContent.includes('未提供')&&r.width>=300&&r.top<document.querySelector('[data-metric=tokens]').getBoundingClientRect().top})()"));
  await shot('dsh-composer-statistics');
  await send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
  check('Escape closes the token details and restores its trigger state', await evaluate("document.querySelector('.conversation-stat-popover').hidden && document.querySelector('[data-metric=tokens]').getAttribute('aria-expanded')==='false'"));
  await click('#new-chat');
  await pageWait("!document.querySelector('#welcome').hidden && !document.querySelector('#sessions-refresh').disabled", 'return to empty task');
  check('new task does not display the previous session usage as its own data', await evaluate("document.querySelector('#conversation-metrics').hidden && !document.querySelector('#timeline .turn')"));
  await click(`#sessions-list button[data-session-id="${a.id}"]`);
  await pageWait("document.querySelectorAll('#timeline .turn').length===10 && !document.querySelector('#conversation-metrics').hidden && document.querySelectorAll('.conversation-stat').length===3 && document.querySelector('[data-metric=tokens]').textContent.includes('14K')&&document.querySelector('[data-metric=tokens]').textContent.includes('80%') && !document.querySelector('#sessions-refresh').disabled", 'restored real statistics');
  check('returning to an existing session restores its real usage below the composer', await evaluate(statisticsVisible) && await evaluate("document.querySelector('[data-metric=tokens]').textContent.includes('14K')&&document.querySelector('[data-metric=tokens]').textContent.includes('80%')"));
  await evaluate("window.__latestTurn=[...document.querySelectorAll('#timeline .turn')].at(-1);window.__oldFirst=document.querySelector('#timeline .turn');document.querySelector('#main').scrollTop=0;document.querySelector('#main').dispatchEvent(new Event('scroll'))");
  await click('.history-pagination button'); await pageWait("document.querySelectorAll('#timeline .turn').length===13 && !document.querySelector('#sessions-refresh').disabled", 'continuous history prepend');
  check('older history is prepended without replacing existing turns', await evaluate("__latestTurn===[...document.querySelectorAll('#timeline .turn')].at(-1) && __oldFirst.isConnected"));
  await evaluate("document.querySelector('#main').scrollTop=0;document.querySelector('#main').dispatchEvent(new Event('scroll'))");
  await click(`#sessions-list button[data-session-id="${b.id}"]`);
  await pageWait(`location.hash.includes('${b.id}') && !document.querySelector('#sessions-refresh').disabled`, 'switch to independent conversation');
  await click(`#sessions-list button[data-session-id="${a.id}"]`);
  await pageWait("document.querySelectorAll('#timeline .turn').length===3 && !document.querySelector('#sessions-refresh').disabled", 'restore the older reading page');
  check('switching conversations restores the older history window and reading position', await evaluate("document.querySelector('#timeline .user-message').textContent==='主任务原文' && document.querySelector('#main').scrollTop<80"));
  check('task history buttons expose the actual visited-session stack', await evaluate("!document.querySelector('#task-back').disabled&&document.querySelector('#task-forward').disabled"));
  await click('#task-back'); await pageWait(`location.hash.includes('${b.id}') && !document.querySelector('#sessions-refresh').disabled`, 'task history back');
  check('back opens the prior visited task and enables forward', await evaluate("!document.querySelector('#task-forward').disabled"));
  await click('#task-forward'); await pageWait(`location.hash.includes('${a.id}') && !document.querySelector('#sessions-refresh').disabled`, 'task history forward');
  await evaluate("document.querySelector('#task-back').focus()");
  await send('Input.dispatchKeyEvent', { type: 'keyDown', key: '[', code: 'BracketLeft', windowsVirtualKeyCode: 219, modifiers: 2 });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key: '[', code: 'BracketLeft', windowsVirtualKeyCode: 219, modifiers: 2 });
  await pageWait(`location.hash.includes('${b.id}') && !document.querySelector('#sessions-refresh').disabled`, 'task history keyboard back');
  await send('Input.dispatchKeyEvent', { type: 'keyDown', key: ']', code: 'BracketRight', windowsVirtualKeyCode: 221, modifiers: 2 });
  await send('Input.dispatchKeyEvent', { type: 'keyUp', key: ']', code: 'BracketRight', windowsVirtualKeyCode: 221, modifiers: 2 });
  await pageWait(`location.hash.includes('${a.id}') && !document.querySelector('#sessions-refresh').disabled`, 'task history keyboard forward');
  check('Ctrl+[ and Ctrl+] replay the same task history', await evaluate("document.querySelector('#timeline .user-message').textContent==='主任务原文'"));
  await textButton('.history-pagination', '返回最新对话');
  await pageWait("document.querySelectorAll('#timeline .turn').length===10 && document.querySelector('#timeline .message-file-link[data-file-path=\"code.rs#L1\"]') && !document.querySelector('#sessions-refresh').disabled", 'return to latest after reading restoration');
  await click('#timeline .message-file-link[data-file-path="code.rs#L1"]');
  await pageWait("document.querySelector('#right-content .workspace-source-preview code')?.textContent.includes('MONA_CODE')", 'conversation link to right pane');
  check('code references open the correct file and line', await evaluate("document.querySelector('#right-content .is-referenced-line')?.dataset.line==='1'"));
  await click('.right-tab-close');
  await click('#workspace-files-open'); await pageWait("document.querySelector('.file-entry[data-path=\"notes.md\"]')", 'file tree');
  check('browsing files never replaces the left task navigation', await evaluate("document.querySelector('#sessions-list').getBoundingClientRect().width>0 && document.querySelector('#files-panel .workspace-files') && !document.querySelector('#chat-sidebar').classList.contains('files-mode')"));
  await click('.file-entry[data-path="docs"]'); await pageWait("document.querySelector('.file-entry[data-path=\"docs/nested.txt\"]')", 'lazy directory');
  check('right-side file page lazily expands a real directory', await evaluate("document.querySelector('.file-entry[data-path=\"docs\"]').closest('details').open"));
  await click('.file-entry[data-path="notes.md"]'); await pageWait("document.querySelector('#right-content .workspace-markdown-preview h1')?.textContent==='MONA_MARKDOWN'", 'markdown preview');
  await evaluate("window.__retained=document.querySelector('#right-content .workspace-markdown-preview')");
  await click('.file-entry[data-path="code.rs"]'); await pageWait("document.querySelector('#right-content .workspace-source-preview code')?.textContent.includes('MONA_CODE')", 'code preview');
  await pageWait("document.querySelector('#right-content .hljs-keyword')", 'highlight engine loaded');
  check('opening files produces deduplicated compact tabs and highlighted source', await evaluate("document.querySelectorAll('.right-tab').length===3 && document.querySelector('#right-content .hljs-keyword')!==null && [...document.querySelectorAll('.right-tab-wrap')].every(n=>n.getBoundingClientRect().height===28)"));
  await click('.right-tab[data-tab-id$=":notes.md"]'); check('switching tabs preserves the mounted preview DOM', await evaluate("window.__retained===document.querySelector('.workspace-markdown-preview') && !window.__retained.closest('[role=tabpanel]').hidden"));
  await textButton('#right-content > :not([hidden]) .right-file-toolbar', '源码'); check('Markdown source/preview switching is real', await evaluate("document.querySelector('#right-content > :not([hidden]) code').textContent.includes('# MONA_MARKDOWN')"));
  await click('.right-tab-wrap:last-child .right-tab', 'right'); await textButton('.right-tab-context', '关闭其他标签'); check('tab context menu closes only the requested peers', await evaluate("document.querySelectorAll('.right-tab').length===1"));
  await click('.right-overview-button'); await textButton('.right-overview-rows', 'notes.md'); await pageWait("document.querySelectorAll('.right-tab').length===2", 'reopen tab');
  check('tab overview reopens a recently closed file', true);
  await click('#right-fullscreen'); await click('#right-compare');
  check('two existing contents can be compared without remounting in a fullscreen workspace', await evaluate("document.querySelector('#files-panel').classList.contains('is-fullscreen') && document.querySelectorAll('#right-content > :not([hidden])').length===2"));
  await shot('dsh-compare'); await click('#right-fullscreen');
  check('exiting fullscreen preserves both panes and restores conversation interaction', await evaluate("!document.querySelector('#chat-workspace').inert && document.querySelectorAll('#right-content > :not([hidden])').length===2"));
  await click('.right-tab[data-tab-id$=":notes.md"]');  const modelCallsBeforeReload = modelRequests.length;
  await send('Page.reload');
  await pageWait("document.querySelectorAll('.right-tab').length===2 && !document.querySelector('#sessions-refresh').disabled && document.querySelector('#right-content > :not([hidden]) .workspace-source-preview')", 'file tabs restored after reload');
  await pageWait("!document.querySelector('#conversation-metrics').hidden&&document.querySelector('[data-metric=tokens]').textContent.includes('80%')", 'cached token statistics restored after reload');
  check('reload restores file tabs without rerunning terminals or models', modelRequests.length === modelCallsBeforeReload && await evaluate("!document.querySelector('.terminal-pane') && !document.querySelector('.side-conversation')"));
  check('reopened file preserves both panes and the source display mode across reload', await evaluate("document.querySelectorAll('#right-content > :not([hidden])').length===2 && [...document.querySelectorAll('#right-content > :not([hidden]) code')].some(n=>n.textContent.includes('# MONA_MARKDOWN'))"));
  await click('#right-compare');
  await click('#workspace-files-open'); await pageWait("document.querySelector('.file-entry[data-path=\"safe.html\"]')", 'file tree after restore');
  await click('.file-entry[data-path="long.txt"]'); await pageWait("!document.querySelector('#right-content > :not([hidden]) .right-file-more').hidden", 'partial text preview');
  await evaluate("window.__pagedCode=document.querySelector('#right-content > :not([hidden]) .message-code-block');window.__pagedText=__pagedCode.textContent.length");
  await click('#right-content > :not([hidden]) .right-file-more'); await pageWait("__pagedCode.textContent.length>__pagedText",'append text page');
  check('reading another file page preserves the code container rather than replacing it',await evaluate("__pagedCode===document.querySelector('#right-content > :not([hidden]) .message-code-block')"));
  await click('.file-entry[data-path="safe.html"]'); await pageWait("document.querySelector('#right-content > :not([hidden]) iframe')", 'safe HTML');
  check('HTML preview is isolated without script or network permissions', await evaluate("document.querySelector('#right-content > :not([hidden]) iframe').getAttribute('sandbox')==='' && !window.__unsafe"));
  await shot('files-light');
  // Close other file tabs first so the isolated-world check targets the active Office frame.
  await click('.right-tab-wrap:last-child .right-tab', 'right'); await textButton('.right-tab-context', '关闭所有标签');
  for (const extension of ['docx', 'xlsx', 'pptx']) {
    await click(`.file-entry[data-path="test.${extension}"]`);
    await pageWait("document.querySelector('#right-content iframe[data-rendered=true]')", extension + ' actual renderer', 55000);
    const text = await officeText(extension);
    check(extension + ' uses the real local document engine in an opaque sandbox', extension === 'xlsx' ? text.includes('销售') && text.includes('汇总') : text.includes(extension === 'docx' ? 'MONA_DOCX_REFERENCE' : 'MONA_PPTX_REFERENCE'));
    await shot(extension + '-preview'); await click('.right-tab-wrap:last-child .right-tab-close');
  }
  await click('#right-add'); await textButton('#right-add-menu', '审查'); await pageWait("document.querySelector('.review-entry')", 'review UI'); await textButton('.review-files', 'notes.mdM');
  await pageWait("document.querySelector('.review-detail').textContent.includes('MODIFIED_FOR_REVIEW')", 'patch rendered'); check('review tab is connected to actual Git diffs', true);
  await textButton('.review-pane .right-file-toolbar','分栏'); check('side-by-side review has paired original and revised columns', await evaluate("document.querySelector('.review-detail .message-diff.is-split .diff-columns') && document.querySelector('.review-detail .diff-line-number')"));
  await click('#right-add'); await textButton('#right-add-menu', '终端'); await pageWait("document.querySelector('.xterm-screen') && document.querySelector('.terminal-status').textContent===''", 'terminal UI');
  check('terminal tab mounts xterm with the native PTY', await evaluate("document.querySelector('.terminal-viewport .xterm-screen').getBoundingClientRect().height>0"));
  await shot('terminal-and-tabs'); await click('.right-tab-wrap:last-child .right-tab-close');
  await click('#right-add'); await textButton('#right-add-menu', '侧边对话');
  await pageWait("document.querySelector('.side-composer textarea')", 'side conversation form');
  await evaluate("document.querySelector('.side-composer textarea').value='旁支界面验收'"); await click('.side-composer button[type=submit]');
  await pageWait("document.querySelector('.side-timeline').textContent.includes('已完成：旁支界面验收') && document.querySelector('.side-composer .secondary').hidden", 'real side UI completion');
  check('side conversation UI calls the real host without changing main turns', (await api(`/api/sessions/${a.id}`)).session.turn_count === countBefore);
  await shot('side-conversation'); await click('.right-tab-wrap:last-child .right-tab-close');
  const originalWidth = await evaluate("document.querySelector('#files-panel').getBoundingClientRect().width");
  await evaluate("document.querySelector('#right-resizer').focus()"); await send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'ArrowLeft', code: 'ArrowLeft', windowsVirtualKeyCode: 37 }); await send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'ArrowLeft', code: 'ArrowLeft', windowsVirtualKeyCode: 37 });
  check('right pane resizes with the keyboard while preserving tabs', await evaluate(`document.querySelector('#files-panel').getBoundingClientRect().width>${originalWidth}`));
  await click('#files-close'); await click('#files-toggle'); check('collapse keeps the review tab and its contents', await evaluate("document.querySelector('.review-detail').textContent.includes('MODIFIED_FOR_REVIEW')"));
  await send('Emulation.setDeviceMetricsOverride', { width: 390, height: 900, deviceScaleFactor: 1, mobile: false });
  await pageWait("document.querySelector('#files-panel').getAttribute('aria-modal')==='true'", 'mobile right pane layout');
  check('narrow screens use a bounded independent panel', await evaluate("document.querySelector('#files-panel').getAttribute('aria-modal')==='true' && document.documentElement.scrollWidth<=390")); await shot('right-pane-mobile');
  await send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false });
  const desktopConfig = JSON.stringify({ endpoint: base, token, open_with: ['explorer', 'terminal'] });
  await send('Page.addScriptToEvaluateOnNewDocument', { source: `window.__desktopOpens=[];window.__MONA_TAURI__={invoke:async(command,args)=>{if(command==='desktop_config')return ${desktopConfig};if(command==='desktop_open_workspace'){if(window.__desktopFailNext){window.__desktopFailNext=false;throw '系统应用启动失败';}window.__desktopOpens.push(args);return null;}throw Error('unexpected native command '+command);}};` });
  await send('Page.navigate', { url: `${origin}/?monaDesktop=1#session=${a.id}` });
  await pageWait("document.querySelector('#header-open-group')&&!document.querySelector('#header-open-group').hidden&&!document.querySelector('#sessions-refresh').disabled", 'desktop task header');
  await click('#header-open-options');
  check('desktop dropdown shows system Explorer and terminal with the reference selected row', await evaluate("(()=>{const m=document.querySelector('#header-open-menu'),items=[...m.querySelectorAll('[role=menuitemradio]')];return !m.hidden&&items.length===2&&items[0].textContent.trim()==='资源管理器✓'&&items[0].getAttribute('aria-checked')==='true'&&items[1].textContent.trim()==='终端'&&getComputedStyle(items[0]).backgroundColor!==getComputedStyle(items[1]).backgroundColor})()"));
  await shot('topbar-open-with-menu');
  await textButton('#header-open-menu', '终端');
  await pageWait("window.__desktopOpens.length===1", 'desktop terminal command');
  check('terminal choice opens the current session workspace through the desktop command', await evaluate(`window.__desktopOpens[0].sessionId==='${a.id}'&&window.__desktopOpens[0].application==='terminal'&&localStorage.getItem('mona.web.header.open-mode.v1')==='terminal'`));
  await click('#header-open-workspace'); await pageWait("window.__desktopOpens.length===2", 'selected desktop command');
  check('main split button repeats the selected system application', await evaluate("window.__desktopOpens[1].application==='terminal'"));
  await click('#header-open-options'); await textButton('#header-open-menu', '资源管理器');
  await pageWait("window.__desktopOpens.length===3", 'desktop Explorer command');
  check('Explorer choice targets the same current workspace', await evaluate(`window.__desktopOpens[2].sessionId==='${a.id}'&&window.__desktopOpens[2].application==='explorer'`));
  await evaluate("window.__desktopFailNext=true"); await click('#header-open-workspace');
  await pageWait("document.querySelector('#projects-notice').textContent.includes('系统应用启动失败')", 'desktop launch failure');
  check('a rejected native launch reports the error without claiming success', await evaluate("window.__desktopOpens.length===3&&document.querySelector('#projects-notice').textContent==='系统应用启动失败'"));
  check('no unhandled browser exceptions', errors.length === 0);
  await writeFile(join(reportDir, 'report.json'), JSON.stringify({ passed: true, checks, modelCalls: modelRequests.length, errors }, null, 2)); console.log(`PASS ${checks.length} real application assertions`);
} catch (error) {
  console.error(error); console.error(logs); console.error(errors);
  if (socket?.readyState === WebSocket.OPEN) { console.error(await evaluate("document.body.innerText.slice(-5000)").catch(() => 'page unavailable')); await shot('failure').catch(() => {}); }
  await writeFile(join(reportDir, 'report.json'), JSON.stringify({ passed: false, checks, error: String(error), errors }, null, 2)); process.exitCode = 1;
} finally {
  if (socket?.readyState === WebSocket.OPEN) { await send('Browser.close').catch(() => {}); socket.close(); }
  for (const item of pending.values()) clearTimeout(item.timer); pending.clear(); await stop(browser); await stop(host);
  ui?.closeAllConnections(); if (ui) await new Promise(r => ui.close(r)); model.closeAllConnections(); await new Promise(r => model.close(r));
  await rm(scratch, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 }).catch(() => {});
}
