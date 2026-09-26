// Formal shell, real view controllers, deterministic bounded data. No user service or paid model.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { startUiServer } from '../../../scripts/dev-web.mjs';
const capture = process.env.MONA_CAPTURE_BASELINE === '1';
const output = resolve(process.env.MONA_TEST_REPORT_DIR || '.tmp-verify/layout-batch3/after');
const scratch = await mkdtemp(join(tmpdir(), 'mona-layout-')), profile = join(scratch, 'browser');
await mkdir(output, { recursive: true });
let upstream, server, browser, socket, seq = 0; const pending = new Map(), checks = [], errors = [], measures = [];
const sleep = ms => new Promise(r => setTimeout(r, ms));
async function wait(fn, label) { for (let i = 0; i < 250; i++) { if (await fn()) return; await sleep(60); } throw Error('Timeout: ' + label); }
function send(method, params = {}) { return new Promise((resolve, reject) => { const id = ++seq, timer = setTimeout(() => { pending.delete(id); reject(Error('CDP timeout: ' + method)); }, 20000); pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params })); }); }
async function evaluate(expression) { const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true, replMode: true }); if (result.exceptionDetails) throw Error(result.exceptionDetails.exception?.description || 'Browser error'); return result.result.value; }
async function check(name, expression) { const value = await evaluate(expression), passed = Boolean(value); checks.push({ name, passed }); console.log((passed ? 'PASS ' : 'FAIL ') + name); if (!capture) assert.ok(passed, name); }
async function screen(width, height = 900) { await send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false }); await sleep(120); }
async function shot(name) { await sleep(80); const image = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false }); await writeFile(join(output, name + '.png'), Buffer.from(image.data, 'base64')); }
async function key(key, code, windowsVirtualKeyCode, modifiers = 0) { await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key, code, windowsVirtualKeyCode, modifiers }); await send('Input.dispatchKeyEvent', { type: 'keyUp', key, code, windowsVirtualKeyCode, modifiers }); }
try {
  upstream = await startUiServer({ host: '127.0.0.1', port: 0, endpoint: 'http://127.0.0.1:1', token: 'layout-fixture-token-without-user-credentials' });
  const html = (await readFile('apps/web/index.html', 'utf8')).replace(/<script\b[^>]*src="[^"]*app\.mjs"[^>]*><\/script>/g, '');
  server = createServer(async (req, res) => { try { if (req.url === '/') { res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' }); res.end(html); return; } const r = await fetch(`http://127.0.0.1:${upstream.address().port}${req.url}`); res.writeHead(r.status, { 'Content-Type': r.headers.get('content-type') || 'application/octet-stream' }); res.end(Buffer.from(await r.arrayBuffer())); } catch { res.writeHead(500); res.end(); } });
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  browser = spawn(process.env.BROWSER_BIN || (process.platform === 'win32' ? 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe' : 'chromium'), ['--headless=new', '--disable-gpu', '--no-first-run', '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank'], { stdio: 'ignore' });
  let port; await wait(async () => { try { port = +(await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]; return port > 0; } catch { return false; } }, 'browser');
  const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json(); socket = new WebSocket(pages.find(p => p.type === 'page').webSocketDebuggerUrl); await once(socket, 'open');
  socket.addEventListener('message', event => { const v = JSON.parse(event.data), p = pending.get(v.id); if (p) { pending.delete(v.id); clearTimeout(p.timer); v.error ? p.reject(Error(JSON.stringify(v.error))) : p.resolve(v.result); } if (v.method === 'Runtime.exceptionThrown') errors.push(v.params.exceptionDetails.exception?.description || 'browser exception'); });
  await send('Page.enable'); await send('Runtime.enable'); await send('Emulation.setFocusEmulationEnabled', { enabled: true }); await screen(1440, 1000);
  await send('Page.navigate', { url: `http://127.0.0.1:${server.address().port}/` }); await wait(() => evaluate("Boolean(document.querySelector('#files-panel'))"), 'formal markup');
  // Optional only for the before-capture: absence is a measured missing implementation, not a passing fallback.
  const hasLayout = !capture || await readFile('apps/web/shell-layout.mjs', 'utf8').then(() => true, () => false);
  await evaluate(`window.ready = (async () => {
    const {RightPane}=await import('/apps/web/right-pane.mjs'); const {ConversationMetrics,validateStatistics}=await import('/apps/web/conversation-metrics.mjs');
    window.pane=new RightPane(); pane.setScope('session:layout'); window.root=document.querySelector('.shell'); window.workspace=document.querySelector('#chat-workspace');window.main=document.querySelector('#main');window.draft=document.querySelector('#prompt');
    window.rect=selector=>document.querySelector(selector).getBoundingClientRect();
    ${hasLayout ? "const {NavigationLayout,ComposerLayout}=await import('/apps/web/shell-layout.mjs');window.navLayout=new NavigationLayout({pane});window.composerLayout=new ComposerLayout();const {ConversationControls}=await import('/apps/web/conversation-controls.mjs');window.reading=new ConversationControls({main,timeline:document.querySelector('#timeline'),loadOlder:async()=>{},hasOlder:()=>false,onQuote:()=>{},onSide:()=>{},canSide:()=>false});" : ''}
    document.querySelector('#welcome').hidden=true;const timeline=document.querySelector('#timeline');for(let i=0;i<28;i++){const turn=document.createElement('article');turn.className='turn';turn.innerHTML='<div class="user-message">第 '+(i+1)+' 个任务</div><div class="assistant-body message-text"><p>会话阅读区域与输入器需要共享宽度。文件浏览不能挤压输入和操作按钮。</p></div>';timeline.append(turn);}
    document.querySelector('#composer-add').hidden=false;const leading=document.querySelector('#composer-ui-modes');leading.hidden=false;leading.innerHTML='<span class="permission-select"><button class="permission-trigger"><span class="permission-trigger-icon">◈</span><span class="permission-trigger-label">工作区内修改</span><span class="permission-chevron">⌄</span></button></span><button class="planner-mode-chip"><span class="planner-mode-glyph">▤</span><span>计划</span></button>';
    const actions=document.querySelector('#composer-ui-actions');actions.hidden=false;actions.innerHTML='<button id="model-button"><span id="model-label">qwen-very-long-enterprise-model-name-2026</span></button>';
    const status=document.querySelector('#conversation-status-slots');status.hidden=false;const monitor=document.createElement('div');monitor.id='conversation-metrics';monitor.className='conversation-metrics';status.append(monitor);
    window.metrics=new ConversationMetrics(monitor,{statistics:async()=>metrics.value});metrics.session={id:'layout',turn_count:13};metrics.value=validateStatistics({session_id:'layout',revision:3,latest_run_id:'run-layout',turns:13,steps:132,latest_step:132,active:false,reported_tokens:107477,input_tokens:105025,output_tokens:2452,cache_read_tokens:82560,cache_write_tokens:0,model_time_ms:58200,tool_time_ms:60000,ttft_ms:1600,ttft_samples:1,decode_ms:42000,decode_tokens:2452,model_calls:140,usage_complete:true,context:{run_id:'run-layout',observed_at:1,tokens:12600,capacity:262000,system_tokens:2000,tool_tokens:7300,message_tokens:3300,provider_anchored:true}},'layout');metrics.render();
    const content=document.createElement('section');content.className='fixture-pane-body';content.textContent='工作区资料预览';pane.openTab({id:'file:notes',scope:'session:layout',title:'notes.md',kind:'file',node:content});window.preview=content;
    window.setDraft=value=>{draft.value=value;draft.dispatchEvent(new Event('input',{bubbles:true}));window.composerLayout?.schedule?.();};
    window.setLeft=value=>{if(window.navLayout)navLayout.applyWidth(value);else root.style.setProperty('--sidebar-width',value+'px');};
    return true;
  })().catch(error=>{window.fixtureError=error.stack;throw error})`);
  await wait(() => evaluate("(()=>{if(window.fixtureError)throw Error(window.fixtureError);return Boolean(window.metrics?.value && window.preview)})()"), 'views'); await sleep(200);
  await check('transcript, composer and statistics share the same horizontal axis', "(()=>{const a=rect('#timeline'),b=rect('.composer'),c=rect('#conversation-metrics');return Math.abs(a.x-b.x)<2&&Math.abs(a.width-b.width)<2&&Math.abs(c.x-b.x)<2&&Math.abs(c.width-b.width)<2})()");
  await check('bottom pills show real cache hit and context percentages', "document.querySelector('[data-metric=tokens]').textContent.includes('79%')&&document.querySelector('[data-metric=context]').textContent.includes('5%')&&document.querySelector('[data-metric=performance]').textContent.includes('58 tok/s')");
  await evaluate("metrics.show('performance')");
  await check('performance popup reports model time, tool time, average TTFT and TPS', "(()=>{const p=metrics.panel;return p.classList.contains('is-performance')&&p.textContent.includes('58.2秒')&&p.textContent.includes('1分0秒')&&p.textContent.includes('首 token 平均（TTFT）1.6秒')&&p.textContent.includes('58 tok/s')})()");
  await shot('statistics-performance');
  await evaluate("metrics.show('tokens')");
  await check('token popup splits uncached input, cache reads and output without double counting', "(()=>{const p=metrics.panel;return p.textContent.includes('107,477 tok')&&p.textContent.includes('缓存命中79%')&&p.textContent.includes('未缓存输入22,465 tok')&&p.textContent.includes('缓存读取82,560 tok')&&p.textContent.includes('输出2,452 tok')})()");
  await shot('dsh-composer-statistics');
  await evaluate("metrics.show('context')");
  await check('context popup keeps approximate values and the three colored components', "(()=>{const p=metrics.panel;return p.textContent.includes('~12.6K / 262K')&&p.textContent.includes('系统提示词~2K')&&p.textContent.includes('工具定义~7.3K')&&p.textContent.includes('对话消息~3.3K')&&p.querySelectorAll('.context-segment').length===3&&[...p.querySelectorAll('.context-segment')].every(n=>Number(n.getAttribute('width'))>0)})()");
  await shot('statistics-context');
  await evaluate("metrics.show(null)");
  await evaluate("pane.applyWidth(700);setLeft(460)"); await sleep(120);
  measures.push(await evaluate("({width:innerWidth,sidebar:rect('#chat-sidebar').width,center:rect('#chat-workspace').width,right:rect('#files-panel').width})"));
  await check('resizing the left navigation never squeezes the conversation below its 400px floor', "rect('#chat-workspace').width>=399 && document.documentElement.scrollWidth<=innerWidth");
  await evaluate("setLeft(264);pane.applyWidth(700)"); await screen(1600); await screen(1100);
  await check('a usable three-column layout does not become fullscreen solely at 1180px', "!document.querySelector('#files-panel').classList.contains('is-fullscreen')&&rect('#chat-workspace').width>=399");
  await screen(1600);
  await check('returning to a wider screen restores the requested right width, not the temporarily shrunken width', "Math.abs(rect('#files-panel').width-700)<2");
  await evaluate("pane.setOpen(false);setDraft('短草稿')"); await sleep(120); await evaluate("window.smallDraft=rect('#prompt').height;setDraft(Array.from({length:9},(_,i)=>'草稿行 '+i).join('\\n'))"); await sleep(120);
  await check('draft automatically grows without manual textarea resizing', "rect('#prompt').height>smallDraft+70 && getComputedStyle(draft).resize==='none'");
  await evaluate("setDraft('恢复短草稿')"); await sleep(120);
  await check('shortening or programmatically replacing a draft releases the occupied height', "Math.abs(rect('#prompt').height-smallDraft)<2");
  await evaluate("pane.setOpen(true);pane.applyWidth(420);setDraft('短草稿')"); await screen(1440,1000); await shot('desktop-three-columns');
  for(const [width,height]of [[1024,768],[800,600],[390,900],[320,800],[390,320],[900,360]]){
    await screen(width,height);await evaluate("pane.setOpen(false);setDraft(Array.from({length:40},(_,i)=>'这是一段跨学科任务草稿 '+i).join('\\n'))");await sleep(180);
    await check(width+'x'+height+' keeps the transcript, textarea and send controls usable',"main.clientHeight>=Math.min(96,innerHeight*0.25) && rect('#prompt').height>=30 && rect('#send').bottom<=innerHeight+1 && document.documentElement.scrollWidth<=innerWidth");
    await check(width+'x'+height+' statistics occupy one row aligned with the composer',"(()=>{const pills=[...document.querySelectorAll('.conversation-stat')].map(n=>n.getBoundingClientRect()),a=rect('.composer'),b=rect('#conversation-metrics');return pills.length===3&&pills.every(p=>p.width>0&&Math.abs(p.y-pills[0].y)<2)&&Math.abs(a.x-b.x)<2&&Math.abs(a.width-b.width)<2})()");
    await evaluate("metrics.show('context')");await sleep(80);
    await check(width+'x'+height+' statistics details remain within the visible viewport',"(()=>{const r=metrics.panel.getBoundingClientRect();return r.top>=0&&r.left>=0&&r.right<=innerWidth+1&&r.bottom<=innerHeight+1})()");
    await evaluate("metrics.show(null)");if(width===390||width===320)await shot('composer-'+width+'x'+height);
  }
  if(hasLayout){
    await screen(390,800);await evaluate("setDraft('焦点测试');navLayout.toggle()");await sleep(80);
    await check('mobile navigation owns focus and makes only its background inert',"document.querySelector('#chat-sidebar').contains(document.activeElement)&&workspace.inert&&!document.querySelector('#chat-sidebar').inert");
    await evaluate("document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{key:'f',ctrlKey:true,bubbles:true,cancelable:true}))");
    await check('conversation shortcuts cannot open controls behind an active navigation modal',"reading.find.hidden");
    await evaluate("document.querySelector('#settings-button').focus()");await key('Tab','Tab',9);
    await check('Tab wraps within the open mobile navigation',"document.querySelector('#chat-sidebar').contains(document.activeElement)");
    await evaluate("pane.setOpen(true,true)");await sleep(80);
    await check('opening a right modal from navigation closes the drawer and transfers focus',"!root.classList.contains('sidebar-open')&&!pane.pane.inert&&pane.pane.contains(document.activeElement)&&workspace.inert");
    await evaluate("pane.setOpen(false,true)");await sleep(80);
    await check('closing the right modal does not leave the conversation inert or focus hidden',"!workspace.inert&&!pane.pane.contains(document.activeElement)&&!document.activeElement.closest('[hidden],[inert]')");
    await screen(1440,900);await evaluate("pane.setOpen(true);pane.setFullscreen(true)");await screen(390,800);await screen(1440,900);await evaluate("pane.setFullscreen(false);pane.setOpen(false)");
    await check('viewport and fullscreen transitions do not restore stale inert snapshots',"!workspace.inert&&!document.querySelector('#chat-sidebar').inert&&preview.isConnected");
    await evaluate("window.selectionStart=3;draft.value='中文输入法验收';draft.setSelectionRange(3,5);composerLayout.schedule()");await sleep(80);
    await check('sizing never replaces the input or changes the draft selection',"document.querySelector('#prompt')===draft&&draft.value==='中文输入法验收'&&draft.selectionStart===3&&draft.selectionEnd===5");
    await evaluate("const {browserTheme}=await import('/apps/web/theme.mjs');browserTheme.update({mode:'dark'});pane.setOpen(true)");await sleep(120);await shot('desktop-dark');
  }
  if(hasLayout){
    await evaluate("pane.setOpen(false);metrics.show('context');pane.setOpen(true);pane.setFullscreen(true)");await sleep(100);
    await check('opening a workspace modal dismisses statistics without leaving an orphan top-layer dialog',"metrics.panel.hidden&&!metrics.panel.matches(':popover-open')");
  }
  await check('no unhandled JavaScript exception','true');assert.equal(errors.length,0);
}catch(error){errors.push(String(error));console.error(error);process.exitCode=1;if(socket?.readyState===WebSocket.OPEN)await shot('failure').catch(()=>{});}
finally{await writeFile(join(output,'report.json'),JSON.stringify({captureOnly:capture,passed:!capture&&!errors.length&&checks.every(c=>c.passed),checks,measures,errors},null,2));if(socket?.readyState===WebSocket.OPEN){await send('Browser.close').catch(()=>{});socket.close();}for(const p of pending.values())clearTimeout(p.timer);browser?.kill();for(const s of[server,upstream])if(s){s.closeAllConnections();await new Promise(r=>s.close(r));}await rm(scratch,{recursive:true,force:true,maxRetries:5,retryDelay:100}).catch(()=>{});}
