// Formal Web UI + controlled HTTP/SSE fixture + real Chromium. No model credentials or Rust build needed.
// Covers the task sidebar: structure, pointer/keyboard resizing, persistence and the search dialog.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createServer } from 'node:net';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startUiServer } from '../../../scripts/dev-web.mjs';
import { createFixtureApiServer, HOST, UI_PORT, API_PORT, TEST_TOKEN } from './fixture-server.mjs';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const output = join(root, '.tmp-verify/sidebar-browser-report');
const scratch = await mkdtemp(join(tmpdir(), 'mona-sidebar-e2e-'));
const profile = join(scratch, 'browser');
await mkdir(output, { recursive: true });
const checks = [], errors = [];
let browser, ui, api, cdp, origin = '';
const WIDTH_KEY = 'mona.web.sidebar.v1';
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
  return { send, socket, async evaluate(expression) {
    const value = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (value.exceptionDetails) throw new Error(value.exceptionDetails.exception?.description || 'page script failed');
    return value.result.value;
  }, close() { socket.close(); for (const request of pending.values()) { clearTimeout(request.timer); request.reject(new Error('CDP closed')); } pending.clear(); } };
}
const evaluate = expression => cdp.evaluate(expression);
const waitPage = (expression, label) => waitFor(() => evaluate(expression), label);
async function ready() {
  await waitPage("document.querySelector('#sessions-notice')?.textContent.length > 0 && document.querySelector('#model-label')", 'formal UI connected');
}
async function launch() {
  const reservation = createServer();
  await new Promise((resolve, reject) => {
    reservation.once('error', reject);
    reservation.listen(0, '127.0.0.1', () => { reservation.off('error', reject); resolve(); });
  });
  const port = reservation.address().port;
  await new Promise(resolve => reservation.close(resolve));
  const executable = process.env.BROWSER_BIN || (process.platform === 'win32' ? 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe' : 'chromium');
  browser = spawn(executable, [
    '--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check',
    '--edge-skip-compat-layer-relaunch', `--remote-debugging-port=${port}`,
    `--user-data-dir=${profile}`, '--window-size=1440,1000', 'about:blank',
  ], { stdio: 'ignore' });
  browser.on('error', error => errors.push(`browser: ${error.message}`));
  await waitFor(async () => {
    if (browser.exitCode != null) throw new Error(`browser exited before DevTools became ready: ${browser.exitCode}`);
    try { return (await fetch(`http://127.0.0.1:${port}/json/list`, { signal: AbortSignal.timeout(500) })).ok; } catch { return false; }
  }, 'browser launch');
  const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  cdp = await connect(pages.find(page => page.type === 'page').webSocketDebuggerUrl);
  await cdp.send('Page.enable'); await cdp.send('Runtime.enable');
  // Headless pages otherwise drop renderer key events (Tab and Escape are browser-level and still work).
  await cdp.send('Emulation.setFocusEmulationEnabled', { enabled: true });
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false });
  await cdp.send('Page.navigate', { url: origin }); await ready();
}
const sidebarWidth = () => evaluate("Math.round(document.querySelector('#chat-sidebar').getBoundingClientRect().width)");
const storedWidth = () => evaluate(`localStorage.getItem(${JSON.stringify(WIDTH_KEY)})`);
async function point(selector) {
  return evaluate(`(() => {const node=document.querySelector(${JSON.stringify(selector)}); if(!node)throw new Error('Missing control'); const rect=node.getBoundingClientRect();return {x:rect.x+rect.width/2,y:rect.y+rect.height/2};})()`);
}
async function click(selector) {
  const at = await point(selector);
  await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...at });
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, ...at });
}
async function drag(selector, deltaX) {
  const at = await point(selector);
  await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...at });
  for (const step of [0.5, 1]) await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', button: 'left', buttons: 1, x: at.x + deltaX * step, y: at.y });
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, x: at.x + deltaX, y: at.y });
}
async function tabTo(selector, attempts = 20) {
  for (let index = 0; index < attempts; index += 1) {
    await press('Tab', 'Tab', 9);
    if (await evaluate(`document.activeElement === document.querySelector(${JSON.stringify(selector)})`)) return true;
  }
  return false;
}
async function press(key, code, virtualKeyCode, modifiers = 0) {
  await cdp.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key, code, modifiers, windowsVirtualKeyCode: virtualKeyCode, nativeVirtualKeyCode: virtualKeyCode });
  await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key, code, modifiers, windowsVirtualKeyCode: virtualKeyCode, nativeVirtualKeyCode: virtualKeyCode });
}
async function screenshot(name) {
  const image = await cdp.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  await writeFile(join(output, `${name}.png`), Buffer.from(image.data, 'base64'));
}

try {
  // The fixture pins its CORS allowlist to its own ports, so this harness uses them too.
  api = createFixtureApiServer();
  await new Promise((resolve, reject) => { api.once('error', reject); api.listen(API_PORT, HOST, () => resolve()); });
  ui = await startUiServer({ host: HOST, port: UI_PORT, endpoint: `http://${HOST}:${API_PORT}`, token: TEST_TOKEN });
  origin = `http://${HOST}:${UI_PORT}`;
  await launch();

  check('the sidebar exposes 新建任务, the task list and settings, and no workspace/current-task leftovers',
    await evaluate(`(() => {
      const sidebar = document.querySelector('#chat-sidebar');
      return sidebar.textContent.includes('新建任务') && sidebar.textContent.includes('任务')
        && sidebar.querySelector('#sessions-list') && sidebar.querySelector('#settings-button')
        && !sidebar.querySelector('#current-task') && !sidebar.querySelector('#current-task-title')
        && !sidebar.querySelector('.section-label') && !sidebar.querySelector('.project')
        && !sidebar.querySelector('#sessions-search');
    })()`));
  check('search is a dialog behind the brand button, not a sidebar row',
    await evaluate("document.querySelector('#search-button') !== null && !document.querySelector('#search-dialog').open"));
  check('the task header carries the section label, its collapse control and the section actions',
    await evaluate(`(() => {
      const heading = document.querySelector('.sessions-heading');
      return document.querySelector('#sessions-collapse').getAttribute('aria-expanded') === 'true'
        && heading.textContent.includes('任务')
        && heading.querySelector('#sessions-options') !== null
        && heading.querySelector('#sessions-new') !== null
        && document.querySelector('#sessions-options-menu #sessions-refresh') !== null;
    })()`));
  const headingSpot = await point('.sessions-heading');
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: headingSpot.x, y: headingSpot.y });
  await waitPage("getComputedStyle(document.querySelector('.sessions-heading-actions')).opacity === '1'", 'section actions revealed');
  check('the section actions appear when the header is hovered', true);
  await click('#sessions-collapse');
  check('the collapse control hides the task list and remembers the choice',
    await evaluate("document.querySelector('.sessions-region').classList.contains('is-collapsed') && document.querySelector('#sessions-collapse').getAttribute('aria-expanded') === 'false' && localStorage.getItem('mona.web.tasks.v1') === '1'"));
  await cdp.send('Page.navigate', { url: origin }); await ready();
  check('the collapsed task section survives a reload', await evaluate("document.querySelector('.sessions-region').classList.contains('is-collapsed')"));
  await click('#sessions-collapse');
  check('expanding the task section clears the stored choice',
    await evaluate("!document.querySelector('.sessions-region').classList.contains('is-collapsed') && localStorage.getItem('mona.web.tasks.v1') === '0'"));
  check('the resizer is an exposed window splitter with current values',
    await evaluate("(() => {const r=document.querySelector('#sidebar-resizer');return r.getAttribute('role')==='separator' && r.tabIndex===0 && Number(r.getAttribute('aria-valuenow'))===264 && Number(r.getAttribute('aria-valuemin'))===208 && Number(r.getAttribute('aria-valuemax'))===460;})()"));
  check('the default sidebar is 264px wide', await sidebarWidth() === 264);

  await drag('#sidebar-resizer', 96);
  const dragged = await sidebarWidth();
  check(`dragging the resizer right widens the sidebar (${dragged}px)`, dragged === 360);
  check('the dragged width is persisted for this browser', await storedWidth() === '360');
  await screenshot('sidebar-resized');
  const zoom = await cdp.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false, clip: { x: 352, y: 200, width: 22, height: 80, scale: 8 } });
  await writeFile(join(output, 'sidebar-boundary-zoom.png'), Buffer.from(zoom.data, 'base64'));

  await drag('#sidebar-resizer', 800);
  check('the sidebar stops at its maximum width', await sidebarWidth() === 460);
  await drag('#sidebar-resizer', -800);
  check('the sidebar stops at its minimum width', await sidebarWidth() === 208);

  check('the resizer is reachable with Tab and shows a focus ring',
    await tabTo('#sidebar-resizer') && await evaluate("document.querySelector('#sidebar-resizer').matches(':focus-visible')"));
  // Headless Chromium consumes arrow keys as scroll commands before the renderer sees them, so the
  // arrow steps dispatch the keydown a focused splitter receives; Tab reachability above is the real path.
  const arrow = key => evaluate(`document.querySelector('#sidebar-resizer').dispatchEvent(new KeyboardEvent('keydown', { key: ${JSON.stringify(key)}, bubbles: true, cancelable: true }))`);
  await arrow('ArrowRight'); await arrow('ArrowRight');
  const keyboardWidth = await sidebarWidth();
  check(`arrow keys resize the sidebar by one step (${keyboardWidth}px, stored ${await storedWidth()})`, keyboardWidth === 240 && await storedWidth() === '240');
  await arrow('ArrowLeft');
  check('arrow keys resize in the other direction too', await sidebarWidth() === 224);

  await cdp.send('Page.navigate', { url: origin }); await ready();
  check('the stored width is restored after a reload', await sidebarWidth() === 224 && await storedWidth() === '224');

  await click('#search-button');
  await waitPage("document.querySelector('#search-dialog').open", 'search dialog open');
  check('the search dialog opens as a modal dialog with the brand button marked expanded',
    await evaluate("document.querySelector('#search-dialog').matches(':modal') && document.querySelector('#search-button').getAttribute('aria-expanded')==='true'"));
  check('this fixture host keeps no local tasks, so the query field is disabled and says so',
    await evaluate("document.querySelector('#sessions-search').disabled && document.querySelector('#search-results').textContent.includes('未提供本地会话存储')"));
  check('the dialog offers only actions that exist', await evaluate(`(() => {
    const ids = [...document.querySelectorAll('.search-actions .search-result')].map(node => node.id);
    return ids.join() === 'search-new,search-settings,search-refresh';
  })()`));
  await screenshot('sidebar-search-dialog');
  await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, x: 2, y: 2 });
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, x: 2, y: 2 });
  await waitPage("!document.querySelector('#search-dialog').open && document.querySelector('#search-button').getAttribute('aria-expanded')==='false'", 'search dialog closed by backdrop');
  check('the search backdrop closes the dialog and restores the collapsed state', true);

  await press('k', 'KeyK', 75, 2);
  check('Ctrl+K opens the search dialog from the keyboard', await evaluate("document.querySelector('#search-dialog').open"));
  await press('Escape', 'Escape', 27);
  await waitPage("!document.querySelector('#search-dialog').open && document.querySelector('#search-button').getAttribute('aria-expanded')==='false'", 'search dialog closed by Escape');
  check('Escape closes the search dialog and marks the brand button collapsed', true);

  await click('#prompt');
  check('the composer owns textarea focus without adding duplicate focus chrome', await evaluate(`(() => {
    const composer = getComputedStyle(document.querySelector('.composer'));
    const field = getComputedStyle(document.querySelector('#prompt'));
    return document.activeElement===document.querySelector('#prompt')
      && composer.outlineStyle==='none' && composer.boxShadow==='none'
      && field.outlineStyle==='none' && field.boxShadow==='none';
  })()`));
  await screenshot('composer-focus');

  await click('#sidebar-toggle');
  check('collapsing the sidebar hides the resizer', await evaluate("getComputedStyle(document.querySelector('#sidebar-resizer')).display === 'none'"));
  await click('#sidebar-toggle');
  check('expanding the sidebar restores the chosen width', await sidebarWidth() === 224);

  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 900, deviceScaleFactor: 1, mobile: false });
  await waitPage("matchMedia('(max-width: 680px)').matches && getComputedStyle(document.querySelector('#sidebar-resizer')).display === 'none' && document.querySelector('#sidebar-toggle').getBoundingClientRect().width > 0 && document.querySelector('#chat-sidebar').inert", 'mobile layout applied and responsive listener settled');
  await click('#sidebar-toggle');
  await waitPage("document.querySelector('.shell').classList.contains('sidebar-open')", 'mobile drawer opened');
  check('the mobile drawer hides the resizer and keeps the task list reachable',
    await evaluate("getComputedStyle(document.querySelector('#sidebar-resizer')).display === 'none' && !document.querySelector('#sessions-list').closest('aside').inert"));
  await screenshot('sidebar-mobile-drawer');
  check('390px layout has no horizontal overflow', await evaluate("document.documentElement.scrollWidth<=390 && document.querySelector('.shell').scrollWidth<=390"));
  check('no unhandled page exception occurred', errors.length === 0);
  await writeFile(join(output, 'result.json'), JSON.stringify({ passed: true, checks }, null, 2));
  console.log(`PASS ${checks.length} sidebar checks; screenshots in .tmp-verify/sidebar-browser-report`);
} catch (error) {
  console.error(error);
  if (cdp) console.error(String(await evaluate("JSON.stringify({width:document.querySelector('#chat-sidebar')?.getBoundingClientRect().width,body:document.body?.innerText.slice(0,600)})").catch(() => 'no diagnostics')));
  await screenshot('failure').catch(() => {});
  await writeFile(join(output, 'result.json'), JSON.stringify({ passed: false, checks, error: String(error), errors }, null, 2));
  process.exitCode = 1;
} finally {
  cdp?.close();
  await stop(browser); await new Promise(resolve => api?.close(resolve)); await new Promise(resolve => ui?.close(resolve));
  await rm(scratch, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
}
