// Formal application + isolated HTTP fixture; verifies the empty task and its shared live composer.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { startUiServer } from '../../../scripts/dev-web.mjs';
import { createFixtureApiServer, TEST_TOKEN } from './fixture-server.mjs';
const output = resolve(process.env.MONA_TEST_REPORT_DIR || '.tmp-verify/new-task/after');
const scratch = await mkdtemp(join(tmpdir(), 'mona-new-task-')), profile = join(scratch, 'browser');
await mkdir(output, { recursive: true });
let api, ui, browser, socket, origin = '', serial = 0;
const pending = new Map(), checks = [], errors = [], requests = [];
const sleep = ms => new Promise(r => setTimeout(r, ms));
async function wait(fn, label) { const until = Date.now() + 20000; while (Date.now() < until) { if (await fn()) return; await sleep(60); } throw Error('Timeout: ' + label); }
function send(method, params = {}) { return new Promise((resolve, reject) => { const id = ++serial, timer = setTimeout(() => { pending.delete(id); reject(Error('CDP timeout: ' + method)); }, 20000); pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params })); }); }
async function evaluate(expression) { const value = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }); if (value.exceptionDetails) throw Error(value.exceptionDetails.exception?.description || 'Browser error'); return value.result.value; }
async function check(name, expression) { assert.ok(await evaluate(expression), name); checks.push(name); console.log('PASS ' + name); }
async function screen(width, height) { await send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false }); await sleep(150); }
async function shot(name) { const image = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false }); await writeFile(join(output, name + '.png'), Buffer.from(image.data, 'base64')); }
try {
  api = createFixtureApiServer({ management: true, allowOrigin: value => value === origin }); await new Promise(r => api.listen(0, '127.0.0.1', r));
  ui = await startUiServer({ host: '127.0.0.1', port: 0, endpoint: `http://127.0.0.1:${api.address().port}`, token: TEST_TOKEN }); origin = `http://127.0.0.1:${ui.address().port}`;
  browser = spawn(process.env.BROWSER_BIN || (process.platform === 'win32' ? 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe' : 'chromium'), ['--headless=new', '--disable-gpu', '--no-first-run', '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank'], { stdio: 'ignore' });
  let port; await wait(async () => { try { port = Number((await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]); return port > 0; } catch { return false; } }, 'browser');
  const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json(); socket = new WebSocket(pages.find(p => p.type === 'page').webSocketDebuggerUrl); await once(socket, 'open');
  socket.addEventListener('message', event => { const value = JSON.parse(event.data), item = pending.get(value.id); if (item) { pending.delete(value.id); clearTimeout(item.timer); value.error ? item.reject(Error(JSON.stringify(value.error))) : item.resolve(value.result); } if (value.method === 'Runtime.exceptionThrown') errors.push(value.params.exceptionDetails.exception?.description || 'Browser exception'); if (value.method === 'Network.requestWillBeSent') requests.push({ method: value.params.request.method, url: value.params.request.url }); });
  await send('Page.enable'); await send('Runtime.enable'); await send('Network.enable'); await send('Emulation.setFocusEmulationEnabled', { enabled: true }); await screen(1536, 984);
  await send('Page.navigate', { url: origin });
  await wait(() => evaluate("document.querySelector('#model-label')?.textContent==='fixture-chat' && !document.querySelector('#prompt').disabled"), 'formal app');
  await evaluate("window.originalPrompt=document.querySelector('#prompt'); window.originalComposer=document.querySelector('.composer'); window.box=s=>document.querySelector(s).getBoundingClientRect()");
  await check('new task contains only Mona mark/title and the shared composer, with no suggestion or marketing copy', "document.querySelector('#welcome h1').textContent==='Mona' && document.querySelector('#welcome .brand-mark') && !document.querySelector('#welcome p,.suggestions,[data-prompt]') && document.querySelectorAll('#prompt').length===1");
  await check('empty title/input group is centered together rather than keeping the input on the bottom', "(()=>{const w=box('#chat-workspace'),h=box('#welcome'),c=box('.composer');return c.top>h.bottom && c.top-h.bottom>=24 && c.bottom<innerHeight-180 && Math.abs((h.top+c.bottom)/2-(w.top+48+(w.height-48)/2))<35 && Math.abs((h.left+h.width/2)-(c.left+c.width/2))<2})()");
  await check('composer uses the registered 22px rounded surface, subtle elevation and 34px circular send icon', "getComputedStyle(originalComposer).borderRadius==='22px' && getComputedStyle(originalComposer).boxShadow!=='none' && box('#send').width===34 && document.querySelector('#send .send-arrow') && getComputedStyle(document.querySelector('#send')).backgroundColor==='rgb(57, 100, 254)'");
  await check('new task has a two-line draft floor and compact model/send controls', "parseFloat(getComputedStyle(originalPrompt).minHeight)===52 && box('.composer').height>=108 && box('.composer').height<170 && box('#model-button').right<box('#send').left");
  await check('uninstalled planner and projects show no fake mode control or empty metadata strip', "!document.querySelector('#planner-mode-chip') && document.querySelector('#composer-ui-modes').hidden && !document.querySelector('#composer-add').hidden && document.querySelector('.project-chip').hidden && getComputedStyle(document.querySelector('.composer-context')).display==='none'");
  await check('placeholder never advertises unimplemented attachment or mention handling', "!originalPrompt.placeholder.includes('@') && !originalPrompt.placeholder.includes('/') && !document.querySelector('.composer button[aria-label*=上传]')");
  const menuChecks = await evaluate("(async()=>{const m=await import('/apps/web/test/command-menu-checks.mjs');return m.commandMenuChecks(['model']);})()");
  for (const name of menuChecks) { checks.push(name); console.log('PASS '+name); }
  await evaluate("document.querySelector('#composer-add').click()"); await shot('command-menu-model-light');
  await evaluate("document.querySelector('[data-command=model]').click()");
  await wait(() => evaluate("document.querySelector('#model-picker-dialog').open"), 'existing model picker');
  await check('model command opens the existing picker without starting a Run', "document.querySelectorAll('#model-picker-dialog').length===1&&document.querySelector('#composer-command-menu').hidden");
  assert.equal(requests.filter(r => r.method==='POST' && /\/v1\/runs$/.test(r.url)).length, 0);
  await evaluate("[...document.querySelectorAll('.model-picker-row')].find(n=>n.querySelector('strong').textContent==='fixture-code').click()");
  await wait(() => evaluate("!document.querySelector('#model-picker-dialog').open&&document.querySelector('#model-label').textContent==='fixture-code'"), 'model selection saved');
  checks.push('model command selection uses the existing saved model configuration');
  await shot('new-task-light');
  await evaluate("originalPrompt.value='保留这段草稿与选择区';originalPrompt.dispatchEvent(new Event('input',{bubbles:true}));originalPrompt.focus();originalPrompt.setSelectionRange(2,7)");
  await sleep(100); await screen(1100, 760);
  await check('responsive empty layout retains the real textarea, draft and selection', "originalPrompt===document.querySelector('#prompt') && originalPrompt.value==='保留这段草稿与选择区' && originalPrompt.selectionStart===2 && originalPrompt.selectionEnd===7");
  await evaluate("originalPrompt.value='检查一下配置，不要修改文件';originalPrompt.dispatchEvent(new Event('input',{bubbles:true}));document.querySelector('#send').click()");
  await wait(() => evaluate("document.querySelector('#welcome').hidden && document.querySelector('#timeline .turn')"), 'task accepted');
  await check('first message docks the same composer and reveals the timeline without replacing input nodes', "originalComposer===document.querySelector('.composer') && originalPrompt===document.querySelector('#prompt') && getComputedStyle(document.querySelector('#main')).display!=='none' && box('.composer').bottom>innerHeight-90");
  await wait(() => evaluate("document.querySelector('#cancel').hidden && document.querySelector('.turn[data-ending=completed]')"), 'task settled');
  await check('send arrow survives busy and completion updates', "document.querySelectorAll('#send svg').length===2 && document.querySelector('#send').dataset.action==='send' && getComputedStyle(document.querySelector('.send-arrow')).display!=='none'");
  await shot('conversation-docked');
  await evaluate("document.querySelector('#new-chat').click()");
  await wait(() => evaluate("!document.querySelector('#welcome').hidden && !document.querySelector('#timeline .turn')"), 'new task');
  await screen(1536, 984);
  await check('new task returns to the centered layout using the existing composer', "originalComposer===document.querySelector('.composer') && originalPrompt===document.querySelector('#prompt') && box('.composer').bottom<innerHeight-180");
  await evaluate("document.querySelector('#settings-button').click();document.querySelector('#settings-appearance-tab').click();document.querySelector('#theme-mode-dark').click();document.querySelector('#settings-back').click()"); await sleep(180);
  await check('actual dark palette preserves the new input shape and blue send affordance', "document.documentElement.dataset.theme==='dark' && getComputedStyle(originalComposer).borderRadius==='22px' && getComputedStyle(document.querySelector('#send')).backgroundColor==='rgb(103, 158, 254)'"); await shot('new-task-dark');
  await evaluate("document.querySelector('#composer-add').click()");
  await check('dark command menu keeps the real model entry, tokenized colors and full composer width', "(()=>{const menu=document.querySelector('#composer-command-menu'),item=menu.querySelector('[data-command=model]');return menu.matches(':popover-open')&&item&&!item.disabled&&getComputedStyle(item).color===getComputedStyle(document.body).color&&Math.abs(box('#composer-command-menu').width-box('.composer').width)<1})()");
  await shot('command-menu-model-dark');
  await evaluate("document.querySelector('#composer-command-menu [data-command]').dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true,cancelable:true}))");
  await evaluate("document.querySelector('#settings-button').click();document.querySelector('#settings-appearance-tab').click();document.querySelector('#theme-mode-light').click();document.querySelector('#settings-back').click()");
  for (const [width, height] of [[390,900],[320,800],[390,320],[900,360]]) {
    await screen(width,height); await sleep(100);
    await check(`${width}x${height} empty task keeps title, input and send on-screen without horizontal overflow`, "(()=>{const a=box('#welcome'),b=box('#prompt'),c=box('#send');return document.documentElement.scrollWidth<=innerWidth && a.top>=0 && a.bottom<=innerHeight && b.width>100 && b.top>=0 && c.right<=innerWidth && c.bottom<=innerHeight})()"); await shot(`new-task-${width}x${height}`);
    await evaluate("document.querySelector('#composer-add').click()");
    await check(`${width}x${height} command menu remains in the viewport without losing its real action`, "(()=>{const a=box('#composer-command-menu'),menu=document.querySelector('#composer-command-menu');return menu.matches(':popover-open')&&a.left>=0&&a.top>=0&&a.right<=innerWidth+1&&a.bottom<=innerHeight+1&&!menu.querySelector('[data-command=model]').disabled})()");
    await shot(`command-menu-model-${width}x${height}`);
    await evaluate("document.querySelector('#composer-command-menu [data-command]').dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true,cancelable:true}))");
  }
  await screen(390, 900);
  await evaluate("originalPrompt.value=Array.from({length:70},(_,i)=>'第 '+i+' 行长草稿').join('\\n');originalPrompt.dispatchEvent(new Event('input',{bubbles:true}))"); await sleep(180);
  await check('long draft grows within its cap and does not cover the send action', "box('#prompt').height<=220 && originalPrompt.scrollHeight>originalPrompt.clientHeight && box('#send').bottom<innerHeight");
  await evaluate("originalPrompt.value='';originalPrompt.dispatchEvent(new Event('input',{bubbles:true}))"); await sleep(150);
  await check('clearing a long draft restores the centered two-line input', "box('#prompt').height===52 && document.querySelector('#send').disabled");
  assert.equal(errors.length, 0, errors.join('\n')); checks.push('no unhandled JavaScript errors');
  await writeFile(join(output, 'report.json'), JSON.stringify({ passed: true, checks, errors, taskPosts: requests.filter(r=>r.method==='POST' && /\/v1\/runs$/.test(r.url)).length }, null, 2));
  console.log(`PASS ${checks.length} new task assertions`);
} catch (error) { console.error(error); console.error(errors); if (socket?.readyState===1) { console.error(await evaluate("document.body.innerText.slice(-2500)").catch(()=>'')); await shot('failure').catch(()=>{}); } await writeFile(join(output,'report.json'),JSON.stringify({passed:false,checks,error:String(error),errors},null,2));process.exitCode=1; }
finally { if(socket?.readyState===1){await send('Browser.close').catch(()=>{});socket.close();}for(const item of pending.values())clearTimeout(item.timer);if(browser?.exitCode==null){browser.kill('SIGKILL');await sleep(350);}ui?.closeAllConnections();api?.closeAllConnections();if(ui)await new Promise(r=>ui.close(r));if(api)await new Promise(r=>api.close(r));await rm(scratch,{recursive:true,force:true,maxRetries:5,retryDelay:100}).catch(()=>{}); }
