// Formal Web UI + controlled HTTP/SSE fixture + real Chromium. No model credentials or Rust build needed.
// Uses isolated ephemeral ports and a temporary browser profile. Node 22+ provides the native WebSocket client.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, mkdir, readFile, writeFile, rm, readdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startUiServer } from '../../../scripts/dev-web.mjs';
import { createFixtureApiServer, TEST_TOKEN } from './fixture-server.mjs';
import { SKINS, THEME_STORAGE_KEY, DEFAULT_APPEARANCE } from '../theme.mjs';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const output = join(root, '.tmp-verify/theme-browser-report');
const scratch = await mkdtemp(join(tmpdir(), 'mona-themes-e2e-'));
const profile = join(scratch, 'browser'), downloads = join(scratch, 'downloads');
await mkdir(output, { recursive: true }); await mkdir(downloads);
const checks = [], errors = [], requests = [], connections = [];
let browser, ui, api, cdp, origin = '', endpoint = '';
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
async function waitFor(fn, label, timeout = 12000) {
  const until = Date.now() + timeout;
  while (Date.now() < until) { if (await fn()) return; await sleep(60); }
  throw new Error(`Timed out: ${label}`);
}
async function stop(child) {
  if (!child || child.exitCode != null) return;
  const exited = once(child, 'exit'); child.kill('SIGKILL'); await Promise.race([exited, sleep(3000)]);
}
function check(name, condition) { assert.ok(condition, name); checks.push(name); console.log(`PASS ${name}`); }
async function connect(url) {
  const socket = new WebSocket(url); await once(socket, 'open');
  const pending = new Map(); let seq = 0;
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++seq, timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 12000);
    pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params }));
  });
  socket.addEventListener('message', event => {
    const value = JSON.parse(event.data), request = pending.get(value.id);
    if (request) { pending.delete(value.id); clearTimeout(request.timer); value.error ? request.reject(new Error(JSON.stringify(value.error))) : request.resolve(value.result); }
    if (value.method === 'Runtime.exceptionThrown') errors.push(value.params.exceptionDetails.exception?.description || 'browser exception');
  });
  const connection = { send, socket, async evaluate(expression) {
    const value = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (value.exceptionDetails) throw new Error(value.exceptionDetails.exception?.description || 'page script failed');
    return value.result.value;
  }, close() { socket.close(); for (const request of pending.values()) { clearTimeout(request.timer); request.reject(new Error('CDP closed')); } pending.clear(); } };
  connections.push(connection); return connection;
}
async function launch() {
  await rm(join(profile, 'DevToolsActivePort'), { force: true });
  const executable = process.env.BROWSER_BIN || (process.platform === 'win32' ? 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe' : 'chromium');
  browser = spawn(executable, ['--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check', '--remote-debugging-port=0', `--user-data-dir=${profile}`, '--window-size=1440,1000', 'about:blank'], { stdio: 'ignore' });
  browser.on('error', error => errors.push(`browser: ${error.message}`));
  let port;
  await waitFor(async () => { try { port = Number((await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]); return port > 0; } catch { return false; } }, 'browser launch');
  const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  cdp = await connect(pages.find(page => page.type === 'page').webSocketDebuggerUrl);
  cdp.port = port;
  await cdp.send('Page.enable'); await cdp.send('Runtime.enable');
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false });
  await cdp.send('Page.navigate', { url: origin }); await ready();
}
const evaluate = expression => cdp.evaluate(expression);
const waitPage = (expression, label) => waitFor(() => evaluate(expression), label);
async function ready() { await waitPage("document.querySelectorAll('#theme-skins input').length >= 6 && document.querySelector('#sessions-notice').textContent.length > 0", 'formal UI connected'); }
async function click(selector) {
  const point = await evaluate(`(() => {const node=document.querySelector(${JSON.stringify(selector)}); if(!node)throw new Error('Missing control'); node.scrollIntoView({block:'center'}); const rect=node.getBoundingClientRect();return {x:rect.x+rect.width/2,y:rect.y+rect.height/2};})()`);
  await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...point });
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, ...point });
}
async function appearance() { await click('#settings-button'); await click('#settings-appearance-tab'); }
async function chooseSkin(id) { await click(`.theme-card:has(input[value="${id}"])`); }
async function chooseMode(mode) { await click(`.theme-mode:has(input[value="${mode}"])`); }
async function select(selector, value) {
  await evaluate(`(() => {const node=document.querySelector(${JSON.stringify(selector)});node.value=${JSON.stringify(String(value))};node.dispatchEvent(new Event('change',{bubbles:true}));})()`);
}
async function screenshot(name) {
  const image = await cdp.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  await writeFile(join(output, `${name}.png`), Buffer.from(image.data, 'base64'));
}
async function importFile(name, content) {
  const path = join(scratch, name); await writeFile(path, content);
  const tree = await cdp.send('DOM.getDocument');
  const node = await cdp.send('DOM.querySelector', { nodeId: tree.root.nodeId, selector: '#theme-file' });
  await cdp.send('DOM.setFileInputFiles', { nodeId: node.nodeId, files: [path] });
  await waitPage("!document.querySelector('#theme-import').disabled", 'skin file consumed');
}
const stored = () => evaluate(`localStorage.getItem(${JSON.stringify(THEME_STORAGE_KEY)})`);
const readState = () => evaluate(`JSON.parse(localStorage.getItem(${JSON.stringify(THEME_STORAGE_KEY)}))`);
const requestCount = suffix => requests.filter(item => item.method === 'POST' && item.path.endsWith(suffix)).length;

try {
  api = createFixtureApiServer({ allowOrigin: value => value === origin });
  api.on('request', request => requests.push({ method: request.method, path: new URL(request.url, 'http://127.0.0.1').pathname }));
  await new Promise(resolve => api.listen(0, '127.0.0.1', resolve)); endpoint = `http://127.0.0.1:${api.address().port}`;
  ui = await startUiServer({ host: '127.0.0.1', port: 0, endpoint, token: TEST_TOKEN }); origin = `http://127.0.0.1:${ui.address().port}`;
  await launch();
  check('clean profile starts with neutral light theme and six skin choices', await evaluate("document.documentElement.dataset.skin==='graphite' && document.documentElement.dataset.theme==='light' && document.querySelectorAll('#theme-skins input').length===6"));
  await evaluate("document.querySelector('#prompt').value='保留这段未发送的草稿'");
  await appearance();
  check('appearance settings work with management plugins absent', await evaluate("!document.querySelector('#appearance-settings-section').hidden && document.querySelector('#settings-appearance-tab').getAttribute('aria-current')==='page'"));
  await screenshot('appearance-default');
  await chooseSkin('ocean'); await chooseMode('dark'); await select('#theme-font-size', 18); await select('#theme-shape', 'soft');
  check('palette, mode, font and shape are persisted by real controls', JSON.stringify(await readState()) === JSON.stringify({ ...DEFAULT_APPEARANCE, skin: 'ocean', mode: 'dark', fontSize: 18, shape: 'soft' }));
  await screenshot('appearance-ocean-dark');
  await click('#settings-back');
  check('settings changes preserve the composer draft and live connection', await evaluate("document.querySelector('#prompt').value==='保留这段未发送的草稿' && document.querySelector('#sessions-notice').textContent.includes('当前宿主未提供本地会话存储')"));
  await cdp.send('Page.reload', { ignoreCache: true }); await ready();
  check('reload restores the skin and typography before opening settings', await evaluate("document.documentElement.dataset.skin==='ocean' && document.documentElement.dataset.theme==='dark' && getComputedStyle(document.documentElement).getPropertyValue('--chat-font-size')==='18px'"));
  // Close and reopen the browser, not only the UI component.
  const exited = once(browser, 'exit'); await cdp.send('Browser.close'); cdp.close(); await Promise.race([exited, sleep(5000)]); await stop(browser); await launch();
  check('browser restart with the same profile restores saved appearance', (await readState()).skin === 'ocean' && await evaluate("document.documentElement.dataset.theme==='dark'"));
  await appearance();
  await evaluate("document.querySelector('#theme-mode-dark').focus()");
  await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'ArrowLeft', code: 'ArrowLeft', windowsVirtualKeyCode: 37 });
  await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'ArrowLeft', code: 'ArrowLeft', windowsVirtualKeyCode: 37 });
  check('native radio groups support keyboard selection and a visible focus ring', await evaluate("document.documentElement.dataset.theme==='light' && document.activeElement.id==='theme-mode-light' && getComputedStyle(document.activeElement.nextElementSibling).outlineStyle!=='none'"));
  await chooseMode('system');
  await cdp.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'dark' }] });
  await waitPage("document.documentElement.dataset.theme==='dark'", 'system dark change');
  await cdp.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'light' }] });
  await waitPage("document.documentElement.dataset.theme==='light'", 'system light change');
  check('follow-system responds without losing its saved preference', (await readState()).mode === 'system');
  for (const item of SKINS) for (const mode of ['light', 'dark']) {
    await chooseSkin(item.id); await chooseMode(mode);
    assert.equal(await evaluate("getComputedStyle(document.documentElement).getPropertyValue('--bg').trim()"), item[mode].bg);
  }
  check('all twelve palette variants are applied by the actual settings UI', true);
  await chooseSkin('iris'); await chooseMode('light'); await screenshot('appearance-iris-light');
  await cdp.send('Browser.setDownloadBehavior', { behavior: 'allow', downloadPath: downloads });
  await click('#theme-export'); let exported;
  await waitFor(async () => { const file = (await readdir(downloads)).find(name => name.endsWith('.json')); if (!file) return false; exported = JSON.parse(await readFile(join(downloads, file), 'utf8')); return true; }, 'skin download');
  check('downloaded skin contains only public palette data', exported.format === 'mona-theme' && exported.name === '鸢尾紫' && Object.keys(exported).length === 5);
  exported.name = '<b>自定义</b>';
  await importFile('custom.json', JSON.stringify(exported)); await waitPage("document.documentElement.dataset.skin==='custom'", 'custom skin selected');
  check('import creates a custom choice and renders its name as text, not HTML', await evaluate("document.querySelector('.theme-card:has(input[value=custom]) strong').textContent==='<b>自定义</b>' && !document.querySelector('.theme-card:has(input[value=custom]) strong b')"));
  const beforeBadImport = await stored(); exported.light.bg = 'url(https://invalid.example/no-request)';
  await importFile('invalid.json', JSON.stringify(exported));
  check('invalid imports are rejected without changing the stored or visible skin', await stored() === beforeBadImport && await evaluate("document.querySelector('#theme-notice').classList.contains('error') && document.documentElement.dataset.skin==='custom'"));
  await importFile('too-large.json', ' '.repeat(32769));
  check('oversized imports have an actionable error', await evaluate("document.querySelector('#theme-notice').textContent.includes('32 KiB')"));
  await evaluate("window.__themeSetItem=Storage.prototype.setItem;Storage.prototype.setItem=function(){throw new DOMException('full','QuotaExceededError')}");
  await chooseSkin('forest');
  check('a real browser storage failure rolls the radio selection back and keeps the palette', await evaluate("document.documentElement.dataset.skin==='custom' && document.querySelector('input[name=appearance-skin]:checked').value==='custom' && document.querySelector('#theme-notice').textContent.includes('保存失败')"));
  await evaluate("Storage.prototype.setItem=window.__themeSetItem");
  await chooseSkin('forest');
  // A second real tab updates localStorage; the first receives the native storage event.
  const target = await cdp.send('Target.createTarget', { url: origin });
  const pages = await (await fetch(`http://127.0.0.1:${cdp.port}/json/list`)).json();
  const other = await connect(pages.find(page => page.id === target.targetId).webSocketDebuggerUrl);
  await waitFor(() => other.evaluate("document.querySelectorAll('#theme-skins input').length>=6"), 'second tab loaded');
  await other.evaluate(`(() => {const key=${JSON.stringify(THEME_STORAGE_KEY)};const value=JSON.parse(localStorage.getItem(key));value.skin='sand';localStorage.setItem(key,JSON.stringify(value));})()`);
  await waitPage("document.documentElement.dataset.skin==='sand'", 'native cross-tab update');
  check('native storage events synchronize open tabs', true); await cdp.send('Target.closeTarget', { targetId: target.targetId }); other.close();
  await evaluate(`localStorage.setItem(${JSON.stringify(THEME_STORAGE_KEY)},'{broken')`);
  await cdp.send('Page.reload', { ignoreCache: true }); await ready(); await appearance();
  check('corrupt stored data falls back visibly without deleting the old value', await stored() === '{broken' && await evaluate("document.documentElement.dataset.skin==='graphite' && document.querySelector('#theme-notice').classList.contains('error')"));
  await click('#theme-reset'); check('restore default repairs invalid saved preferences', JSON.stringify(await readState()) === JSON.stringify(DEFAULT_APPEARANCE));
  await click('#settings-back');
  await evaluate("(() => {const p=document.querySelector('#prompt');p.value='长任务：验证换肤不中断执行';p.dispatchEvent(new Event('input',{bubbles:true}));})()");
  await click('#send');
  await waitPage("!document.querySelector('#cancel').hidden && document.querySelector('#timeline .turn') && document.querySelector('#timeline').textContent.length>30", 'streamed task started');
  await evaluate("window.__themeTurn=document.querySelector('#timeline .turn'); document.querySelector('#prompt').value='执行中的未发送草稿'");
  const starts = requestCount('/v1/runs'), subscriptions = requests.filter(item => item.path.endsWith('/events')).length;
  await appearance(); await chooseSkin('iris'); await chooseMode('dark'); await select('#theme-font-size', 16); await click('#settings-back');
  check('switching themes never restarts, cancels or resubscribes the active task', requestCount('/v1/runs') === starts && starts === 1 && requestCount('/cancel') === 0 && requests.filter(item => item.path.endsWith('/events')).length === subscriptions);
  check('active transcript DOM and draft remain intact after theme changes', await evaluate("window.__themeTurn===document.querySelector('#timeline .turn') && document.querySelector('#prompt').value==='执行中的未发送草稿' && !document.querySelector('#cancel').hidden"));
  await screenshot('chat-running-iris-dark');
  await click('#cancel'); await waitPage("document.querySelector('#cancel').hidden", 'explicit stop completes');
  check('the explicit stop control still cancels exactly once', requestCount('/cancel') === 1);
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 900, deviceScaleFactor: 1, mobile: false });
  await click('#sidebar-toggle'); await appearance();
  check('mobile settings close the sidebar scrim and remain interactive', await evaluate("document.querySelector('#sidebar-scrim').hidden && !document.querySelector('#appearance-settings-section').hidden"));
  check('390px appearance settings have no horizontal overflow', await evaluate("document.querySelector('#settings-page').scrollWidth<=390 && document.querySelector('#appearance-settings-section').scrollWidth<=document.querySelector('#appearance-settings-section').clientWidth+1"));
  await screenshot('appearance-mobile-dark');
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 320, height: 800, deviceScaleFactor: 1, mobile: false });
  check('320px cards and navigation stay within the viewport', await evaluate("document.querySelector('#settings-page').scrollWidth<=320 && [...document.querySelectorAll('.theme-card:not([hidden])')].every(node=>node.getBoundingClientRect().right<=320)"));
  await cdp.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
  check('reduced-motion preference disables theme transitions', await evaluate("getComputedStyle(document.querySelector('.theme-card-content')).transitionDuration==='0s'"));
  const denied = await cdp.send('Page.addScriptToEvaluateOnNewDocument', { source: "Object.defineProperty(window,'localStorage',{get(){throw new DOMException('denied','SecurityError')}})" });
  await cdp.send('Page.reload', { ignoreCache: true }); await ready();
  check('denied browser storage does not prevent connecting or loading the default UI', await evaluate("document.documentElement.dataset.skin==='graphite' && document.querySelector('#sessions-notice').textContent.length>0"));
  await cdp.send('Page.removeScriptToEvaluateOnNewDocument', { identifier: denied.identifier });
  check('browser raised no unhandled JavaScript exceptions', errors.length === 0);
  await writeFile(join(output, 'result.json'), JSON.stringify({ passed: true, checks, real_browser: true, controlled_http_sse_fixture: true, real_provider: false, runtime_modified: false }, null, 2));
  console.log(`PASS ${checks.length} browser assertions; screenshots: .tmp-verify/theme-browser-report`);
} catch (error) {
  console.error(error);
  if (cdp?.socket.readyState === WebSocket.OPEN) {
    console.error(await evaluate("JSON.stringify({page:document.body?.innerText.slice(0,2500),theme:document.documentElement.dataset.theme,skin:document.documentElement.dataset.skin})").catch(() => 'page unavailable'));
    await screenshot('failure').catch(() => {});
  }
  await writeFile(join(output, 'result.json'), JSON.stringify({ passed: false, checks, error: String(error), errors }, null, 2)); process.exitCode = 1;
} finally {
  const runIds = new Set(requests.map(item => item.path.match(/^\/v1\/runs\/([^/]+)\/events$/)?.[1]).filter(Boolean));
  for (const id of runIds) await fetch(`${endpoint}/v1/runs/${id}/cancel`, { method: 'POST', headers: { Authorization: `Bearer ${TEST_TOKEN}` }, signal: AbortSignal.timeout(2000) }).catch(() => {});
  if (cdp?.socket.readyState === WebSocket.OPEN) await cdp.send('Browser.close').catch(() => {});
  for (const connection of connections) connection.close(); await stop(browser);
  for (const server of [ui, api]) { if (server) { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); } }
  await rm(scratch, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
}
