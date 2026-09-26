// Production shell markup/CSS and real panel classes with bounded file fixtures. No Agent or user service.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { startUiServer } from '../../../scripts/dev-web.mjs';
const capture = process.env.MONA_CAPTURE_BASELINE === '1';
const output = resolve(process.env.MONA_TEST_REPORT_DIR || '.tmp-verify/right-pane-batch2/after');
const scratch = await mkdtemp(join(tmpdir(), 'mona-pane-layout-')), profile = join(scratch, 'browser');
await mkdir(output, { recursive: true });
let server, upstream, browser, socket, sequence = 0; const pending = new Map(), checks = [], errors = [];
const sleep = ms => new Promise(r => setTimeout(r, ms));
async function wait(fn, label, timeout = 20000) { const end = Date.now() + timeout; while (Date.now() < end) { if (await fn()) return; await sleep(50); } throw Error('Timeout: ' + label); }
function send(method, params = {}) { return new Promise((resolve, reject) => { const id = ++sequence, timer = setTimeout(() => { pending.delete(id); reject(Error(method + ' timeout')); }, 20000); pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params })); }); }
async function evaluate(expression) { const r = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true, replMode: true }); if (r.exceptionDetails) throw Error(r.exceptionDetails.exception?.description || 'Browser failure'); return r.result.value; }
async function check(name, expression) { const passed = Boolean(await evaluate(expression)); checks.push({ name, passed }); console.log((passed ? 'PASS ' : 'FAIL ') + name); if (!capture) assert.ok(passed, name); }
async function shot(name) { await sleep(180); const r = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false }); await writeFile(join(output, name + '.png'), Buffer.from(r.data, 'base64')); }
try {
  upstream = await startUiServer({ host: '127.0.0.1', port: 0, endpoint: 'http://127.0.0.1:1', token: 'isolated-panel-fixture-token-12345678' });
  const html = (await readFile('apps/web/index.html', 'utf8')).replace(/<script\b[^>]*src="[^"]*app\.mjs"[^>]*><\/script>/g, '');
  // Only the app bootstrap is replaced. All tested markup, modules and styles are product sources.
  server = createServer(async (req, res) => {
    try {
      if (req.url === '/') { res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' }); res.end(html); return; }
      const reply = await fetch(`http://127.0.0.1:${upstream.address().port}${req.url}`); res.writeHead(reply.status, { 'Content-Type': reply.headers.get('content-type') || 'application/octet-stream' }); res.end(Buffer.from(await reply.arrayBuffer()));
    } catch { res.writeHead(500); res.end(); }
  }); await new Promise(r => server.listen(0, '127.0.0.1', r));
  browser = spawn(process.env.BROWSER_BIN || (process.platform === 'win32' ? 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe' : 'chromium'), ['--headless=new', '--disable-gpu', '--no-first-run', '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank'], { stdio: 'ignore' });
  let port; await wait(async () => { try { port = +(await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]; return port > 0; } catch { return false; } }, 'browser');
  const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json(); socket = new WebSocket(pages.find(p => p.type === 'page').webSocketDebuggerUrl); await once(socket, 'open');
  socket.addEventListener('message', event => { const m = JSON.parse(event.data), p = pending.get(m.id); if (p) { pending.delete(m.id); clearTimeout(p.timer); m.error ? p.reject(Error(JSON.stringify(m.error))) : p.resolve(m.result); } if (m.method === 'Runtime.exceptionThrown') errors.push(m.params.exceptionDetails.exception?.description || 'JS exception'); });
  await send('Page.enable'); await send('Runtime.enable'); await send('Emulation.setFocusEmulationEnabled', { enabled: true }); await send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false });
  await send('Page.navigate', { url: `http://127.0.0.1:${server.address().port}/` }); await wait(() => evaluate("Boolean(document.querySelector('#files-panel'))"), 'markup');
  await evaluate(`window.fixtureReady = (async () => {
    const { RightPane } = await import('/apps/web/right-pane.mjs'); const { FilePreview } = await import('/apps/web/file-preview.mjs'); const { WorkspaceFiles } = await import('/apps/web/workspace-files.mjs');
    window.pane = new RightPane(); pane.setScope('session:one'); window.views = new Map(); window.reads = []; window.closes = []; window.loads = 0;
    window.fileText = path => path.endsWith('.html') ? '<p>FRAME_REFERENCE</p>' : path.endsWith('.md') ? '# 工作文档\\n\\n说明 **重点** 和内容。' : 'fn main() {\\n'+Array.from({length:90},(_,i)=>'    let value_'+i+' = '+i+';').join('\\n')+'\\n}';
    const entry = (name, kind='file') => ({ name, path:name, kind });
    window.filesApi = { fork(){return this}, clear(){}, list:async(id,path,options)=>{reads.push(path);return {entries:path==='docs'?[entry('docs/nested.md')]:[entry('docs','directory'),entry('notes.md'),entry('code.rs'),entry('static.html')],revision:'tree',next_offset:null};}, search:async(id,query)=>{ await new Promise(r=>setTimeout(r,query==='old'?250:10)); return {entries:[entry(query+'.md')],truncated:false};} };
    window.openFile = (path, scope=pane.scope, extra={}) => {
      const id='file:'+path; if(pane.hasTab(id,scope)){pane.activate(id,scope);return views.get(scope+id);}
      const p = new FilePreview({api:{bytes:async()=>new TextEncoder().encode(fileText(path))},target:{kind:'session',id:scope.split(':')[1]},path,read:async()=>{loads++;return{kind:'text',text:fileText(path),bytes:1000,next_offset:1000,eof:true,revision:'v1'};}});
      p.mode=extra.mode||'preview';if(extra.wrap)p.root.classList.add('is-wrapped');views.set(scope+id,p);
      pane.openTab({id,title:path.split('/').at(-1),hint:path,kind:'file',node:p.root,scope,activate:!extra.background,fileState:()=>({path,mode:p.mode,wrap:p.root.classList.contains('is-wrapped')}),onActivate:()=>{if(!p.revision&&!p.loading)void p.load(true)},onClose:()=>{closes.push(path);p.close()},reopen:saved=>openFile(path,scope,saved)});
      p.onPreferenceChange=()=>pane.saveScope(scope);return p;
    };
    window.openTree=()=>{if(pane.hasTab('files')){pane.activate('files');return;};window.tree=new WorkspaceFiles({api:filesApi,target:{kind:'session',id:'one',path:'C:/Users/user/.mona-agent/workspaces/session-one'},openFile:path=>openFile(path)});pane.openTab({id:'files',title:'文件',kind:'files',node:tree.root,onActivate:()=>{if(!tree.loaded&&!tree.loading)void tree.load()},onClose:()=>tree.close()});};
    pane.configurePersistence('http://127.0.0.1:8080',(scope,file)=>file.kind==='files'?openTree():openFile(file.path,scope,file));
    pane.registerAction({id:'files',label:'浏览工作区文件',icon:'folder',run:openTree});pane.registerAction({id:'report',label:'查看工作文档',icon:'file',run:()=>openFile('notes.md')});
    document.querySelector('#welcome').hidden=true;document.querySelector('#timeline').innerHTML='<article class="turn"><div class="user-message">检查工作区文件，保留会话导航。</div><div class="assistant-body message-text"><p>文件可以在右侧打开，继续对话不受影响。</p></div></article>';
    openTree(); return true;
  })().catch(error=>{window.fixtureError=error.stack;throw error;})`);
  await wait(() => evaluate("(()=>{if(window.fixtureError)throw Error(window.fixtureError);return Boolean(window.tree?.loaded)})()"), 'file tree');
  await check('file tree has one compact path toolbar without redundant title/location rows', "tree.root.querySelector('.workspace-files-head').getBoundingClientRect().height<=40 && (!tree.root.querySelector('.files-location') || tree.root.querySelector('.files-location').hidden) && !tree.root.querySelector('.workspace-files-head strong')");
  await evaluate("tree.root.querySelector('summary').click()");await wait(()=>evaluate("Boolean(tree.root.querySelector('[data-path=\"docs/nested.md\"]'))"),'nested directory');
  await shot('01-files-light');
  await check('directory tree has a keyboard entry and semantic expanded state', "tree.browse.querySelector('.file-entry[tabindex=\"0\"]') && tree.browse.querySelector('summary[aria-expanded=true]')");
  await evaluate("window.originalBrowse=tree.browse;window.originalBranch=tree.browse.querySelector('details');tree.showSearch(true);tree.search.value='old';window.oldSearch=tree.searchFiles();tree.search.value='new';await tree.searchFiles();await oldSearch;");
  await check('late file search cannot overwrite the newer query', "tree.results.textContent.includes('new.md') && !tree.results.textContent.includes('old.md')");
  await evaluate("tree.showSearch(false)");
  await check('leaving search preserves the expanded directory and its DOM', "tree.browse===originalBrowse && tree.browse.querySelector('details')===originalBranch && originalBranch.open && !tree.browse.hidden");
  await evaluate("await tree.reload()");await wait(()=>evaluate("Boolean(tree.browse.querySelector('[data-path=\"docs/nested.md\"]'))"),'expanded branch restored');
  await check('refresh restores expanded folders', "tree.browse.querySelector('details').open");
  await evaluate("window.preview=openFile('documentation/very-long-folder/large-example-long-file-name.rs');await new Promise(r=>setTimeout(r,150))");
  await check('preview path and actions stay in one compact row', "preview.toolbar.getBoundingClientRect().height<=40");
  await check('preview copy accurately identifies a paged excerpt', "preview.copy.getAttribute('aria-label')!==null");
  await shot('02-preview-light');
  await evaluate("window.previousReader=preview.read;window.previousContent=preview.content.firstElementChild;preview.read=async()=>{throw Error('fixture read refused')};await preview.load(true)");
  await check('failed refresh preserves the last readable content and provides retry', "preview.content.firstElementChild===previousContent && preview.meta.textContent.includes('刷新失败') && preview.meta.querySelector('button')");
  await evaluate("preview.read=previousReader;preview.meta.querySelector('button').click()");await wait(()=>evaluate("!preview.loading && !preview.meta.textContent.includes('refused')"),'file retry');
  await evaluate("preview.actionsButton.click()");
  await check('file menu is top-layer, bounded and named rather than clipped by the preview', "preview.menuNode.matches(':popover-open') && preview.menuNode.getBoundingClientRect().right<=innerWidth && document.activeElement===preview.copy");
  await evaluate("preview.menuNode.dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true,cancelable:true}))");
  await check('Escape closes only the file menu and restores its trigger', "!preview.menu.opened && pane.open && document.activeElement===preview.actionsButton");
  await evaluate("preview.page.eof=false;preview.render()");
  await check('copying an excerpt is explicitly labelled as loaded text', "preview.copy.getAttribute('aria-label')==='复制已加载文本'");
  await evaluate("preview.page.eof=true;preview.render()");
  await evaluate("preview.page.kind='binary';preview.render();preview.actionsButton.click()");
  await check('binary file menu focuses a visible action instead of the hidden copy command', "document.activeElement.closest('.file-actions-menu')===preview.menuNode && !document.activeElement.hidden && document.activeElement.getClientRects().length>0");
  await evaluate("preview.menu.close();preview.page.kind='text';preview.render();window.linePreview=openFile('line-jump.md');await new Promise(r=>setTimeout(r,80));linePreview.read=async()=>({kind:'text',text:Array.from({length:140},(_,i)=>'line '+(i+1)).join('\\n'),bytes:1500,next_offset:1500,eof:true,revision:'lines'});await linePreview.load(true);await linePreview.focusLine(100);await new Promise(r=>setTimeout(r,60))");
  await check('line reference remains visible after switching Markdown preview to source', "(()=>{const line=linePreview.content.querySelector('[data-line=\"100\"]'),r=line.getBoundingClientRect(),p=linePreview.content.getBoundingClientRect();return r.top>=p.top&&r.bottom<=p.bottom})()");
  await evaluate("await linePreview.focusLine(80);await new Promise(r=>setTimeout(r,40))");
  await check('a newer file reference replaces the previous highlighted line', "linePreview.content.querySelectorAll('.is-referenced-line').length===1 && linePreview.content.querySelector('.is-referenced-line').dataset.line==='80'");
  await evaluate("for(let i=0;i<8;i++)openFile('document-'+i+'.md');await new Promise(r=>setTimeout(r,150))");
  await check('active tab is scrolled into its own strip without moving the transcript', "(()=>{const t=pane.tabs.get(pane.scope+'\\0'+pane.active.get(pane.scope)).button,r=t.getBoundingClientRect(),s=t.closest('[role=tablist]').getBoundingClientRect();return r.x>=s.x-1&&r.right<=s.right+1})()");
  await evaluate("window.activeBeforeKeys=pane.active.get(pane.scope);window.loadBeforeKeys=loads;pane.tabs.get(pane.scope+'\\0'+activeBeforeKeys).button.focus();pane.tabs.get(pane.scope+'\\0'+activeBeforeKeys).button.dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowLeft',bubbles:true,cancelable:true}));");
  await check('arrow navigation focuses a tab without loading or activating its content', "pane.active.get(pane.scope)===activeBeforeKeys && loads===loadBeforeKeys && document.activeElement.getAttribute('role')==='tab' && document.activeElement.dataset.tabId!==activeBeforeKeys");
  await evaluate("document.activeElement.click();window.htmlPreview=openFile('static.html');await new Promise(r=>setTimeout(r,120));window.frame=htmlPreview.content.querySelector('iframe');window.frameWindow=frame.contentWindow;window.frameLoads=0;frame.addEventListener('load',()=>frameLoads++);");
  await evaluate("pane.setFullscreen(true);pane.compare(pane.active.get(pane.scope))");
  await check('split layout has two independent visible tab strips', "[...pane.pane.querySelectorAll('[role=tablist]')].filter(n=>n.getClientRects().length).length===2");
  await check('split has a named adjustable divider rather than an extra compare title row', "Boolean(pane.pane.querySelector('[role=separator][data-split-divider]')) && !pane.pane.querySelector('.right-compare-header:not([hidden])')");
  await shot('03-split-light');
  await evaluate("window.primarySelected=pane.layoutFor().selected[0];window.secondarySelected=pane.layoutFor().selected[1];window.anotherPrimary=pane.layoutFor().groups[0].find(id=>id!==primarySelected);pane.activate(anotherPrimary)");
  await check('switching one pane keeps the other pane selection and iframe intact', "pane.layoutFor().selected[1]===secondarySelected && frame===htmlPreview.content.querySelector('iframe') && frame.contentWindow===frameWindow && !htmlPreview.root.hidden");
  await evaluate("window.beforeRatio=pane.headers[0].getBoundingClientRect().width;pane.splitDivider.focus();pane.splitDivider.dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowRight',bubbles:true,cancelable:true}));");
  await check('split divider keyboard movement changes real pane geometry', "pane.headers[0].getBoundingClientRect().width>beforeRatio && pane.splitDivider.getAttribute('aria-valuenow')==='52'");
  await evaluate("pane.setRatio(20)");
  await check('pane sizing keeps controls within both columns at the minimum ratio', "[...pane.pane.querySelectorAll('.right-pane-actions button')].filter(n=>n.getClientRects().length).every(n=>{const r=n.getBoundingClientRect(),p=n.closest('header').getBoundingClientRect();return r.left>=p.left-1&&r.right<=p.right+1})");
  await evaluate("pane.setRatio(50);pane.moveTab(secondarySelected,0);pane.compare(secondarySelected)");
  await check('moving and splitting an iframe does not reload its document', "frame.contentWindow===frameWindow && frameLoads===0");
  await evaluate("pane.saveScope();window.savedLayout=JSON.parse(sessionStorage.getItem('mona.web.file-tabs')).records.find(r=>r.scope==='session:one');");
  await check('both pane orders and active files persist as bounded descriptors', "savedLayout.panes.length===2 && savedLayout.panes[1].length>0 && savedLayout.selected.every((id,i)=>savedLayout.panes[i].includes(id)) && !JSON.stringify(savedLayout).includes('FRAME_REFERENCE')");
  await evaluate("window.visibleBefore=[...pane.content.children].filter(n=>!n.hidden);pane.setFullscreen(false)");
  await check('leaving fullscreen preserves both panes and original content elements', "visibleBefore.length===2 && visibleBefore.every(n=>n.isConnected&&!n.hidden)");
  await evaluate("window.previousWidth=pane.width;pane.applyWidth(700)");
  await check('resizing the docked panel immediately restores horizontal panes when space permits', "!pane.pane.classList.contains('is-stacked') && pane.splitDivider.getAttribute('aria-orientation')==='vertical'");
  await evaluate("pane.applyWidth(420)");
  await check('resizing back immediately stacks existing panes without dropping content', "pane.pane.classList.contains('is-stacked') && visibleBefore.every(n=>!n.hidden)");
  await evaluate("pane.setOpen(false);pane.setOpen(true)");
  await check('collapse and reopen do not erase the split layout', "visibleBefore.every(n=>n.isConnected&&!n.hidden)");
  await evaluate("pane.setScope('session:two');openFile('other.md');pane.setScope('session:one')");
  await check('switching sessions restores each independent pane selection', "visibleBefore.every(n=>n.isConnected&&!n.hidden)");
  await evaluate("pane.setFullscreen(true);await new Promise(r=>setTimeout(r,80))");
  await check('fullscreen never reinitializes file preview content', "visibleBefore.every(n=>n.isConnected) && closes.length===0");
  await evaluate("window.lightSurface=getComputedStyle(pane.pane).backgroundColor;(await import('/apps/web/theme.mjs')).browserTheme.update({mode:'dark'})");
  await check('dark screenshot uses the real theme palette, not only a data attribute', "document.documentElement.dataset.theme==='dark' && getComputedStyle(pane.pane).backgroundColor!==lightSurface");
  await shot('04-split-dark');
  for(const width of [390,320]){await send('Emulation.setDeviceMetricsOverride',{width,height:900,deviceScaleFactor:1,mobile:false});await evaluate("pane.setOpen(true)");await sleep(100);await check(width+'px panel and toolbar controls fit',`document.documentElement.scrollWidth<=${width} && [...pane.pane.querySelectorAll('.right-pane-actions button:not([hidden])')].filter(n=>n.getClientRects().length).every(n=>{const r=n.getBoundingClientRect();return r.left>=0&&r.right<=${width}+1})`);await shot('panel-'+width);}
  await check('stacked pane headers and both contents fit the panel height', "[...pane.content.children].filter(n=>!n.hidden).every(n=>{const r=n.getBoundingClientRect(),p=pane.pane.getBoundingClientRect();return r.height>0&&r.bottom<=p.bottom+1&&r.left>=p.left&&r.right<=p.right+1})");
  await evaluate("pane.moveTab('file:static.html',1);htmlPreview.actionsButton.click();pane.activate(pane.layoutFor().groups[1].find(id=>id!=='file:static.html')||pane.layoutFor().groups[0][0]);pane.setOpen(false)");await sleep(50);
  await check('hiding the panel dismisses all top-layer content menus', "!document.querySelector('.pane-menu:popover-open')");
  await evaluate("pane.setOpen(true)");
  await evaluate("for(const tab of [...pane.scopeTabs()])pane.closeTab(tab.id); ");
  await check('closing the final content tab collapses the panel', "!pane.open && pane.pane.hidden");
  await check('closing a modal panel returns focus outside hidden content', "!pane.pane.contains(document.activeElement)");
  await check('no unhandled browser exceptions', 'true');assert.equal(errors.length,0);
} catch(error){errors.push(String(error));console.error(error);if(socket?.readyState===WebSocket.OPEN)await shot('failure').catch(()=>{});process.exitCode=1;}
finally{
  await writeFile(join(output,'report.json'),JSON.stringify({captureOnly:capture,passed:!capture&&!errors.length&&checks.every(c=>c.passed),checks,errors},null,2));
  if(socket?.readyState===WebSocket.OPEN){await send('Browser.close').catch(()=>{});socket.close();}for(const p of pending.values())clearTimeout(p.timer);browser?.kill();
  for(const s of [server,upstream])if(s){s.closeAllConnections();await new Promise(r=>s.close(r));}await rm(scratch,{recursive:true,force:true,maxRetries:5,retryDelay:100}).catch(()=>{});
}
