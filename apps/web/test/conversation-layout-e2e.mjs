// Same formal-page scenarios before/after a rendering change. Capture mode records failures, never passes them off as tests.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { startUiServer } from '../../../scripts/dev-web.mjs';
import { createFixtureApiServer, TEST_TOKEN } from './fixture-server.mjs';
const capture = process.env.MONA_CAPTURE_BASELINE === '1';
const output = resolve(process.env.MONA_TEST_REPORT_DIR || '.tmp-verify/conversation-batch1/after');
const scratch = await mkdtemp(join(tmpdir(), 'mona-conversation-layout-')), profile = join(scratch, 'browser');
await mkdir(output, { recursive: true });
let api, ui, browser, socket, origin = '', sequence = 0;
const pending = new Map(), checks = [], errors = [], measurements = {};
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
async function wait(fn, label, timeout = 20000) { const end = Date.now() + timeout; while (Date.now() < end) { if (await fn()) return; await sleep(50); } throw Error('Timeout: ' + label); }
function send(method, params = {}) { return new Promise((resolve, reject) => { const id = ++sequence, timer = setTimeout(() => { pending.delete(id); reject(Error(method + ' timed out')); }, 20000); pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params })); }); }
async function evaluate(expression) { const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true, replMode: true }); if (result.exceptionDetails) throw Error(result.exceptionDetails.exception?.description || 'Browser expression failed'); return result.result.value; }
async function check(name, expression) { const passed = Boolean(await evaluate(expression)); checks.push({ name, passed }); console.log((passed ? 'PASS ' : 'FAIL ') + name); if (!capture) assert.ok(passed, name); }
async function shot(name) { await sleep(140); const image = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false }); await writeFile(join(output, name + '.png'), Buffer.from(image.data, 'base64')); }
const finalText = '## 验证结果\n\n**配置检查完成。** `service_name` 保持原样，下面是两项确认结果。\n\n- 配置文件位于工作目录。\n- 服务未被重启，文件也没有被改写。\n\n### 后续操作\n\n> 检查输出后，再决定是否执行修改。\n\n| 项目 | 结果 |\n| --- | --- |\n| 配置 | 正常 |\n| 网络 | 等待确认 |\n\n$$\nx_i^2 + y_i^2 = r^2\n$$\n\n```rust\nfn main() {\n    println!("ready");\n}\n```';
try {
  api = createFixtureApiServer({ allowOrigin: value => value === origin, management: true }); await new Promise(r => api.listen(0, '127.0.0.1', r));
  ui = await startUiServer({ host: '127.0.0.1', port: 0, endpoint: `http://127.0.0.1:${api.address().port}`, token: TEST_TOKEN }); origin = `http://127.0.0.1:${ui.address().port}`;
  browser = spawn(process.env.BROWSER_BIN || (process.platform === 'win32' ? 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe' : 'chromium'), ['--headless=new', '--disable-gpu', '--no-first-run', '--remote-debugging-port=0', `--user-data-dir=${profile}`, 'about:blank'], { stdio: 'ignore' });
  let port; await wait(async () => { try { port = Number((await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]); return port > 0; } catch { return false; } }, 'browser');
  const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json(); socket = new WebSocket(pages.find(p => p.type === 'page').webSocketDebuggerUrl); await once(socket, 'open');
  socket.addEventListener('message', event => { const m = JSON.parse(event.data), p = pending.get(m.id); if (p) { pending.delete(m.id); clearTimeout(p.timer); m.error ? p.reject(Error(JSON.stringify(m.error))) : p.resolve(m.result); } if (m.method === 'Runtime.exceptionThrown') errors.push(m.params.exceptionDetails.exception?.description || 'JS exception'); });
  await send('Page.enable'); await send('Runtime.enable'); await send('Emulation.setFocusEmulationEnabled', { enabled: true }); await send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false });
  await send('Page.navigate', { url: origin }); await wait(() => evaluate("document.querySelector('#model-label')?.textContent==='fixture-chat'"), 'formal app');
  await evaluate(`(async () => {
    const { TurnView } = await import('/apps/web/run-view.mjs'); window.renderer = await import('/apps/web/content-renderer.mjs');
    document.querySelector('#welcome').hidden = true; document.querySelector('#timeline').replaceChildren();
    window.view = new TurnView('请检查项目配置，先说明检查过程，不要重启服务。'); document.querySelector('#timeline').append(view.turn);
    window.snapshot = { run_id: 'layout-test', step: 3, pruned_items: 0, outcome: null, items: [
      { id: 'intro', step: 1, state: 'completed', content: { kind: 'agent_message', text: '先核对工作目录和配置文件，再执行只读检查。\\n\\n**检查范围**：项目配置与服务状态。', truncated: false } },
      ...Array.from({ length: 18 }, (_, i) => ({ id: 'read-' + i, step: i + 1, state: 'completed', content: { kind: 'tool_call', name: 'read', arguments: { path: 'config/service-' + i + '.toml' }, result: { content: '配置读取完成', status: 'success' } } })),
      { id: 'shell', step: 19, state: 'running', content: { kind: 'tool_call', name: 'shell', arguments: { command: 'git status --short' }, output: '正在读取状态…\\n', details: {} } },
    ] }; view.update(snapshot); await view.answerReady; document.querySelector('#main').scrollTop = 0;
  })()`);
  measurements.narration = await evaluate("({ font: getComputedStyle(view.turn.querySelector('.kind-message .message-text')).fontSize, answerFont: getComputedStyle(view.answer).fontSize, summary: view.summary.textContent, assistantGap: getComputedStyle(view.turn.querySelector('.assistant-body')).marginTop })");
  await check('intermediate narration keeps the same readable font as the answer', "getComputedStyle(view.turn.querySelector('.kind-message .message-text')).fontSize===getComputedStyle(view.answer).fontSize");
  await check('outer turn row contains elapsed state instead of duplicating the tool activity title', "!view.summary.textContent.includes('git status') && !view.summary.textContent.includes('运行命令')");
  await shot('01-running-collapsed-light');
  await evaluate("window.group = view.groups.values().next().value; group.summary.click()"); await sleep(120);
  measurements.group = await evaluate("({ height: group.body.clientHeight, scrollHeight: group.body.scrollHeight, overflow: getComputedStyle(group.body).overflowY })");
  await check('expanded process group has a bounded independently scrollable body', "group.body.clientHeight<=400 && group.body.scrollHeight>group.body.clientHeight && getComputedStyle(group.body).overflowY==='auto'");
  await check('opening live process positions it at the latest activity', "group.body.scrollHeight-group.body.clientHeight-group.body.scrollTop<3");
  await check('current tool owns the group icon and running state is not repeated', "group.iconKind==='shell' && group.state.hidden");
  await check('standard mode immediately restores the current command', "group.name.textContent.includes('git status')");
  await shot('02-running-open-light');
  await evaluate("group.body.scrollTop=0;group.body.dispatchEvent(new Event('scroll'));window.retainedGroup=group;window.retainedTool=view.entries.get('read-0').node;snapshot.items.push({id:'next-read',step:20,state:'running',content:{kind:'tool_call',name:'read',arguments:{path:'long-new-path.toml'}}});view.update(snapshot)"); await sleep(100);
  await check('incoming work preserves the reader position and original group/tool DOM', "group===retainedGroup && view.entries.get('read-0').node===retainedTool && group.body.scrollTop<3");
  await evaluate(`snapshot.items.forEach(item => item.state='completed');snapshot.outcome={status:'completed',output:${JSON.stringify(finalText)}};view.update(snapshot);await view.answerReady;document.querySelector('#main').scrollTop=0;`);
  await check('successful process folds once and leaves the final answer visible', "!view.process.open && !view.answer.hidden && view.answer.textContent.includes('验证结果')");
  await check('copy-answer excludes hidden intermediate process narration', `view.copyAnswerText === ${JSON.stringify(finalText)}`);
  measurements.markdown = await evaluate("({ firstTop: getComputedStyle(view.answer.firstElementChild).marginTop, tightMargins: [...view.answer.querySelectorAll('li > p')].map(p=>[getComputedStyle(p).marginTop,getComputedStyle(p).marginBottom]), mathBorder: getComputedStyle(view.answer.querySelector('.message-math-block')).borderTopWidth, actionWidth: view.answerActions.querySelector('button').getBoundingClientRect().width })");
  await check('first heading has no redundant top gap and tight list paragraphs have no extra margins', "getComputedStyle(view.answer.firstElementChild).marginTop==='0px' && [...view.answer.querySelectorAll('li > p')].every(p=>getComputedStyle(p).marginTop==='0px'&&getComputedStyle(p).marginBottom==='0px')");
  await check('display math belongs to the document instead of another bordered card', "getComputedStyle(view.answer.querySelector('.message-math-block')).borderTopWidth==='0px'");
  await check('short tables fill the bordered reading frame without cell gridlines', "(()=>{const frame=view.answer.querySelector('.message-table'),table=frame.querySelector('table');return Math.abs(table.getBoundingClientRect().width-frame.clientWidth)<2&&getComputedStyle(frame).borderLeftWidth==='1px'&&getComputedStyle(view.answer.querySelector('td')).borderLeftWidth==='0px'})()");
  await check('conversation text, headings, table, code and composer match the ZCode type scale', "(()=>{const style=node=>getComputedStyle(node),answer=view.answer,table=answer.querySelector('table'),code=answer.querySelector('.message-code code');return style(answer).fontSize==='14px'&&style(answer).lineHeight==='24.5px'&&style(answer.querySelector('h2')).fontSize==='16px'&&style(table).fontSize==='14px'&&style(table.querySelector('th')).fontWeight==='400'&&style(code).fontSize==='12px'&&!answer.querySelector('.message-code-lines')&&style(document.querySelector('#prompt')).lineHeight==='20px'&&style(view.turn.querySelector('.user-message')).paddingLeft==='16px'})()");
  await check('message copy is a named compact icon action with a fixed hit target', "view.answerActions.querySelector('button').getAttribute('aria-label')==='复制回答' && view.answerActions.querySelector('button svg') && view.answerActions.querySelector('button').getBoundingClientRect().width<=32");
  await evaluate("window.savedClipboard=Object.getOwnPropertyDescriptor(navigator,'clipboard');Object.defineProperty(navigator,'clipboard',{configurable:true,value:{writeText:async value=>{window.copiedAnswer=value;}}});window.copy=view.answerActions.querySelector('button');window.copyGlyph=copy.firstElementChild;copy.click();");
  await wait(() => evaluate("copy.dataset.copyState==='success'"), 'copy confirmation');
  await check('copy feedback preserves its icon and copies only the answer', `copy.firstElementChild===copyGlyph && copy.getBoundingClientRect().width===28 && copiedAnswer===${JSON.stringify(finalText)}`);
  await evaluate("copy.click()"); await sleep(1300);
  await check('repeated copy restores the original label and fixed geometry', "copy.getAttribute('aria-label')==='复制回答' && copy.firstElementChild===copyGlyph && copy.getBoundingClientRect().width===28");
  await evaluate("if(savedClipboard)Object.defineProperty(navigator,'clipboard',savedClipboard);else delete navigator.clipboard;");
  await shot('03-answer-light');
  await evaluate("view.summary.click();group.summary.click();group.summary.click();document.querySelector('#main').scrollTop=0"); await sleep(120);
  await check('manually reopening settled process starts from its beginning', "group.body.scrollTop<3");
  await shot('04-completed-process-light');
  await evaluate("snapshot.outcome={status:'failed',output:null,error:{message:'检查失败，未执行重试'}};view.update(snapshot)");
  await check('failed process remains inspectable and never looks like a completed answer', "view.process.open && view.answer.hidden && view.footer.textContent.includes('失败')");
  await evaluate(`snapshot.outcome={status:'completed',output:${JSON.stringify(finalText)}};view.update(snapshot);await view.answerReady;view.process.open=false;
    document.querySelector('#settings-button').click();document.querySelector('#settings-appearance-tab').click();document.querySelector('#theme-mode-dark').click();document.querySelector('#settings-back').click();document.querySelector('#main').scrollTop=0;`);
  await shot('05-answer-dark');
  await evaluate("window.stream=document.createElement('div');stream.className='message-text';document.querySelector('#timeline').append(stream);await renderer.renderMessage(stream,'稳定的 `service_name` 与 **配置** 正文',{streaming:true,contextKey:'inline-selection'});window.originalInline=stream.querySelector('code');let range=document.createRange();range.selectNodeContents(originalInline);getSelection().removeAllRanges();getSelection().addRange(range);await renderer.renderMessage(stream,'稳定的 `service_name` 与 **配置** 正文，继续输出',{streaming:true,contextKey:'inline-selection'});");
  await check('streamed paragraphs retain inline-code identity and selection', "stream.querySelector('code')===originalInline && getSelection().toString()==='service_name'");
  await evaluate("getSelection().removeAllRanges();await renderer.renderMessage(stream,'[打开文件](a.md) 后面的文字',{streaming:true,contextKey:'file-stream',openFile:path=>{window.clickedFile=path;}});window.stableLink=stream.querySelector('.message-file-link');await renderer.renderMessage(stream,'[打开文件](a.md) 后面的文字，追加',{streaming:true,contextKey:'file-stream',openFile:path=>{window.clickedFile=path;}});");
  await check('unchanged file actions retain their node across a streamed paragraph', "stream.querySelector('.message-file-link')===stableLink");
  await evaluate("await renderer.renderMessage(stream,'[打开文件](b.md) 后面的文字',{streaming:true,contextKey:'file-stream',openFile:path=>{window.clickedFile=path;}});stream.querySelector('.message-file-link').click();");
  await wait(() => evaluate("clickedFile==='b.md'"), 'changed file action');
  await check('changed links replace stale authorization callbacks rather than using the old path', "stream.querySelector('.message-file-link')!==stableLink && clickedFile==='b.md'");
  await evaluate("getSelection().removeAllRanges();renderer.disposeMessage(stream);stream.remove()");
  for (const width of [390, 320]) { await send('Emulation.setDeviceMetricsOverride', { width, height: 900, deviceScaleFactor: 1, mobile: false }); await sleep(100); await check(`${width}px body and controls remain inside the viewport`, `document.documentElement.scrollWidth<=${width}`); await shot('answer-dark-' + width); }
  await check('no unhandled browser exceptions', 'true'); assert.equal(errors.length, 0);
} catch (error) { errors.push(String(error)); if (socket?.readyState === WebSocket.OPEN) await shot('failure').catch(() => {}); console.error(error); process.exitCode = 1; }
finally {
  await writeFile(join(output, 'report.json'), JSON.stringify({ captureOnly: capture, passed: !capture && errors.length===0 && checks.every(c=>c.passed), checks, measurements, errors }, null, 2));
  if (socket?.readyState===WebSocket.OPEN) { await send('Browser.close').catch(()=>{}); socket.close(); }
  for (const p of pending.values()) clearTimeout(p.timer); pending.clear(); browser?.kill();
  ui?.closeAllConnections(); if (ui) await new Promise(r=>ui.close(r)); api?.closeAllConnections(); if (api) await new Promise(r=>api.close(r));
  await rm(scratch,{recursive:true,force:true,maxRetries:5,retryDelay:100}).catch(()=>{});
}
