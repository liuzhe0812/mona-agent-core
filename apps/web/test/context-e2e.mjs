// Formal Web + real Rust host/tools + local model fixture. Never reads the user's .env or state.
// Build an isolated binary first; MONA_TEST_SERVER selects it. No running development service is stopped.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, mkdir, readdir, readFile, writeFile, rm, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startUiServer } from '../../../scripts/dev-web.mjs';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const output = resolve(process.env.MONA_TEST_REPORT_DIR || join(root, 'target/browser-reports/context'));
const scratch = await mkdtemp(join(tmpdir(), 'mona-context-e2e-'));
const workspace = join(scratch, 'workspace'), state = join(scratch, 'state');
await mkdir(workspace); await mkdir(state); await mkdir(output, { recursive: true });
const skillRoot = join(workspace, '.agents', 'skills');
await mkdir(join(skillRoot, 'context-check'), { recursive: true });
await mkdir(join(scratch, '.agents/skills/wrong-workspace'), { recursive: true });
await writeFile(join(scratch, '.agents/skills/wrong-workspace/SKILL.md'), '---\nname: wrong-workspace\ndescription: MUST_NOT_LOAD_STARTUP_DIRECTORY\n---\nDecoy.\n');
await writeFile(join(skillRoot, 'context-check', 'SKILL.md'), '---\nname: context-check\ndescription: Test-only contextual skill catalog marker.\n---\nPRIVATE_SKILL_BODY_NOT_EAGERLY_LOADED\n');
await writeFile(join(workspace, 'AGENTS.md'), 'PROJECT_CONTEXT_RULE_ALPHA: preserve existing work.\n');
await writeFile(join(workspace, 'agent.toml'), 'version = 1\n[capabilities.skills]\nenabled = true\nuser_configurable = true\n');
const token = 'isolated-context-test-bearer-1234567890';
const requests = [], checks = [], errors = [];
let summaries = 0, artifactUri = null, host, browser, socket, ui, hostLog = '';
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const check = (name, condition) => { assert.ok(condition, name); checks.push(name); console.log(`PASS ${name}`); };
async function waitFor(fn, name, timeout = 30000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) { if (await fn()) return; await sleep(60); }
  throw new Error(`Timed out: ${name}`);
}
async function stop(child) {
  if (!child || child.exitCode != null) return;
  const closed = once(child, 'exit'); child.kill('SIGKILL');
  await Promise.race([closed, sleep(5000)]);
}
function userMessages(request) {
  return request.messages.filter(message => message.role === 'user' && typeof message.content === 'string'
    && !message.content.startsWith('[Host context source:') && !message.content.startsWith('[Earlier conversation summary'));
}
const fixture = createServer(async (req, res) => {
  try {
    if (req.method === 'GET' && req.url === '/models') { res.setHeader('content-type', 'application/json'); res.end(JSON.stringify({ data: [{ id: 'context-fixture' }] })); return; }
    let raw = ''; for await (const chunk of req) { raw += chunk; if (raw.length > 4 * 1024 * 1024) throw new Error('fixture input limit'); }
    const body = JSON.parse(raw); requests.push(body);
    const summary = body.messages[0]?.content?.startsWith('Summarize the earlier');
    const latest = userMessages(body).at(-1)?.content || '';
    res.writeHead(200, { 'Content-Type': 'text/event-stream', 'Cache-Control': 'no-store' });
    const frame = value => res.write(`data: ${JSON.stringify(value)}\n\n`);
    const text = value => frame({ choices: [{ index: 0, delta: { content: value }, finish_reason: 'stop' }] });
    const call = (name, args) => frame({ choices: [{ index: 0, delta: { tool_calls: [{ index: 0, id: `context-call-${requests.length}`, type: 'function', function: { name, arguments: JSON.stringify(args) } }] }, finish_reason: 'tool_calls' }] });
    if (summary) { summaries++; text('Preserved decision 31415 and completed earlier conversation.'); }
    else if (latest.startsWith('ARCHIVE_CREATE') && body.messages.at(-1)?.role === 'user') {
      const command = process.platform === 'win32'
        ? "[Console]::Write('ARTIFACT_31415 ' + ('z' * 60000) + 'MIDDLE_27182' + ('y' * 60000) + 'END_16180')"
        : "printf 'ARTIFACT_31415 '; head -c 60000 /dev/zero | tr '\\0' z; printf MIDDLE_27182; head -c 60000 /dev/zero | tr '\\0' y; printf END_16180";
      call('shell', { command });
    } else if ((latest.startsWith('ARCHIVE_READ') || latest.startsWith('FORGED_READ')) && body.messages.at(-1)?.role === 'user') {
      call('read', { path: artifactUri, limit: 100 });
    } else if (latest.startsWith('ARCHIVE_READ') || latest.startsWith('FORGED_READ')) {
      text(String(body.messages.at(-1)?.content).includes('ARTIFACT_31415') ? 'ARCHIVE_READ_OK' : 'ARCHIVE_READ_DENIED');
    } else { text(`ACK ${latest.slice(0, 28)}`); }
    frame({ choices: [], usage: { prompt_tokens: summary ? 100000 : Math.ceil(Buffer.byteLength(raw) / 3), completion_tokens: 20 } });
    res.end('data: [DONE]\n\n');
  } catch (error) { errors.push(`fixture: ${error.message}`); res.destroy(); }
});
await new Promise(resolve => fixture.listen(0, '127.0.0.1', resolve));
const modelBase = `http://127.0.0.1:${fixture.address().port}`;
async function unusedPort() { const server = createServer(); await new Promise(r => server.listen(0, '127.0.0.1', r)); const port = server.address().port; await new Promise(r => server.close(r)); return port; }
const apiPort = await unusedPort(), endpoint = `http://127.0.0.1:${apiPort}`;
async function api(path) {
  const response = await fetch(endpoint + path, { headers: { Authorization: `Bearer ${token}` }, cache: 'no-store', signal: AbortSignal.timeout(10000) });
  if (!response.ok) throw new Error(`API ${path}: ${response.status} ${await response.text()}`); return response.json();
}
async function startHost(origin) {
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('AGENT_')));
  Object.assign(env, { AGENT_SERVER_ADDR: `127.0.0.1:${apiPort}`, AGENT_SERVER_TOKEN: token, AGENT_UI_ORIGIN: origin,
    AGENT_ALLOW_HTTP_LOOPBACK: '1', AGENT_MODEL_STORE_KEY: 'isolated-test-storage-key-not-a-real-secret',
    AGENT_MODEL_SETTINGS_PATH: join(state, 'models.enc'), AGENT_CAPABILITY_STATE_PATH: join(state, 'capabilities.json'),
    AGENT_SESSIONS_DIR: join(state, 'sessions'), AGENT_SPILL_DIR: join(state, 'spill'), AGENT_WORKSPACE_DIR: workspace,
    USERPROFILE: join(state, 'user'), HOME: join(state, 'user'), LOCALAPPDATA: state, XDG_STATE_HOME: state });
  const binary = resolve(process.env.MONA_TEST_SERVER || join(root, 'target', 'context-validation', 'debug', process.platform === 'win32' ? 'server.exe' : 'server'));
  await access(binary); host = spawn(binary, ['--config', join(workspace, 'agent.toml')], { cwd: scratch, env, stdio: ['ignore', 'pipe', 'pipe'] });
  host.stderr.on('data', chunk => { hostLog = (hostLog + chunk).slice(-10000); });
  host.on('error', error => errors.push(`host: ${error.message}`));
  await waitFor(async () => { if (host.exitCode != null) throw new Error(`host exited: ${hostLog}`); try { return (await api('/v1/info')).protocol_version === 2; } catch { return false; } }, 'host ready');
}
let nextId = 0; const pending = new Map();
function cdp(method, params = {}) {
  return new Promise((resolve, reject) => { const id = ++nextId; const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 20000);
    pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params })); });
}
async function evaluate(expression) {
  const result = await cdp('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description || 'page script failed'); return result.result.value;
}
const waitPage = (expression, name) => waitFor(() => evaluate(expression), name);
async function click(selector) { await evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`); }
async function fill(selector, value) { await evaluate(`(() => { const n = document.querySelector(${JSON.stringify(selector)}); n.value = ${JSON.stringify(value)}; n.dispatchEvent(new Event('input', {bubbles:true})); })()`); }
async function failureDetail(entry) {
  let savedError = null;
  // Only this test's isolated state directory is inspected; never user configuration/history.
  for (const folder of await readdir(join(state, 'sessions'))) {
    try {
      const text = await readFile(join(state, 'sessions', folder, `${entry.id}.jsonl`), 'utf8');
      const body = JSON.parse(text.slice(text.indexOf('\n') + 1));
      savedError = body.checkpoint?.error ?? body.turns?.at(-1)?.error;
    } catch { /* a lock/non-session entry */ }
  }
  return JSON.stringify({ status:entry.status, turn_count:entry.turn_count, saved_error:savedError,
    summaries, recent_requests:requests.slice(-3).map(request => ({bytes:Buffer.byteLength(JSON.stringify(request)),
      roles:request.messages.map(message=>message.role)})) });
}
async function submit(value, count) {
  await fill('#prompt', value); await waitPage("!document.querySelector('#send').disabled", 'composer ready'); await click('#send');
  await waitFor(async () => { const entry = (await api('/api/sessions')).sessions[0]; if (entry?.status === 'failed' || entry?.status === 'limited') throw new Error(`turn failed: ${await failureDetail(entry)}`); return entry?.turn_count === count && entry.status === 'completed'; }, `durable turn ${count}`);
  await waitPage("document.querySelector('#cancel').hidden && !document.querySelector('#sessions-refresh').disabled", 'UI saved');
}
async function screenshot(name) { const image = await cdp('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false }); await writeFile(join(output, `${name}.png`), Buffer.from(image.data, 'base64')); }
const primary = () => requests.filter(request => !request.messages[0]?.content?.startsWith('Summarize the earlier'));
try {
  ui = await startUiServer({ host: '127.0.0.1', port: 0, endpoint, token });
  const origin = `http://127.0.0.1:${ui.address().port}`; await startHost(origin);
  check('fresh host has no provider settings or inherited sessions', (await api('/api/model-settings')).providers.length === 0 && (await api('/api/sessions')).sessions.length === 0);
  const debugPort = await unusedPort();
  const executable = process.env.BROWSER_BIN || (process.platform === 'win32' ? 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe' : 'chromium');
  browser = spawn(executable, ['--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check', '--edge-skip-compat-layer-relaunch', `--remote-debugging-port=${debugPort}`, `--user-data-dir=${join(scratch, 'browser')}`, '--window-size=1440,1000', 'about:blank'], { stdio: 'ignore' });
  browser.on('error', error => errors.push(`browser: ${error.message}`));
  let page; await waitFor(async () => { try { page = (await (await fetch(`http://127.0.0.1:${debugPort}/json/list`, { signal: AbortSignal.timeout(800) })).json()).find(p => p.type === 'page'); return !!page; } catch { return false; } }, 'browser ready');
  socket = new WebSocket(page.webSocketDebuggerUrl); await once(socket, 'open');
  socket.addEventListener('message', event => { const value = JSON.parse(event.data), request = pending.get(value.id);
    if (request) { pending.delete(value.id); clearTimeout(request.timer); if (value.error) request.reject(new Error(JSON.stringify(value.error))); else request.resolve(value.result); }
    if (value.method === 'Runtime.exceptionThrown') errors.push(value.params.exceptionDetails.exception?.description || 'browser exception'); });
  await cdp('Page.enable'); await cdp('Runtime.enable');
  await cdp('Page.addScriptToEvaluateOnNewDocument', { source: "window.__contextErrors=[];addEventListener('error',e=>window.__contextErrors.push(String(e.message)));addEventListener('unhandledrejection',e=>window.__contextErrors.push(String(e.reason)))" });
  await cdp('Page.navigate', { url: origin }); await waitPage("document.querySelector('#sessions-notice')?.textContent.includes('已保存在宿主本地') && !document.querySelector('#sessions-refresh').disabled", 'persistent session UI ready');
  await click('#settings-button'); await click('#settings-models-tab'); await click('#add-provider');
  await fill('#provider-name-input', '隔离上下文验证'); await fill('#provider-api-base-input', modelBase); await fill('#provider-models-input', 'context-fixture'); await click('#provider-save');
  await waitPage("!document.querySelector('#provider-dialog').open && document.querySelector('[data-model-context]')", 'provider saved from UI');
  await click('[data-model-context="context-fixture"]'); await fill('#model-context-tokens', '16384');
  await screenshot('model-window-setting');
  await cdp('Emulation.setDeviceMetricsOverride', { width:390, height:900, deviceScaleFactor:1, mobile:true });
  check('model-window dialog fits the narrow viewport', await evaluate("(() => {const r=document.querySelector('#model-dialog').getBoundingClientRect();return r.left>=0 && r.right<=innerWidth+1 && document.querySelector('#model-dialog').scrollWidth<=r.width+1;})()"));
  await screenshot('model-window-narrow'); await cdp('Emulation.setDeviceMetricsOverride', { width:1440, height:1000, deviceScaleFactor:1, mobile:false });
  await click('#model-save'); await waitPage("!document.querySelector('#model-dialog').open", 'window saved');
  check('UI saves per-model window through the real management API', (await api('/api/model-settings')).providers[0].models[0].context_window_tokens === 16384);
  await click('#discover-models'); await waitPage("!document.querySelector('#discover-models').disabled", 'model discovery finished');
  check('model discovery preserves the configured window', (await api('/api/model-settings')).providers[0].models[0].context_window_tokens === 16384);
  await click('#settings-back');
  const firstPrompt = 'FIRST_MARKER decision 31415 ' + 'A'.repeat(18000);
  await submit(firstPrompt, 1); check('short context avoids a summary call', summaries === 0);
  const firstRequest = primary()[0], firstText = JSON.stringify(firstRequest);
  check('skills use the configured workspace rather than the server startup directory', firstText.includes('context-check') && !firstText.includes('MUST_NOT_LOAD_STARTUP_DIRECTORY'));
  check('project rules and skill catalog are supplied once without eager skill body loading', firstText.includes('PROJECT_CONTEXT_RULE_ALPHA') && firstText.includes('context-check') && !firstText.includes('PRIVATE_SKILL_BODY_NOT_EAGERLY_LOADED') && firstRequest.messages.filter(m => String(m.content).startsWith('[Host context source: project.instructions')).length === 1);
  await submit('SECOND_MARKER ' + 'B'.repeat(14000), 2);
  check('configured model window triggers compaction below the byte ceiling', summaries === 1 && requests.every(request => Buffer.byteLength(JSON.stringify(request)) < 256 * 1024));
  check('compaction preserves the true latest user request and keeps source blocks separate', userMessages(primary().at(-1)).at(-1).content.startsWith('SECOND_MARKER') && primary().at(-1).messages.filter(m => String(m.content).startsWith('[Host context source: skills.catalog')).length === 1);
  await submit('THIRD_MARKER continue', 3); check('next Run reuses the saved summary without summarizing the same prefix', summaries === 1);
  const sessionId = (await api('/api/sessions')).sessions[0].id;
  check('complete archive retains original messages rather than replacing them with summaries', (await api(`/api/sessions/${sessionId}`)).turns[0].prompt === firstPrompt);
  await stop(host); await writeFile(join(workspace, 'AGENTS.md'), 'PROJECT_CONTEXT_RULE_BETA: rules changed after restart.\n'); await startHost(origin);
  await cdp('Page.reload', { ignoreCache:true }); await waitPage("document.querySelectorAll('#timeline .turn').length===3 && !document.querySelector('#sessions-refresh').disabled", 'history after restart');
  check('model window survives a real host restart', (await api('/api/model-settings')).providers[0].models[0].context_window_tokens === 16384);
  await submit('AFTER_RESTART_MARKER', 4);
  check('restart reuses compacted context without additional summary work', summaries === 1);
  const resumed = JSON.stringify(primary().at(-1));
  check('project rules are refreshed after restart without stale injected rules', resumed.includes('PROJECT_CONTEXT_RULE_BETA') && !resumed.includes('PROJECT_CONTEXT_RULE_ALPHA'));
  await submit('ARCHIVE_CREATE', 5);
  let history = await api(`/api/sessions/${sessionId}`), latestTurn = history.turns.at(-1);
  let turnView = await api(`/api/sessions/${sessionId}/turns/${latestTurn.id}`);
  artifactUri = turnView.snapshot.items.map(item => item.content?.result?.artifact?.uri).find(uri => uri?.startsWith('spill:'));
  check('shell output above the former 50 KiB boundary uses the formal Spill component', Boolean(artifactUri));
  await stop(host); await startHost(origin); await cdp('Page.reload', { ignoreCache:true });
  await waitPage("document.querySelectorAll('#timeline .turn').length===5 && !document.querySelector('#sessions-refresh').disabled", 'archive after restart');
  let fullArchive = '', cursor = 0;
  for (;;) {
    const page = await api(`/api/spill/${latestTurn.run_id}/${artifactUri.slice('spill:'.length)}?offset=${cursor}&limit=16000`);
    fullArchive += page.text; cursor = page.next_offset;
    if (page.eof) break;
  }
  check('the full 120000-byte-plus output including middle and tail survives process restart',
    fullArchive === 'ARTIFACT_31415 ' + 'z'.repeat(60000) + 'MIDDLE_27182' + 'y'.repeat(60000) + 'END_16180');
  await submit('ARCHIVE_READ', 6);
  check('same conversation reads an earlier Run artifact after a real restart', String(primary().at(-1).messages.at(-1).content).includes('ARTIFACT_31415'));
  await screenshot('resumed-context-conversation');
  await click('#new-chat'); await submit(`FORGED_READ ${artifactUri}`, 1);
  const foreign = primary().at(-1);
  check('another conversation cannot gain archive access from a supplied URI', !String(foreign.messages.at(-1).content).includes('ARTIFACT_31415') && !JSON.stringify(foreign).includes('FIRST_MARKER'));
  check('no browser unhandled exceptions or fixture failures', (await evaluate('window.__contextErrors')).length === 0 && errors.length === 0);
  await writeFile(join(output, 'result.json'), JSON.stringify({ passed:true, checks, summaries, model_requests:requests.length, real_provider:false, isolated:true }, null, 2));
  console.log(`PASS ${checks.length} context end-to-end assertions`);
} catch (error) {
  console.error(error); console.error(hostLog);
  if (socket?.readyState === WebSocket.OPEN) {
    console.error(await evaluate("JSON.stringify({url:location.href,notice:document.querySelector('#sessions-notice')?.textContent,modelError:document.querySelector('#model-form-error')?.textContent,providerError:document.querySelector('#provider-form-error')?.textContent,footer:document.querySelector('#timeline')?.innerText.slice(-1500),errors:window.__contextErrors})").catch(()=>'no page diagnostics'));
    await screenshot('failure').catch(()=>{});
  }
  await writeFile(join(output, 'result.json'), JSON.stringify({ passed:false, checks, summaries, error:String(error), errors }, null, 2)); process.exitCode = 1;
} finally {
  if (socket?.readyState === WebSocket.OPEN) { await cdp('Browser.close').catch(()=>{}); socket.close(); }
  for (const entry of pending.values()) { clearTimeout(entry.timer); entry.reject(new Error('test cleanup')); } pending.clear();
  await stop(browser); await stop(host); ui?.closeAllConnections?.(); if (ui) await new Promise(r=>ui.close(r));
  fixture.closeAllConnections(); await new Promise(r=>fixture.close(r));
  await rm(scratch, { recursive:true, force:true, maxRetries:10, retryDelay:200 });
}
