// Formal Web + real Rust host + native OS sandbox + real files, using only a deterministic model.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { access, mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startUiServer } from '../../../scripts/dev-web.mjs';
const root = fileURLToPath(new URL('../../../', import.meta.url));
const report = join(root, 'target/sandbox-validation/browser'); await mkdir(report, { recursive: true });
// The native ACL backend needs WRITE_DAC + WRITE_OWNER. Data-volume Modify ACLs
// intentionally fail closed; never grant extra host rights just to make a test pass.
const testRoot = process.platform === 'win32'
  ? join(process.env.LOCALAPPDATA || (() => { throw Error('LOCALAPPDATA is required for native sandbox tests'); })(), 'mona-agent-core', 'sandbox-test-workspaces')
  : report;
await mkdir(testRoot, { recursive: true });
const scratch = await mkdtemp(join(testRoot, 'isolated-'));
const state = join(scratch, 'state'), home = join(scratch, 'home'), temp = join(scratch, 'temp');
for (const path of [state, home, temp]) await mkdir(path, { recursive: true });
const token = 'sandbox-test-isolated-bearer-at-least-32-characters';
const checks = [], errors = [], requests = [], cases = new Map(), results = new Map();
let host, browser, socket, ui, hostLog = '', seq = 0;
const pending = new Map(), sleep = ms => new Promise(r => setTimeout(r, ms));
async function wait(fn, label, timeout = 30000) { const until = Date.now() + timeout; while (Date.now() < until) { if (await fn()) return; await sleep(60); } throw Error('Timeout: ' + label); }
function check(name, ok) { assert.ok(ok, name); checks.push(name); console.log('PASS ' + name); }
async function exists(path) { try { await access(path); return true; } catch { return false; } }
async function freePort() { const s = createServer(); await new Promise(r => s.listen(0, '127.0.0.1', r)); const p = s.address().port; await new Promise(r => s.close(r)); return p; }
async function stop(child) { if (!child || child.exitCode != null) return; const stopped = once(child, 'exit'); child.kill('SIGKILL'); await Promise.race([stopped, sleep(5000)]); }
const port = await freePort(), endpoint = `http://127.0.0.1:${port}`;
const fixture = createServer(async (req, res) => {
  try {
    let raw = ''; for await (const part of req) { raw += part; if (raw.length > 4 * 1024 * 1024) throw Error('fixture bound'); }
    const body = JSON.parse(raw); requests.push(body);
    const index = body.messages.findLastIndex(m => m.role === 'user' && typeof m.content === 'string' && !m.content.startsWith('[Host context source:'));
    const prompt = body.messages[index]?.content, task = cases.get(prompt);
    const output = body.messages.slice(index + 1).find(m => m.role === 'tool');
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    const frame = value => res.write(`data: ${JSON.stringify(value)}\n\n`);
    if (task && !output) {
      assert.ok(body.tools.some(t => t.function.name === task.tool));
      if (task.mode) assert.ok(body.messages.some(m => String(m.content).includes(`Current sandbox mode: ${task.mode}.`)), 'actual pinned mode enters host context');
      frame({ choices: [{ index: 0, delta: { tool_calls: [{ index: 0, id: `sandbox-${requests.length}`, type: 'function', function: { name: task.tool, arguments: JSON.stringify(task.args) } }] }, finish_reason: 'tool_calls' }] });
    } else {
      if (output) results.set(prompt, JSON.parse(output.content));
      frame({ choices: [{ index: 0, delta: { content: 'DONE ' + prompt }, finish_reason: 'stop' }] });
    }
    frame({ choices: [], usage: { prompt_tokens: Math.ceil(Buffer.byteLength(raw) / 3), completion_tokens: 40 } }); res.end('data: [DONE]\n\n');
  } catch (e) { errors.push('fixture: ' + e); res.destroy(); }
});
await new Promise(r => fixture.listen(0, '127.0.0.1', r));
async function api(path, body, expected = 200, auth = true, method = body == null ? 'GET' : 'POST') {
  const r = await fetch(endpoint + path, { method, headers: { ...(auth ? { Authorization: 'Bearer ' + token } : {}), 'Content-Type': 'application/json' }, body: body == null ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(20000) });
  const text = await r.text(); assert.equal(r.status, expected, `${path}: ${text}`); return text ? JSON.parse(text) : null;
}
async function start(origin, extra = {}) {
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('AGENT_') && !key.startsWith('MONA_DEV_')));
  Object.assign(env, { MONA_DEV_STATE_DIR: state, HOME: home, USERPROFILE: home, TEMP: temp, TMP: temp, TMPDIR: temp,
    AGENT_SERVER_ADDR: `127.0.0.1:${port}`, AGENT_SERVER_TOKEN: token, AGENT_UI_ORIGIN: origin, AGENT_MODEL_MANAGEMENT: '0',
    AGENT_MODEL_NAME: 'sandbox-fixture', AGENT_MODEL_ENDPOINT: `http://127.0.0.1:${fixture.address().port}/v1/chat/completions`, AGENT_ALLOW_HTTP_LOOPBACK: '1',
    AGENT_CAPABILITY_STATE_PATH: join(state, 'capabilities.json'), AGENT_INSTRUCTIONS: '0', AGENT_COMPACTION: '0', AGENT_SPILL: '0', ...extra });
  hostLog = ''; host = spawn(resolve(process.env.MONA_TEST_SERVER || join(root, 'target/debug', process.platform === 'win32' ? 'server.exe' : 'server')), [], { cwd: scratch, env, stdio: ['ignore', 'pipe', 'pipe'] });
  host.stderr.on('data', c => hostLog = (hostLog + c).slice(-16000)); host.on('error', e => errors.push(String(e)));
  await wait(async () => { if (host.exitCode != null) throw Error(hostLog); try { return (await api('/v1/info')).protocol_version === 2; } catch { return false; } }, 'host');
}
function cdp(method, params = {}) { return new Promise((resolve, reject) => { const id = ++seq, timer = setTimeout(() => { pending.delete(id); reject(Error('CDP ' + method)); }, 20000); pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params })); }); }
async function evaluate(expression) { const r = await cdp('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }); if (r.exceptionDetails) throw Error(r.exceptionDetails.exception?.description || 'page exception'); return r.result.value; }
const pageWait = (expression, label) => wait(() => evaluate(expression), label);
async function click(selector) { const p = await evaluate(`(()=>{const n=document.querySelector(${JSON.stringify(selector)});if(!n||n.disabled)throw Error('missing or disabled '+${JSON.stringify(selector)});n.scrollIntoView({block:'nearest'});const r=n.getBoundingClientRect();if(!r.width)throw Error('hidden control');return{x:r.x+r.width/2,y:r.y+r.height/2}})()`); await cdp('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...p }); await cdp('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, ...p }); }
async function fill(selector, value) { await evaluate(`(()=>{const n=document.querySelector(${JSON.stringify(selector)});n.value=${JSON.stringify(value)};n.dispatchEvent(new Event('input',{bubbles:true}));n.dispatchEvent(new Event('change',{bubbles:true}));})()`); }
async function shot(name) { const r = await cdp('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false }); await writeFile(join(report, name + '.png'), Buffer.from(r.data, 'base64')); }
async function current() { const id = await evaluate("location.hash.slice('#session='.length)"); return api('/api/sessions/' + id); }
async function mode(value) { const label = { 'read-only':'仅可查看', 'workspace-write':'工作区内修改', 'danger-full-access':'完全权限' }[value]; await click('#sandbox-permission'); await pageWait("!document.querySelector('#sandbox-permission-menu').hidden", 'permission menu'); await click(`[data-sandbox-mode="${value}"]`); await pageWait(`document.querySelector('#sandbox-permission .permission-trigger-label')?.textContent===${JSON.stringify(label)}&&!document.querySelector('#sandbox-permission').disabled&&document.querySelector('#sandbox-permission-menu').hidden`, 'confirmed mode'); }
async function task(name, tool, args, selectedMode, success) {
  cases.set(name, { tool, args, mode: selectedMode }); const before = (await current()).session.turn_count;
  await fill('#prompt', name); await click('#send');
  await wait(async () => { const d = await current(); if (d.session.status === 'failed') throw Error(JSON.stringify(d)); return d.session.status === 'completed' && d.session.turn_count === before + 1; }, name);
  await pageWait("document.querySelector('#cancel').hidden&&!document.querySelector('#sandbox-permission').disabled", 'settled controls');
  assert.ok(results.has(name), 'actual tool result recorded'); assert.equal(results.get(name).status === 'success', success, JSON.stringify(results.get(name)));
}
const ps = value => "'" + value.replaceAll("'", "''") + "'";
const writeCommand = (file, text) => process.platform === 'win32' ? `Set-Content -LiteralPath ${ps(file)} -Value ${ps(text)} -ErrorAction Stop` : `printf '%s' ${ps(text)} > ${ps(file)}`;
try {
  ui = await startUiServer({ host: '127.0.0.1', port: 0, endpoint, token }); const origin = `http://127.0.0.1:${ui.address().port}`; await start(origin);
  await api('/api/sandbox', null, 401, false);
  const initial = await api('/api/sandbox'); check('default product policy is workspace-write with actual backend facts', initial.mode === 'workspace-write' && initial.enabled && initial.backend && !initial.unavailable && (process.platform !== 'win32' || initial.backend.enforcement === 'partial'));
  const debug = await freePort(); browser = spawn(process.env.BROWSER_BIN || (process.platform === 'win32' ? 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe' : 'chromium'), ['--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check', `--remote-debugging-port=${debug}`, `--user-data-dir=${join(scratch, 'browser')}`, '--window-size=1440,1000', 'about:blank'], { stdio: 'ignore' });
  let page; await wait(async () => { try { page = (await (await fetch(`http://127.0.0.1:${debug}/json/list`)).json()).find(p => p.type === 'page'); return Boolean(page); } catch { return false; } }, 'browser');
  socket = new WebSocket(page.webSocketDebuggerUrl); await once(socket, 'open'); socket.addEventListener('message', event => { const m = JSON.parse(event.data), p = pending.get(m.id); if (p) { pending.delete(m.id); clearTimeout(p.timer); m.error ? p.reject(Error(JSON.stringify(m.error))) : p.resolve(m.result); } if (m.method === 'Runtime.exceptionThrown') errors.push(m.params.exceptionDetails.exception?.description || 'browser exception'); });
  await cdp('Page.enable'); await cdp('Runtime.enable'); await cdp('Page.navigate', { url: origin });
  await pageWait("document.querySelector('#sandbox-permission .permission-trigger-label')?.textContent==='工作区内修改'&&!document.querySelector('#sandbox-permission').disabled&&document.querySelector('#sessions-notice').textContent.includes('已保存在宿主本地')", 'sandbox module');
  check('composer matches DSH compact control geometry before plan mode is active', await evaluate(`(()=>{const box=s=>document.querySelector(s).getBoundingClientRect(),css=s=>getComputedStyle(document.querySelector(s));return box('#composer-add').width===28&&box('#composer-add').height===28&&box('#sandbox-permission').height===28&&box('#send').width===34&&box('#send').height===34&&document.querySelector('#planner-mode-chip')?.hidden===true;})()`));
  await click('#sandbox-permission'); await pageWait("!document.querySelector('#sandbox-permission-menu').hidden", 'permission geometry menu');
  check('permission menu matches DSH card and row geometry', await evaluate(`(()=>{const menu=document.querySelector('#sandbox-permission-menu'),row=menu.querySelector('.permission-option'),css=getComputedStyle(menu);return css.borderRadius==='16px'&&Math.round(row.getBoundingClientRect().height)===34&&row.querySelector('.permission-option-icon svg')?.getBoundingClientRect().width===14;})()`));
  await shot('sandbox-permission-menu');
  await click('[data-sandbox-mode="workspace-write"]');
  await mode('read-only'); let doc = await current(), id = doc.session.id, cwd = doc.session.workspace;
  check('UI mode choice is durable before the first model call', requests.length === 0 && doc.session.turn_count === 0 && (await api(`/api/sessions/${id}/sandbox`)).mode === 'read-only');
  const before = await api(`/api/sessions/${id}/sandbox`);
  await api(`/api/sessions/${id}/sandbox`, { revision: before.revision, mode: 'danger-full-access', workspace: 'C:/' }, 400);
  await api(`/api/sessions/${id}/sandbox`, { revision: before.revision - 1, mode: 'danger-full-access' }, 409);
  await task('RO_WRITE', 'write', { path: 'ro-write.txt', content: 'no' }, 'read-only', false);
  await task('RO_SHELL', 'shell', { command: writeCommand('ro-shell.txt', 'no') }, 'read-only', false);
  check('read-only blocks BOTH native Shell and direct write', !await exists(join(cwd, 'ro-shell.txt')) && !await exists(join(cwd, 'ro-write.txt')));
  const outside = join(scratch, 'outside.txt'); await writeFile(outside, 'ORIGINAL_OUTSIDE');
  await task('RO_EDIT', 'edit', { path: outside, edits: [{ oldText: 'ORIGINAL_OUTSIDE', newText: 'MUST_NOT_EDIT' }] }, 'read-only', false);
  await task('RO_READ', 'read', { path: outside }, 'read-only', true);
  check('read-only still reads outside files as specified by DSH', JSON.stringify(results.get('RO_READ')).includes('ORIGINAL_OUTSIDE'));
  await task('RO_SHELL_READ', 'shell', { command: process.platform === 'win32' ? `Get-Content -LiteralPath ${ps(outside)} -Raw` : `cat ${ps(outside)}` }, 'read-only', true);
  check('read-only Shell remains usable rather than failing during its own initialization', results.get('RO_SHELL_READ').content.includes('ORIGINAL_OUTSIDE'));
  await mode('workspace-write');
  await task('RW_WRITE', 'write', { path: 'inside.txt', content: 'before' }, 'workspace-write', true);
  await task('RW_EDIT', 'edit', { path: 'inside.txt', edits: [{ oldText: 'before', newText: 'after' }] }, 'workspace-write', true);
  await task('RW_SHELL', 'shell', { command: writeCommand('shell.txt', 'shell-result') }, 'workspace-write', true);
  check('real file tools and native Shell work inside the same workspace', (await readFile(join(cwd, 'inside.txt'), 'utf8')) === 'after' && (await readFile(join(cwd, 'shell.txt'), 'utf8')).includes('shell-result'));
  await task('RW_OUTSIDE_WRITE', 'write', { path: outside, content: 'MUST_NOT_WRITE' }, 'workspace-write', false);
  await task('RW_OUTSIDE_SHELL', 'shell', { command: writeCommand(outside, 'MUST_NOT_WRITE') }, 'workspace-write', false);
  await task('RW_OUTSIDE_EDIT', 'edit', { path: outside, edits: [{ oldText: 'ORIGINAL_OUTSIDE', newText: 'MUST_NOT_EDIT' }] }, 'workspace-write', false);
  check('both write paths refuse changes outside workspace and allowed temporary roots', (await readFile(outside, 'utf8')) === 'ORIGINAL_OUTSIDE');
  await mode('danger-full-access'); await task('FULL_WRITE', 'write', { path: outside, content: 'EXPLICIT_FULL_ACCESS' }, 'danger-full-access', true);
  check('only an explicit host mode change allows unrestricted file effects', (await readFile(outside, 'utf8')) === 'EXPLICIT_FULL_ACCESS');
  await task('FULL_SHELL', 'shell', { command: writeCommand(outside, 'EXPLICIT_FULL_SHELL') }, 'danger-full-access', true);
  check('explicit full-access Shell uses the same original tool without confinement', (await readFile(outside, 'utf8')).includes('EXPLICIT_FULL_SHELL'));
  if (process.platform === 'win32') {
    cases.set('TEMP_QUERY', { tool: 'shell', args: { command: '$env:TEMP' }, mode: 'workspace-write' });
    const temporary = await api('/api/sessions', { request_id: 'temp-session-cleanup' });
    await api(`/api/sessions/${temporary.id}/turns`, { request_id: 'temp-turn', revision: temporary.revision, prompt: 'TEMP_QUERY' });
    await wait(async () => (await api(`/api/sessions/${temporary.id}`)).session.status === 'completed', 'temporary session');
    const privateTemp = results.get('TEMP_QUERY').content.trim(); assert.ok(await exists(privateTemp));
    const current = (await api(`/api/sessions/${temporary.id}`)).session;
    await api(`/api/sessions/${temporary.id}/delete`, { revision: current.revision }, 204);
    check('deleting an idle session revokes and removes only its private sandbox temp', !await exists(privateTemp) && await exists(cwd));
  }
  await mode('read-only'); await cdp('Page.reload', { ignoreCache: true });
  await pageWait("document.querySelector('#sandbox-permission .permission-trigger-label')?.textContent==='仅可查看'&&!document.querySelector('#sandbox-permission').disabled", 'reload');
  const recordedTurns = (await current()).session.turn_count, requestsBeforeRestart = requests.length;
  await stop(host); await start(origin); await cdp('Page.reload', { ignoreCache: true });
  await pageWait("document.querySelector('#sandbox-permission .permission-trigger-label')?.textContent==='仅可查看'&&!document.querySelector('#sandbox-permission').disabled", 'restart');
  check('confirmed policy survives a real host restart without running a task', (await current()).session.turn_count === recordedTurns && requests.length === requestsBeforeRestart);
  await mode('workspace-write');
  const child = process.platform === 'win32' ? `Set-Content -LiteralPath 'child-started' -Value ready; Start-Sleep -Seconds 4; Set-Content -LiteralPath 'child-leaked' -Value bad` : '';
  const cancelScript = process.platform === 'win32' ? `$child=Start-Process -FilePath (Join-Path $PSHOME $(if ($PSVersionTable.PSEdition -eq 'Core') { 'pwsh.exe' } else { 'powershell.exe' })) -NoNewWindow -PassThru -ArgumentList '-NoProfile','-NonInteractive','-EncodedCommand','${Buffer.from(child, 'utf16le').toString('base64')}'; $child.WaitForExit()` : '(printf ready > child-started; sleep 4; printf bad > child-leaked) & wait';
  cases.set('CANCEL_NATIVE', { tool: 'shell', args: { command: cancelScript }, mode: 'workspace-write' });
  await fill('#prompt', 'CANCEL_NATIVE'); await click('#send'); await wait(() => exists(join(cwd, 'child-started')), 'real confined descendant started');
  const running = await api(`/api/sessions/${id}/sandbox`); await api(`/api/sessions/${id}/sandbox`, { revision: running.revision, mode: 'danger-full-access' }, 409);
  check('running policy cannot be widened by UI or concurrent API write', await evaluate("document.querySelector('#sandbox-permission').disabled"));
  await click('#cancel'); await wait(async () => (await current()).session.status === 'cancelled', 'cancelled'); await sleep(4300);
  check('cancellation kills the actual confined process tree without replay', !await exists(join(cwd, 'child-leaked')) && requests.filter(r => r.messages.some(m => m.content === 'CANCEL_NATIVE')).length === 1);
  await pageWait("!document.querySelector('#sandbox-permission').disabled", 'cancel controls');
  for (const width of [1440, 390, 320]) { await cdp('Emulation.setDeviceMetricsOverride', { width, height: 1000, deviceScaleFactor: 1, mobile: width < 600 }); await sleep(100); check(`sandbox controls stay inside ${width}px viewport`, await evaluate('document.documentElement.scrollWidth<=innerWidth+1')); await shot('sandbox-' + width); }
  await cdp('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'dark' }] }); await shot('sandbox-dark');
  const settings = await api('/api/capabilities'); await api('/api/capabilities/sandbox', { revision: settings.revision, enabled: false }, 200, true, 'PUT');
  check('saved disable is not mistaken for an already unconfined host', (await api(`/api/sessions/${id}/sandbox`)).enabled);
  await stop(host); await start(origin); const disabled = await api(`/api/sessions/${id}/sandbox`), beforeCount = (await api(`/api/sessions/${id}`)).session.turn_count;
  await api(`/api/sessions/${id}/turns`, { request_id: 'do-not-bypass', revision: disabled.revision, prompt: 'blocked' }, 400);
  check('removing sandbox cannot silently resume a previously confined session', !disabled.enabled && disabled.mode === 'workspace-write' && (await api(`/api/sessions/${id}`)).session.turn_count === beforeCount);
  await stop(host); await start(origin, { AGENT_SANDBOX_MODE: 'read-only' }); const locked = await api(`/api/sessions/${id}/sandbox`);
  await api(`/api/sessions/${id}/sandbox`, { revision: locked.revision, mode: 'danger-full-access' }, 400);
  check('deployment override locks both mode and sandbox assembly', locked.enabled && locked.locked && locked.mode === 'read-only' && !(await api('/api/ui')).capabilities.sandbox.configurable);
  check('no unhandled browser or model fixture errors', errors.length === 0);
  await writeFile(join(report, 'result.json'), JSON.stringify({ passed: true, checks, model_requests: requests.length, native_platform: process.platform, real_provider: false, isolated: true }, null, 2));
} catch (e) {
  console.error(e); console.error(hostLog);
  if (socket?.readyState === WebSocket.OPEN) { console.error(await evaluate("document.body.innerText.slice(-2500)").catch(() => '')); await shot('failure').catch(() => {}); }
  await writeFile(join(report, 'result.json'), JSON.stringify({ passed: false, checks, error: String(e), errors }, null, 2)); process.exitCode = 1;
} finally {
  if (socket?.readyState === WebSocket.OPEN) { await cdp('Browser.close').catch(() => {}); socket.close(); }
  for (const p of pending.values()) { clearTimeout(p.timer); p.reject(Error('test cleanup')); } pending.clear();
  await stop(browser); await stop(host); ui?.closeAllConnections(); if (ui) await new Promise(r => ui.close(r)); fixture.closeAllConnections(); await new Promise(r => fixture.close(r));
  await rm(scratch, { recursive: true, force: true, maxRetries: 8, retryDelay: 200 });
}
