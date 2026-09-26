// Formal application markup and real view controllers; deterministic races, no paid model or user data.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { startUiServer } from '../../../scripts/dev-web.mjs';
const capture = process.env.MONA_CAPTURE_BASELINE === '1';
const output = resolve(process.env.MONA_TEST_REPORT_DIR || '.tmp-verify/conversation-batch4/after');
const scratch = await mkdtemp(join(tmpdir(), 'mona-lifecycle-')), profile = join(scratch, 'browser');
await mkdir(output, { recursive: true });
let upstream, server, browser, socket, seq = 0;
const pending = new Map(), checks = [], errors = [];
const sleep = ms => new Promise(r => setTimeout(r, ms));
async function wait(fn, label, timeout = 20000) { const end = Date.now() + timeout; while (Date.now() < end) { if (await fn()) return; await sleep(50); } throw Error('Timeout: ' + label); }
function send(method, params = {}) { return new Promise((resolve, reject) => { const id = ++seq, timer = setTimeout(() => { pending.delete(id); reject(Error('CDP timeout: ' + method)); }, 25000); pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params })); }); }
async function evaluate(expression) { const r = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true, replMode: true }); if (r.exceptionDetails) throw Error(r.exceptionDetails.exception?.description || 'Browser error'); return r.result.value; }
async function check(name, expression) { const passed = Boolean(await evaluate(expression)); checks.push({ name, passed }); console.log((passed ? 'PASS ' : 'FAIL ') + name); if (!passed) console.log('DIAGNOSTIC', await evaluate("({top:window.main?.scrollTop, readerTop:window.readerTop, intent:window.reading?.intentRevision, saved:window.oldAnchor?.intentRevision, following:window.reading?.following})")); if (!capture) assert.ok(passed, name); }
async function shot(name) { const image = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false }); await writeFile(join(output, name + '.png'), Buffer.from(image.data, 'base64')); }
try {
  upstream = await startUiServer({ host: '127.0.0.1', port: 0, endpoint: 'http://127.0.0.1:1', token: 'lifecycle-test-only-no-user-credentials' });
  const html = (await readFile('apps/web/index.html', 'utf8')).replace(/<script\b[^>]*src="[^"]*app\.mjs"[^>]*><\/script>/g, '');
  server = createServer(async (req, res) => { try { if (req.url === '/') { res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' }); res.end(html); return; } const r = await fetch(`http://127.0.0.1:${upstream.address().port}${req.url}`); res.writeHead(r.status, { 'Content-Type': r.headers.get('content-type') || 'application/octet-stream' }); res.end(Buffer.from(await r.arrayBuffer())); } catch { res.writeHead(500); res.end(); } });
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  browser = spawn(process.env.BROWSER_BIN || (process.platform === 'win32' ? 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe' : 'chromium'), ['--headless=new', '--disable-gpu', '--no-first-run', '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank'], { stdio: 'ignore' });
  let port; await wait(async () => { try { port = +(await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]; return port > 0; } catch { return false; } }, 'browser');
  const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json(); socket = new WebSocket(pages.find(p => p.type === 'page').webSocketDebuggerUrl); await once(socket, 'open');
  socket.addEventListener('message', event => { const m = JSON.parse(event.data), p = pending.get(m.id); if (p) { pending.delete(m.id); clearTimeout(p.timer); m.error ? p.reject(Error(JSON.stringify(m.error))) : p.resolve(m.result); } if (m.method === 'Runtime.exceptionThrown') errors.push(m.params.exceptionDetails.exception?.description || 'JS exception'); });
  await send('Page.enable'); await send('Runtime.enable'); await send('Emulation.setFocusEmulationEnabled', { enabled: true }); await send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false });
  await send('Page.navigate', { url: `http://127.0.0.1:${server.address().port}/` }); await wait(() => evaluate("Boolean(document.querySelector('#timeline'))"), 'markup');
  await evaluate(`window.fixtureReady = (async () => {
    window.renderer = await import('/apps/web/content-renderer.mjs');
    window.TurnView = (await import('/apps/web/run-view.mjs')).TurnView;
    window.FilePreview = (await import('/apps/web/file-preview.mjs')).FilePreview;
    const {RightPane} = await import('/apps/web/right-pane.mjs'); const {NavigationLayout,ComposerLayout} = await import('/apps/web/shell-layout.mjs');
    window.pane = new RightPane(); new NavigationLayout({pane}); new ComposerLayout(); pane.setScope('session:a');
    window.main = document.querySelector('#main'); window.timeline = document.querySelector('#timeline'); document.querySelector('#welcome').hidden=true;
    window.reading = new (await import('/apps/web/conversation-controls.mjs')).ConversationControls({main,timeline,loadOlder:async()=>{},hasOlder:()=>false,onQuote:()=>{},onSide:()=>{},canSide:()=>false});
    window.host = document.createElement('article');host.className='message-text';timeline.append(host);
    window.tick=()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));
    window.deferred=()=>{let resolve,reject;const promise=new Promise((a,b)=>{resolve=a;reject=b});return{promise,resolve,reject}};
    window.png={media_type:'image/png',base64:'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jJ/0AAAAASUVORK5CYII='};
    window.fixtureLoaded=true;
  })().catch(error=>{window.fixtureError=error.stack})`);
  await wait(()=>evaluate("(()=>{if(window.fixtureError)throw Error(window.fixtureError);return window.fixtureLoaded===true})()"),'view controllers');
  await evaluate("await renderer.renderMessage(host,'| 项目 | 结果 |\\n| --- | --- |\\n| A | 保留 |',{contextKey:'table-owner'});host.querySelector('.message-table-actions button[aria-label=\"放大表格\"]').click();");
  await check('table large preview displays actual content before disposal', "document.querySelector('dialog[open] td')?.textContent==='A'");
  await evaluate("renderer.disposeMessage(host);await tick()");
  await check('closing a source message also closes its expanded table instead of retaining private content', "!document.querySelector('.content-preview-dialog[open]')");
  await evaluate("document.querySelector('.content-preview-dialog[open]')?.close();host.replaceChildren();await renderer.renderMessage(host,'![local](chart.png)',{contextKey:'media-owner',readMedia:async()=>png});");
  await wait(() => evaluate("host.querySelector('img')?.naturalWidth>0"), 'local image');
  await evaluate("host.querySelector('.image-preview-open').click();host.hidden=true;await tick()");
  await check('hiding a document or switching its tab dismisses the owned enlarged image', "!document.querySelector('.content-preview-dialog[open]')");
  await evaluate("document.querySelector('.content-preview-dialog[open]')?.close();host.hidden=false;renderer.disposeMessage(host);host.replaceChildren();window.mediaAttempts=0;await renderer.renderMessage(host,'![retry](chart.png)',{contextKey:'retry',readMedia:async()=>{if(++mediaAttempts===1)throw Error('暂时读取失败');return png;}});await tick()");
  await check('failed local media offers an explicit in-place retry rather than requiring rerendering the message', "host.textContent.includes('暂时读取失败') && [...host.querySelectorAll('button')].some(b=>!b.hidden&&b.textContent.includes('重试'))");
  if (!capture) {
    await evaluate("[...host.querySelectorAll('button')].find(b=>!b.hidden&&b.textContent.includes('重试')).click()");
    await wait(() => evaluate("host.querySelector('img')?.naturalWidth>0"), 'image retry');
    await check('retry only rereads the same authorized image', "mediaAttempts===2");
  }
  await evaluate("renderer.disposeMessage(host);host.replaceChildren();window.releaseMedia=deferred();window.mediaSignal=null;await renderer.renderMessage(host,'![late](late.png)',{contextKey:'late-a',readMedia:(_,{signal})=>{mediaSignal=signal;return releaseMedia.promise}});await tick();await renderer.renderMessage(host,'会话 B 的新正文',{contextKey:'late-b'});releaseMedia.resolve(png);await tick()");
  await check('late media results cannot populate the replacement conversation', "mediaSignal.aborted && host.textContent==='会话 B 的新正文' && !host.querySelector('img')");
  await evaluate("window.openedFiles=[];await renderer.renderMessage(host,'[过期文件](old.md)',{contextKey:'link-a',openFile:path=>openedFiles.push(path)});host.querySelector('.message-file-link').click();renderer.disposeMessage(host);host.replaceChildren();await tick()");
  await check('a file action queued before its message is disposed cannot open another scope afterward', "openedFiles.length===0");
  // A deterministic worker stalls only the request under test. No production timing assumptions.
  await evaluate("window.NativeWorker=Worker;window.fakeWorkers=[];window.Worker=class {constructor(){fakeWorkers.push(this)}postMessage(){}terminate(){this.terminated=true}};window.parseSettled=false;window.parsePromise=renderer.renderMessage(host,'长段落'.repeat(12000),{contextKey:'disposed-worker'}).then(()=>{parseSettled=true});renderer.disposeMessage(host);host.replaceChildren();await tick()");
  await check('disposing a message releases its outstanding parser wait immediately', "parseSettled===true");
  await evaluate("window.Worker=NativeWorker;for(const w of fakeWorkers)w.onerror?.();await parsePromise;await renderer.renderMessage(host,'取消后仍可正常排版',{contextKey:'after-worker'})");
  await check('cancelling one render does not prevent a subsequent message from rendering', "host.textContent==='取消后仍可正常排版'");
  await evaluate("renderer.disposeMessage(host);host.replaceChildren();await renderer.renderMessage(host,'```mermaid\\nsequenceDiagram\\nAlice->>Bob: Hello\\n```',{contextKey:'diagram'});main.scrollTop=0");
  await wait(() => evaluate("host.querySelector('img.message-mermaid')?.naturalWidth>0"), 'real diagram', 35000);
  await evaluate("host.querySelector('img').click();renderer.disposeMessage(host);await tick()");
  await check('diagram magnification belongs to its source and closes with it', "!document.querySelector('.content-preview-dialog[open]')");
  await evaluate("document.querySelector('.content-preview-dialog[open]')?.close();host.remove();window.turns=[];for(let i=0;i<70;i++){const t=new TurnView('第 '+i+' 轮',{contextKey:'long-history'});t.turn.dataset.turnId='t-'+i;t.turn.dataset.historyBefore=String(i+1);timeline.append(t.turn);t.update({run_id:'r-'+i,items:[],outcome:{status:'completed',output:'## 阅读段落 '+i+'\\n\\n第一段正文包含 **强调** 与 `code_name`。\\n\\n另一段用于确认阅读位置与内容保持。'}});turns.push(t)}await Promise.all(turns.map(t=>t.answerReady));await tick();reading.activate('a',false);await tick();main.scrollTop=1600;main.dispatchEvent(new Event('scroll'));await tick();window.oldAnchor=reading.beforeChange();main.scrollTop=2800;main.dispatchEvent(new Event('scroll'));await tick();window.readerTop=main.scrollTop;reading.afterChange(oldAnchor);await tick()");
  await check('a delayed content completion does not override a newer reader scroll decision', "Math.abs(main.scrollTop-readerTop)<3");
  await evaluate("main.scrollTop=2100;main.dispatchEvent(new Event('scroll'));await tick();window.reference=reading.capture();window.referenceNode=timeline.querySelector('[data-turn-id=\"'+reference.id+'\"]');window.pixels=referenceNode.getBoundingClientRect().top;const anchor=reading.beforeChange();const t=new TurnView('新加载历史');t.turn.dataset.turnId='prepended';timeline.prepend(t.turn);t.update({run_id:'older',items:[],outcome:{status:'completed',output:'前置内容\\n\\n'.repeat(30)}});await t.answerReady;reading.afterChange(anchor);await tick();turns.push(t)");
  await check('prepending actual rich history preserves the visible message identity and anchor', "referenceNode.isConnected && Math.abs(referenceNode.getBoundingClientRect().top-pixels)<3");
  await evaluate("reading.showFind(true);reading.input.value='第一段';reading.input.dispatchEvent(new Event('input',{bubbles:true}));reading.showFind(false);await new Promise(r=>setTimeout(r,250))");
  await check('closing conversation find cancels delayed highlighting and navigation', "!CSS.highlights.has('mona-find') && !CSS.highlights.has('mona-find-active')");
  await evaluate(`window.navigation=new (await import('/apps/web/sessions-ui.mjs')).ConversationUI({busy:()=>false,changed:()=>{},beforeHistoryChange:()=>reading.beforeChange(),afterHistoryChange:a=>reading.afterChange(a),createTurn:(prompt,o)=>{const t=new TurnView(prompt,{contextKey:'history-api'});timeline.insertBefore(t.turn,o.before||null);turns.push(t);return t;}});navigation.mode='persistent';navigation.selected={id:'a',title:'历史会话',turn_count:75,revision:1,status:'completed'};navigation.nextBefore=20;window.headerRead=deferred();navigation.api.get=()=>headerRead.promise;navigation.api.turn=async(_id,id)=>({snapshot:{run_id:'run-'+id,items:[],outcome:{status:'completed',output:'延迟加载的历史正文\\n\\n'.repeat(20)}},total_items:0,supplemental_inputs:[],next_before:null});window.historyRead=navigation.loadOlder();main.scrollTop=2600;main.dispatchEvent(new Event('scroll'));await tick();window.readAt=reading.capture();window.readNode=timeline.querySelector('[data-turn-id="'+readAt.id+'"]');window.readTop=readNode.getBoundingClientRect().top;headerRead.resolve({session:navigation.selected,turns:[{id:'delayed-history',prompt:'更早历史',status:'completed',started_at:1,finished_at:2}],next_before:null});await historyRead;await tick();`);
  await check('delayed history API results preserve the reading position chosen while the request was pending', "readNode.isConnected && Math.abs(readNode.getBoundingClientRect().top-readTop)<3 && timeline.querySelector('[data-turn-id=delayed-history]')");
  await evaluate(`window.historyDom=turns[0];window.keptAnswer=historyDom.answer.firstChild;window.turnReads=0;navigation.api.turn=async()=>{if(++turnReads===1)throw Error('模拟历史网络中断');return{snapshot:{run_id:'r-retry',items:[],outcome:{status:'completed',output:'重新读取已确认的历史'}},total_items:0,supplemental_inputs:[],next_before:null}};await navigation.loadTurn('a',{id:'t-retry',started_at:1,finished_at:2},historyDom,navigation.generation);`);
  await check('history read failure keeps existing rich content and offers a local retry', "historyDom.answer.firstChild===keptAnswer && historyDom.turn.querySelector('.history-load-error button')");
  await evaluate("historyDom.turn.querySelector('.history-load-error button').click()");await wait(()=>evaluate("historyDom.answer.textContent==='重新读取已确认的历史'"),'history read retry');
  await check('retrying history rereads rather than starting any task and removes only its own error', "turnReads===2 && !historyDom.turn.querySelector('.history-load-error')");
  await evaluate("navigation.entries=[{id:'recent',title:'当前最近任务',status:'completed'}];navigation.api.configure('http://127.0.0.1:1','test-only');window.oldSearch=deferred();navigation.api.list=()=>oldSearch.promise;navigation.openSearch();window.searchRead=navigation.search('过期查询');navigation.closeSearch();navigation.openSearch();oldSearch.resolve({sessions:[{id:'stale',title:'过期搜索结果',status:'completed'}]});await searchRead;await tick()");
  await check('a closed task search cannot replace the results of a newly opened search dialog', "document.querySelector('#search-results').textContent.includes('当前最近任务') && !document.querySelector('#search-results').textContent.includes('过期搜索结果')");
  await evaluate("navigation.closeSearch();main.scrollTop=main.scrollHeight;reading.following=true;window.nested=document.createElement('pre');nested.className='activity-panel-output';nested.textContent='内部日志\\n'.repeat(150);timeline.append(nested);await tick();nested.scrollTop=100;reading.following=true;nested.dispatchEvent(new WheelEvent('wheel',{deltaY:-30,bubbles:true}));");
  await check('scrolling within a tool log does not turn off transcript follow intent', "reading.following===true");
  await evaluate("nested.remove();await tick()");
  await evaluate("window.previewRevision='old';window.failBytes=false;window.pdfBytes=new TextEncoder().encode('%PDF-1.1\\nfixture-only');window.file=new FilePreview({api:{bytes:async()=>{if(failBytes)throw Error('文档读取暂时失败');return pdfBytes}},target:{kind:'session',id:'a'},path:'report.pdf',read:async()=>({kind:'binary',revision:previewRevision,bytes:pdfBytes.length,next_offset:pdfBytes.length,eof:true})});pane.openTab({id:'pdf',title:'report.pdf',kind:'file',scope:'session:a',node:file.root,onClose:()=>file.close()});await file.load(true);window.oldFrame=file.content.querySelector('iframe');previewRevision='new';failBytes=true;await file.load(true)");
  await check('failed binary refresh keeps the previous mounted document and confirmed revision', "oldFrame.isConnected && file.content.contains(oldFrame) && file.revision==='old' && file.meta.textContent.includes('失败')");
  await evaluate("failBytes=false;await file.load(true)");
  await check('retry commits the new document once its bytes are available', "file.revision==='new' && file.content.querySelector('iframe') && !oldFrame.isConnected");
  await evaluate("window.lateBytes=deferred();file.api.bytes=()=>lateBytes.promise;window.refreshDone=false;window.refreshJob=file.load(true).then(()=>refreshDone=true);await tick();pane.closeTab('pdf');lateBytes.resolve(pdfBytes);await refreshJob;await tick()");
  await check('closing a loading binary preview cannot reattach content or retain loading chrome', "file.closed && !file.root.isConnected && !file.content.querySelector('iframe')");
  await evaluate("pane.setOpen(false);main.scrollTop=0;reading.following=false;reading.anchor=reading.capture();await tick()");
  await shot('long-history-light');
  await evaluate("(await import('/apps/web/theme.mjs')).browserTheme.update({mode:'dark'})");await shot('long-history-dark');
  for (const width of [390,320]) { await send('Emulation.setDeviceMetricsOverride',{width,height:900,deviceScaleFactor:1,mobile:false});await sleep(100);await check(width+'px long conversation remains bounded with responsive controls',`document.documentElement.scrollWidth<=${width}`); }
  assert.deepEqual(errors, []); await check('no unhandled browser exceptions', JSON.stringify(errors.length === 0));
  await writeFile(join(output,'report.json'),JSON.stringify({passed:!checks.some(c=>!c.passed),captureOnly:capture,checks,errors},null,2));
} catch(error) {
  console.error(error);console.error(errors);if(socket?.readyState===1)await shot('failure').catch(()=>{});
  await writeFile(join(output,'report.json'),JSON.stringify({passed:false,captureOnly:capture,checks,error:String(error),errors},null,2));process.exitCode=1;
} finally {
  if(socket?.readyState===1){await send('Browser.close').catch(()=>{});socket.close()}for(const p of pending.values())clearTimeout(p.timer);
  if(browser?.exitCode==null){browser.kill('SIGKILL');await sleep(400)}server?.closeAllConnections();upstream?.closeAllConnections();if(server)await new Promise(r=>server.close(r));if(upstream)await new Promise(r=>upstream.close(r));await rm(scratch,{recursive:true,force:true,maxRetries:5,retryDelay:100}).catch(()=>{});
}
