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
  res.end('data: ' + JSON.stringify({ choices: [{ index: 0, delta: { content: '已完成：' + prompt }, finish_reason: 'stop' }] }) + '\n\ndata: [DONE]\n\n');
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
  const point = await evaluate(`(() => { const n=document.querySelector(${JSON.stringify(selector)});if(!n)throw Error('missing '+${JSON.stringify(selector)});n.scrollIntoView({block:'nearest'});const r=n.getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2};})()`);
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
  Object.assign(env, { MONA_DEV_STATE_DIR: state, HOME: home, USERPROFILE: home, AGENT_SERVER_ADDR: `127.0.0.1:${port}`, AGENT_SERVER_TOKEN: token, AGENT_UI_ORIGIN: origin,
    AGENT_MODEL_ENDPOINT: `http://127.0.0.1:${model.address().port}/chat/completions`, AGENT_MODEL_NAME: 'right-pane-fixture', AGENT_ALLOW_HTTP_LOOPBACK: '1', AGENT_MODEL_MANAGEMENT: '0', AGENT_SKILLS: '0', AGENT_INSTRUCTIONS: '0', AGENT_MEMORY: '0', AGENT_HISTORY_SEARCH: '0' });
  host = spawn(resolve(process.env.MONA_TEST_SERVER || 'target/zcode-right-pane/debug/server.exe'), [], { cwd: scratch, env, stdio: ['ignore', 'pipe', 'pipe'] }); host.stderr.on('data', d => { logs = (logs + d).slice(-6000); });
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
  await click('#workspace-files-open'); await pageWait("document.querySelector('.file-entry[data-path=\"notes.md\"]')", 'file tree');
  await click('.file-entry[data-path="docs"]'); await pageWait("document.querySelector('.file-entry[data-path=\"docs/nested.txt\"]')", 'lazy directory');
  check('left workspace view lazily expands a real directory', await evaluate("document.querySelector('.file-entry[data-path=\"docs\"]').closest('details').open"));
  await click('.file-entry[data-path="notes.md"]'); await pageWait("document.querySelector('#right-content .workspace-markdown-preview h1')?.textContent==='MONA_MARKDOWN'", 'markdown preview');
  await evaluate("window.__retained=document.querySelector('#right-content .workspace-markdown-preview')");
  await click('.file-entry[data-path="code.rs"]'); await pageWait("document.querySelector('#right-content .workspace-source-preview code')?.textContent.includes('MONA_CODE')", 'code preview');
  check('opening files produces deduplicated compact tabs and highlighted source', await evaluate("document.querySelectorAll('.right-tab').length===2 && document.querySelector('#right-content .token-keyword')!==null && [...document.querySelectorAll('.right-tab-wrap')].every(n=>n.getBoundingClientRect().height===28)"));
  await click('.right-tab:first-child'); check('switching tabs preserves the mounted preview DOM', await evaluate("window.__retained===document.querySelector('.workspace-markdown-preview') && !window.__retained.closest('[role=tabpanel]').hidden"));
  await textButton('#right-content > :not([hidden]) .right-file-toolbar', '源码'); check('Markdown source/preview switching is real', await evaluate("document.querySelector('#right-content > :not([hidden]) code').textContent.includes('# MONA_MARKDOWN')"));
  await click('.right-tab-wrap:last-child .right-tab', 'right'); await textButton('.right-tab-context', '关闭其他标签'); check('tab context menu closes only the requested peers', await evaluate("document.querySelectorAll('.right-tab').length===1"));
  await click('.right-overview-button'); await textButton('.right-overview-rows', 'notes.md'); await pageWait("document.querySelectorAll('.right-tab').length===2", 'reopen tab');
  check('tab overview reopens a recently closed file', true);
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
    await shot(extension + '-preview'); await click('.right-tab-close');
  }
  await click('#right-add'); await textButton('#right-add-menu', '审查'); await pageWait("document.querySelector('.review-entry')", 'review UI'); await textButton('.review-files', 'notes.mdM');
  await pageWait("document.querySelector('.review-detail').textContent.includes('MODIFIED_FOR_REVIEW')", 'patch rendered'); check('review tab is connected to actual Git diffs', true);
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
  await send('Emulation.setDeviceMetricsOverride', { width: 390, height: 900, deviceScaleFactor: 1, mobile: false }); await click('#files-toggle');
  check('narrow screens use a bounded independent panel', await evaluate("document.querySelector('#files-panel').getAttribute('aria-modal')==='true' && document.documentElement.scrollWidth<=390")); await shot('right-pane-mobile');
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
