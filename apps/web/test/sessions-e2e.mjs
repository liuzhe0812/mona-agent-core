// Real Rust host + formal UI + controlled local model. All state, ports and child processes are isolated.
// Run after cargo build -p server: node apps/web/test/sessions-e2e.mjs
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, mkdir, readFile, writeFile, rm, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startUiServer } from '../../../scripts/dev-web.mjs';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const output = resolve(process.env.MONA_TEST_REPORT_DIR || join(root, 'target/browser-reports/sessions'));
const scratch = await mkdtemp(join(tmpdir(), 'mona-sessions-e2e-'));
const workspace = join(scratch, 'workspace'); const state = join(scratch, 'state');
await mkdir(workspace); await mkdir(state); await mkdir(output, { recursive: true });
await writeFile(join(workspace, 'fixture.txt'), 'SESSION_TEST_FILE_31415\n');
const token = 'isolated-local-session-e2e-token-123456789';
const requests = []; const errors = []; const checks = [];
let host, browser, ui, socket, apiPort;
const sleep = ms => new Promise(r => setTimeout(r, ms));
async function waitFor(fn, name, timeout = 15000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) { if (await fn()) return; await sleep(50); }
  throw new Error(`Timed out: ${name}`);
}
async function stop(child) {
  if (!child || child.exitCode != null) return;
  const closed = once(child, 'exit'); child.kill('SIGKILL');
  await Promise.race([closed, sleep(5000)]);
}
function check(name, condition) { assert.ok(condition, name); checks.push(name); console.log(`PASS ${name}`); }
const fixture = createServer(async (req, res) => {
  try {
    let raw = ''; for await (const chunk of req) { raw += chunk; if (raw.length > 4 * 1024 * 1024) throw new Error('fixture request cap'); }
    const body = JSON.parse(raw); requests.push(body);
    const users = body.messages.filter(m => m.role === 'user').map(m => String(m.content));
    const latest = users.at(-1) || '';
    res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-store' });
    const frame = value => res.write(`data: ${JSON.stringify(value)}\n\n`);
    if (latest === 'HOLD_FOR_RESTART') {
      frame({ choices: [{ index: 0, delta: { content: '本地测试：等待进程重启。' }, finish_reason: null }] });
      return;
    }
    if (latest.includes('fixture.txt') && body.messages.at(-1).role === 'user') {
      // The history fixture is explicit test-owned input, not the ordinary session's cwd.
      // Relative per-session writes and Shell cwd are covered in workspaces-e2e.mjs.
      frame({ choices: [{ index: 0, delta: { tool_calls: [{ index: 0, id: `read-${requests.length}`, type: 'function', function: { name: 'read', arguments: JSON.stringify({ path: join(workspace, 'fixture.txt') }) } }] }, finish_reason: 'tool_calls' }] });
    } else {
      frame({ choices: [{ index: 0, delta: { content: `本地测试回复：${users.join(' / ')}` }, finish_reason: 'stop' }] });
    }
    frame({ choices: [], usage: { prompt_tokens: 20, completion_tokens: 10 } }); res.end('data: [DONE]\n\n');
  } catch (e) { errors.push(`fixture: ${e.message}`); res.destroy(); }
});
await new Promise(r => fixture.listen(0, '127.0.0.1', r));
const modelEndpoint = `http://127.0.0.1:${fixture.address().port}/chat/completions`;
const reservation = createServer(); await new Promise(r => reservation.listen(0, '127.0.0.1', r));
apiPort = reservation.address().port; await new Promise(r => reservation.close(r));
const endpoint = `http://127.0.0.1:${apiPort}`;
async function api(path, body) {
  const response = await fetch(endpoint + path, { method: body === undefined ? 'GET' : 'POST',
    headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(5000) });
  if (!response.ok) throw new Error(`API ${path}: ${response.status} ${await response.text()}`);
  return response.status === 204 ? null : response.json();
}
let hostLog = '';
async function startHost(origin) {
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('AGENT_') && !key.startsWith('MONA_DEV_')));
  Object.assign(env, { AGENT_SERVER_ADDR: `127.0.0.1:${apiPort}`, AGENT_SERVER_TOKEN: token, AGENT_UI_ORIGIN: origin,
    AGENT_MODEL_ENDPOINT: modelEndpoint, AGENT_MODEL_NAME: 'local-session-fixture', AGENT_ALLOW_HTTP_LOOPBACK: '1',
    AGENT_MODEL_MANAGEMENT: '0', AGENT_SKILLS: '0', AGENT_SPILL: '0', AGENT_COMPACTION: '0',
    AGENT_WORKSPACE_DIR: workspace, AGENT_SESSIONS_DIR: join(state,'sessions'), AGENT_CAPABILITY_STATE_PATH: join(state,'capabilities.json'),
    MONA_DEV_STATE_DIR: state, LOCALAPPDATA: state, XDG_STATE_HOME: state,
  });
  const binary = resolve(process.env.MONA_TEST_SERVER || join(root,'target','debug',process.platform === 'win32' ? 'server.exe' : 'server'));
  await access(binary); host = spawn(binary, [], { cwd: workspace, env, stdio: ['ignore','pipe','pipe'] });
  host.stderr.on('data', chunk => { hostLog = (hostLog + chunk).slice(-8000); });
  host.on('error', e => errors.push(`host: ${e.message}`));
  await waitFor(async () => {
    if (host.exitCode != null) throw new Error(`host exited: ${hostLog}`);
    try { return (await api('/v1/info')).protocol_version === 2; } catch { return false; }
  }, 'isolated host ready');
}
let nextId = 0; const pending = new Map(); let dialogText;
function send(method, params = {}) {
  return new Promise((resolve, reject) => {
    const id = ++nextId; const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout ${method}`)); }, 15000);
    pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params }));
  });
}
async function evaluate(expression) {
  const result = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description || 'page script failed');
  return result.result.value;
}
async function waitPage(expression, name) { await waitFor(() => evaluate(expression), name); }
async function submit(text) {
  await evaluate(`(() => {const p=document.querySelector('#prompt');p.value=${JSON.stringify(text)};p.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  await waitPage("!document.querySelector('#send').disabled", 'composer ready');
  await evaluate("document.querySelector('#send').click()");
}
async function saved(turnCount) {
  await waitFor(async () => {
    const page = await api('/api/sessions'); const entry = page.sessions[0];
    return entry?.turn_count === turnCount && entry?.status === 'completed';
  }, `durable turn ${turnCount}`);
  await waitPage("document.querySelector('#cancel').hidden && !document.querySelector('#sessions-refresh').disabled", 'UI settled');
}
async function screenshot(name) {
  const shot = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  await writeFile(join(output,name + '.png'), Buffer.from(shot.data, 'base64'));
}
// Sidebar states are a few pixels tall, so the evidence is captured magnified.
const first = "document.querySelector('#sessions-list .session-item')";
async function zoom(name, selector = first) {
  const clip = await evaluate(`(() => {const box=${selector}.getBoundingClientRect();return {x:0,y:Math.max(0,Math.round(box.y-88)),width:264,height:Math.round(box.height)+104};})()`);
  const shot = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false, clip: { ...clip, scale: 3 } });
  await writeFile(join(output, `${name}.png`), Buffer.from(shot.data, 'base64'));
}
try {
  ui = await startUiServer({ host: '127.0.0.1', port: 0, endpoint, token });
  const origin = `http://127.0.0.1:${ui.address().port}`; await startHost(origin);
  const executable = process.env.BROWSER_BIN || (process.platform === 'win32' ? 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe' : 'chromium');
  const profile = join(scratch,'browser');
  const debugReservation = createServer();
  await new Promise((resolve, reject) => {
    debugReservation.once('error', reject);
    debugReservation.listen(0, '127.0.0.1', () => { debugReservation.off('error', reject); resolve(); });
  });
  const port = debugReservation.address().port;
  await new Promise(resolve => debugReservation.close(resolve));
  browser = spawn(executable, ['--headless=new','--disable-gpu','--no-first-run','--no-default-browser-check',
    '--edge-skip-compat-layer-relaunch',`--remote-debugging-port=${port}`,`--user-data-dir=${profile}`,'--window-size=1440,1000','about:blank'], { stdio:'ignore' });
  browser.on('error', e => errors.push(`browser: ${e.message}`));
  await waitFor(async () => {
    if (browser.exitCode != null) throw new Error(`browser exited before DevTools became ready: ${browser.exitCode}`);
    try { return (await fetch(`http://127.0.0.1:${port}/json/list`, { signal: AbortSignal.timeout(500) })).ok; } catch { return false; }
  }, 'browser ready');
  const page = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find(p => p.type === 'page');
  socket = new WebSocket(page.webSocketDebuggerUrl); await once(socket,'open');
  socket.addEventListener('message', event => {
    const value = JSON.parse(event.data); const request = pending.get(value.id);
    if (request) { pending.delete(value.id); clearTimeout(request.timer); if (value.error) request.reject(new Error(JSON.stringify(value.error))); else request.resolve(value.result); }
    if (value.method === 'Page.javascriptDialogOpening') void send('Page.handleJavaScriptDialog', { accept: true, ...(dialogText == null ? {} : { promptText: dialogText }) });
    if (value.method === 'Runtime.exceptionThrown') errors.push(value.params.exceptionDetails.exception?.description || 'browser exception');
  });
  await send('Page.enable'); await send('Runtime.enable');
  await send('Page.addScriptToEvaluateOnNewDocument', { source: "window.__sessionErrors=[];window.addEventListener('error',e=>window.__sessionErrors.push(String(e.message)));window.addEventListener('unhandledrejection',e=>window.__sessionErrors.push(String(e.reason)))" });
  await send('Page.navigate',{url:origin});
  await waitPage("document.querySelector('#sessions-notice')?.textContent.includes('已保存在宿主本地')", 'session UI connected');
  await submit('请读取 fixture.txt 并记住编号31415'); await saved(1);
  check('formal UI creates a locally saved conversation', (await api('/api/sessions')).sessions.length === 1);
  check('formal read tool returned the isolated fixture content', requests.some(r => r.messages.some(m => m.role === 'tool' && String(m.content).includes('SESSION_TEST_FILE_31415'))));
  const sessionId = (await api('/api/sessions')).sessions[0].id;
  check('session navigation preserves the reloadable entry URL despite HTML base', await evaluate("location.pathname==='/' && location.hash.startsWith('#session=')"));
  await stop(host); await startHost(origin); await send('Page.reload',{ignoreCache:true});
  await waitPage("document.querySelectorAll('#timeline .turn').length===1 && document.querySelector('#timeline').textContent.includes('31415') && !document.querySelector('#sessions-refresh').disabled", 'history after real restart');
  check('history and tool results are visible after process restart', (await evaluate("document.querySelector('#timeline').textContent")).includes('SESSION_TEST_FILE_31415'));
  await submit('继续：之前的编号是什么？'); await saved(2);
  check('continuation sends original user, assistant and tool-result history', requests.at(-1).messages.some(m => m.role === 'tool' && String(m.content).includes('SESSION_TEST_FILE_31415')) && requests.at(-1).messages.filter(m=>m.role==='user').length===2);
  dialogText = '重启验证会话';
  // Rename now lives in the row menu; the topbar buttons were removed with the sidebar rework.
  await evaluate(`(() => {const item=document.querySelectorAll('#sessions-list .session-item')[0];const box=item.getBoundingClientRect();
    item.dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true,clientX:Math.round(box.x+40),clientY:Math.round(box.y+8)}));})()`);
  await waitPage("document.querySelectorAll('#session-menu .session-menu-item')[0] !== undefined", 'row menu for rename');
  await evaluate("document.querySelectorAll('#session-menu .session-menu-item')[0].click()");
  await waitPage("document.querySelector('#thread-title').textContent==='重启验证会话'", 'rename saved'); dialogText = undefined;
  check('rename is durable', (await api('/api/sessions')).sessions[0].title === '重启验证会话');
  await screenshot('desktop-saved-conversation');
  // Task list parity with the reference: title + relative stamp, hover reveals the row action, menu on right click.
  const row = "document.querySelector('#sessions-list .session-item')";
  check('a task row shows its title and a relative stamp',
    await evaluate(`(() => {const item=${row};return item.querySelector('.session-title').textContent==='重启验证会话' && /^(刚刚|\\d+分钟|\\d+小时|\\d+天)/.test(item.querySelector('.session-meta').textContent);})()`));
  check('the row action stays hidden until the row is hovered',
    await evaluate(`getComputedStyle(${row}.querySelector('.session-action')).opacity === '0'`));
  const spot = await evaluate(`(() => {const box=${row}.getBoundingClientRect();return {x:Math.round(box.right-40),y:Math.round(box.y+box.height/2)};})()`);
  check('a saved task keeps the glyph slot and paints no spinner', await evaluate(`(() => {
    const item = ${row};
    return item.querySelector('.session-glyph > .session-spinner') !== null
      && getComputedStyle(item.querySelector('.session-spinner')).visibility === 'hidden';
  })()`));
  const savedTitleLeft = await evaluate(`${row}.querySelector('.session-title').getBoundingClientRect().left`);
  await zoom('task-row-resting-zoom');
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: spot.x, y: spot.y });
  await waitPage(`getComputedStyle(${row}.querySelector('.session-action')).opacity === '1'`, 'row action revealed');
  check('hovering a task row reveals its action in place of the stamp',
    await evaluate(`getComputedStyle(${row}.querySelector('.session-meta')).visibility === 'hidden'`));
  await screenshot('task-row-hover');
  await zoom('task-row-hover-zoom');
  const menu = JSON.parse(await evaluate(`(() => {const item=${row};const box=item.getBoundingClientRect();
    item.dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true,clientX:Math.round(box.x+40),clientY:Math.round(box.y+8)}));
    const panel=document.querySelector('#session-menu');const items=[...panel.querySelectorAll('.session-menu-item')];
    return JSON.stringify({hidden:panel.hidden,role:panel.getAttribute('role'),labels:items.map(node=>node.textContent),danger:items.at(-1).classList.contains('is-danger')});})()`));
  check('right click opens the task menu whose last entry is the red 删除任务',
    !menu.hidden && menu.role === 'menu' && menu.labels.length === 5 && menu.labels.at(-1) === '删除任务' && menu.danger);
  await screenshot('task-menu');
  await evaluate("document.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}))");
  await waitPage("document.querySelector('#session-menu').hidden", 'menu closed by Escape');
  check('Escape closes the task menu without touching the task', true);
  const headerSpot = await evaluate("(() => {const box=document.querySelector('.sessions-heading').getBoundingClientRect();return {x:Math.round(box.right-26),y:Math.round(box.y+box.height/2)};})()");
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: headerSpot.x, y: headerSpot.y });
  await waitPage("getComputedStyle(document.querySelector('.sessions-heading-actions')).opacity === '1'", 'section actions revealed');
  await zoom('task-header-zoom');
  // At rest the right slot carries the stamp; the row actions replace it only on hover.
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 720, y: 240 });
  await sleep(120);
  check('the stamp and the row actions overlap in one slot exclusively',
    await evaluate(`(() => {const item=${row};
      const stamp = item.querySelector('.session-meta').getBoundingClientRect();
      const more = item.querySelector('.action-more').getBoundingClientRect();
      const pin = item.querySelector('.action-pin').getBoundingClientRect();
      const title = item.querySelector('.session-title').getBoundingClientRect();
      return getComputedStyle(item.querySelector('.session-meta')).visibility === 'visible'
        && getComputedStyle(item.querySelector('.action-more')).opacity === '0'
        && Math.abs(stamp.right - more.right) < 1 && pin.right <= more.left && pin.left > title.right - 1;})()`));
  // Inline row actions are pin and the menu button; every other operation stays in that menu.
  const actionSpot = await evaluate(`(() => {const box=${row}.getBoundingClientRect();return {x:Math.round(box.right-24),y:Math.round(box.y+box.height/2)};})()`);
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: actionSpot.x, y: actionSpot.y });
  await waitPage(`getComputedStyle(${row}.querySelector('.action-pin')).opacity === '1'`, 'row actions revealed');
  check('a task row offers pin and the menu button',
    await evaluate(`(() => {
      const icons = [...${row}.querySelectorAll('.session-action')];
      return icons.length === 2 && icons[0].classList.contains('action-pin')
        && icons[1].classList.contains('action-more') && icons[1].getAttribute('aria-haspopup') === 'menu'
        && getComputedStyle(${row}.querySelector('.session-meta')).visibility === 'hidden';
    })()`));
  await zoom('task-row-actions-zoom');
  await evaluate(`${row}.querySelector('.action-pin').click()`);
  await waitFor(async () => (await api('/api/sessions')).sessions[0].pinned === true, 'pin persisted');
  await waitPage(`document.querySelectorAll('#sessions-list .session-item')[0].querySelector('.action-pin').getAttribute('aria-pressed') === 'true'`, 'pinned row marked');
  check('pinning persists in the host and marks the pin as pressed',
    await evaluate(`(() => {const pin=document.querySelector('#sessions-list .session-item .action-pin');
      return pin.getAttribute('aria-pressed') === 'true' && pin.dataset.tooltip === '取消置顶';})()`));
  await evaluate(`${row}.querySelector('.action-pin').click()`);
  await waitFor(async () => (await api('/api/sessions')).sessions[0].pinned === false, 'unpin persisted');
  await waitPage("document.querySelector('#sessions-list .session-item .action-pin').getAttribute('aria-pressed') === 'false'", 'unpin shown');
  check('unpinning is durable', true);
  // Archiving hides the task from the active list; the section menu brings the archived view back.
  await evaluate(`${row}.querySelector('.action-more').click()`);
  await waitPage("document.querySelectorAll('#session-menu .session-menu-item').length === 5", 'row menu');
  await evaluate("document.querySelectorAll('#session-menu .session-menu-item')[2].click()");
  await waitFor(async () => (await api('/api/sessions')).sessions.length === 0
    && (await api('/api/sessions?archived=true')).sessions.length === 1, 'archive persisted');
  await waitPage("document.querySelector('#sessions-list .session-row') === null", 'archived task hidden');
  check('archiving reports where the task went', await evaluate("document.querySelector('#sessions-notice').textContent.includes('已归档')"));
  await evaluate("document.querySelector('#sessions-options').click()");
  await waitPage("!document.querySelector('#sessions-options-menu').hidden", 'section menu');
  check('the section menu carries refresh and the archived view',
    await evaluate("document.querySelector('#sessions-archived').textContent === '显示已归档任务' && document.querySelector('#sessions-options-menu #sessions-refresh') !== null"));
  await evaluate("document.querySelector('#sessions-archived').click()");
  await waitPage("document.querySelector('#sessions-list .session-row') !== null", 'archived view listed');
  check('the archived view lists the task and its menu offers to restore it',
    await evaluate("document.querySelector('#sessions-archived').textContent === '隐藏已归档任务' && document.querySelector('#sessions-list .session-item') !== null"));
  await evaluate("document.querySelector('#sessions-list .session-item .action-more').click()");
  await waitPage("document.querySelectorAll('#session-menu .session-menu-item')[2].textContent.includes('取消归档')", 'unarchive entry');
  await evaluate("document.querySelectorAll('#session-menu .session-menu-item')[2].click()");
  await waitFor(async () => (await api('/api/sessions')).sessions.length === 1, 'unarchive persisted');
  await evaluate("document.querySelector('#sessions-options').click()"); await waitPage("!document.querySelector('#sessions-options-menu').hidden", 'section menu again');
  await evaluate("document.querySelector('#sessions-archived').click()");
  await waitPage("document.querySelector('#sessions-list .session-row') !== null", 'active view restored');
  check('the unarchived task is back in the active list',
    await evaluate("document.querySelector('#sessions-archived').textContent === '显示已归档任务' && document.querySelector('#sessions-list .session-row') !== null"));
  // The unread mark lives in the context menu and clears when the task is opened.
  await evaluate(`(() => {
    const item = ${row};
    const box = item.getBoundingClientRect();
    item.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: Math.round(box.x + 40), clientY: Math.round(box.y + 8) }));
  })()`);
  await waitPage("document.querySelectorAll('#session-menu .session-menu-item').length === 5", 'task menu with unread');
  check('the task menu lists rename, pin, archive, the unread mark and the red delete, each with its icon',
    await evaluate(`(() => {
      const items = [...document.querySelectorAll('#session-menu .session-menu-item')];
      const labels = items.map(node => node.querySelector('span').textContent);
      return JSON.stringify(labels) === JSON.stringify(['重命名任务', '置顶聊天', '归档', '标记为未读', '删除任务'])
        && items.every(node => node.querySelector('svg.line-icon') !== null)
        && items[4].classList.contains('is-danger');
    })()`));
  await evaluate("document.querySelectorAll('#session-menu .session-menu-item')[3].click()");
  await waitFor(async () => (await api('/api/sessions')).sessions[0].unread === true, 'unread persisted');
  await waitPage("document.querySelector('#sessions-list .session-item.is-unread') !== null", 'unread dot');
  check('an unread task shows its dot without hover',
    await evaluate("getComputedStyle(document.querySelector('#sessions-list .session-item.is-unread .session-unread')).visibility === 'visible'"));
  await evaluate(`${row}.querySelector('.session-row').click()`);
  await waitFor(async () => (await api('/api/sessions')).sessions[0].unread === false, 'unread cleared by opening');
  check('opening the task clears the unread mark', await evaluate("document.querySelector('#sessions-list .session-item.is-unread') === null"));
  await waitPage("!document.querySelector('#sessions-refresh').disabled", 'UI settled after opening the task');
  // The topbar theme button moved into Settings → 外观, so this check drives the same theme attribute.
  await evaluate("document.documentElement.dataset.theme='dark'");
  check('dark theme retains readable conversation controls', await evaluate("document.documentElement.dataset.theme==='dark' && !document.querySelector('#sessions-refresh').disabled && document.querySelectorAll('.session-row').length===1"));
  await screenshot('dark-saved-conversation');
  await evaluate("document.documentElement.dataset.theme='light'");
  await send('Emulation.setDeviceMetricsOverride',{width:390,height:900,deviceScaleFactor:1,mobile:true});
  check('mobile workspace has no horizontal overflow', await evaluate("document.querySelector('#chat-workspace').scrollWidth<=document.querySelector('#chat-workspace').clientWidth+1"));
  await screenshot('mobile-saved-conversation'); await send('Emulation.setDeviceMetricsOverride',{width:1440,height:1000,deviceScaleFactor:1,mobile:false});
  await submit('HOLD_FOR_RESTART');
  await waitPage("!document.querySelector('#cancel').hidden", 'active streamed run');
  await waitFor(async()=> (await api('/api/sessions')).sessions[0].status==='running','durable active turn');
  await waitPage(`getComputedStyle(${row}.querySelector('.session-spinner')).visibility === 'visible'`, 'running task spinner');
  check('a running task shows the spinner in front of its title while its stamp stays relative',
    await evaluate(`(() => {
      const row = ${row}.querySelector('.session-row');
      const spinner = row.querySelector('.session-spinner');
      return row.firstElementChild.classList.contains('session-glyph')
        && spinner.getAttribute('aria-label') === '运行中'
        && spinner.getBoundingClientRect().left < row.querySelector('.session-title').getBoundingClientRect().left
        && /^(刚刚|\\d+分钟|\\d+小时|\\d+天)/.test(${row}.querySelector('.session-meta').textContent)
        && getComputedStyle(spinner).animationName === 'session-spin';
    })()`));
  const spun = await evaluate(`getComputedStyle(${row}.querySelector('.session-spinner')).transform`);
  await sleep(320);
  check('the running spinner actually turns', spun !== await evaluate(`getComputedStyle(${row}.querySelector('.session-spinner')).transform`));
  check('the title keeps its place while the spinner shows', await evaluate(`Math.abs(${row}.querySelector('.session-title').getBoundingClientRect().left - ${savedTitleLeft}) < 0.5`));
  await zoom('task-running-zoom');
  const beforeRestart = requests.length;
  await stop(host); await startHost(origin); await send('Page.reload',{ignoreCache:true});
  await waitPage("document.querySelector('#sessions-notice').textContent.includes('中断') && !document.querySelector('#sessions-refresh').disabled", 'interrupted run restored');
  await sleep(300);
  check('restart marks interruption and never automatically reruns the model', requests.length===beforeRestart && (await api('/api/sessions')).sessions[0].status==='interrupted');
  check('the title keeps its place when the task settles', await evaluate(`Math.abs(${row}.querySelector('.session-title').getBoundingClientRect().left - ${savedTitleLeft}) < 0.5`));
  await screenshot('interrupted-conversation');
  const old = await api(`/api/sessions/${sessionId}`); const last = old.turns.at(-1);
  const repeated = await api(`/api/sessions/${sessionId}/turns`,{request_id:last.id,revision:0,prompt:last.prompt});
  check('durable request dedup survives process death', repeated.reused && requests.length===beforeRestart);
  await evaluate("document.querySelector('#new-chat').click()");
  await submit('这是一个隔离的新会话'); await saved(1);
  check('new conversations do not inherit another conversation history', requests.at(-1).messages.filter(m=>m.role==='user').length===1 && !JSON.stringify(requests.at(-1).messages).includes('31415'));
  await evaluate(`(() => {
    const item = [...document.querySelectorAll('#sessions-list .session-item')].find(node => node.querySelector('.session-row[aria-current="page"]'));
    const box = item.getBoundingClientRect();
    item.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: Math.round(box.x + 40), clientY: Math.round(box.y + 8) }));
  })()`);
  await waitPage("document.querySelectorAll('#session-menu .session-menu-item')[4] !== undefined", 'row menu for delete');
  await evaluate("document.querySelectorAll('#session-menu .session-menu-item')[4].click()");
  await waitFor(async()=> (await api('/api/sessions')).sessions.length===1,'delete persisted');
  check('deletion removes only the selected conversation', (await api('/api/sessions')).sessions[0].id===sessionId);
  await evaluate("document.querySelector('#search-button').click()");
  await waitPage("document.querySelector('#search-dialog').open && document.querySelectorAll('#search-results .search-result').length===1", 'task search dialog');
  check('the brand search button opens the task search dialog over the loaded list', true);
  await evaluate("(()=>{const s=document.querySelector('#sessions-search');s.value='NO_MATCH_SESSION';s.dispatchEvent(new Event('input',{bubbles:true}));})()");
  await waitPage("document.querySelector('#search-results').textContent.includes('没有匹配的任务')", 'empty task search');
  check('task search reports no match independently of the sidebar list', await evaluate("document.querySelectorAll('#sessions-list .session-row').length===1"));
  await evaluate("(()=>{const s=document.querySelector('#sessions-search');s.value='重启验证';s.dispatchEvent(new Event('input',{bubbles:true}));})()");
  await waitPage("document.querySelectorAll('#search-results .search-result').length===1", 'task search');
  check('task search finds the matching persisted task', (await evaluate("document.querySelector('#search-results .search-result span').textContent")).includes('重启验证'));
  await evaluate("document.querySelector('#search-dialog').close()");
  check('browser has no unhandled exceptions', (await evaluate('window.__sessionErrors')).length===0 && errors.length===0);
  await writeFile(join(output,'result.json'),JSON.stringify({passed:true,checks,model_requests:requests.length,isolated:true,real_provider:false},null,2));
  console.log(`PASS ${checks.length} end-to-end assertions; screenshots in ${output}`);
} catch (error) {
  console.error(error); console.error(hostLog);
  if (socket?.readyState===WebSocket.OPEN) {
    console.error(await evaluate("JSON.stringify({url:location.href,body:document.body?.innerText.slice(0,4000),notice:document.querySelector('#sessions-notice')?.textContent,errors:window.__sessionErrors})").catch(()=> 'no page diagnostics'));
    await screenshot('failure').catch(()=>{});
  }
  await writeFile(join(output,'result.json'),JSON.stringify({passed:false,checks,error:String(error),errors},null,2));
  process.exitCode=1;
} finally {
  if (socket?.readyState===WebSocket.OPEN) { await send('Browser.close').catch(()=>{}); socket.close(); }
  for (const entry of pending.values()) { clearTimeout(entry.timer); entry.reject(new Error('test cleanup')); } pending.clear();
  await stop(browser); await stop(host);
  ui?.closeAllConnections?.(); if (ui) await new Promise(r=>ui.close(r));
  fixture.closeAllConnections(); await new Promise(r=>fixture.close(r));
  await rm(scratch,{recursive:true,force:true,maxRetries:10,retryDelay:200});
}
