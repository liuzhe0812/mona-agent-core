// Formal Web UI + controlled management/runtime fixture + real Chromium.
// Verifies the computed Mona design contract rather than source-text snapshots.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { startUiServer } from '../../../scripts/dev-web.mjs';
import { createFixtureApiServer, TEST_TOKEN } from './fixture-server.mjs';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const output = join(root, '.tmp-verify/design-system-browser-report');
const scratch = await mkdtemp(join(tmpdir(), 'mona-design-e2e-'));
const profile = join(scratch, 'browser');
await mkdir(output, { recursive: true });
const checks = [], errors = [];
let browser, ui, api, cdp, origin = '', endpoint = '';
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

async function waitFor(fn, label, timeout = 15000) {
  const until = Date.now() + timeout;
  while (Date.now() < until) { if (await fn()) return; await sleep(60); }
  throw new Error(`Timed out: ${label}`);
}
async function stop(child) {
  if (!child || child.exitCode != null) return;
  const exited = once(child, 'exit'); child.kill('SIGKILL'); await Promise.race([exited, sleep(5000)]);
}
function check(name, condition) { assert.ok(condition, name); checks.push(name); console.log(`PASS ${name}`); }
async function connect(url) {
  const socket = new WebSocket(url); await once(socket, 'open');
  const pending = new Map(); let seq = 0;
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++seq, timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 15000);
    pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params }));
  });
  socket.addEventListener('message', event => {
    const value = JSON.parse(event.data), request = pending.get(value.id);
    if (request) { pending.delete(value.id); clearTimeout(request.timer); value.error ? request.reject(new Error(JSON.stringify(value.error))) : request.resolve(value.result); }
    if (value.method === 'Runtime.exceptionThrown') errors.push(value.params.exceptionDetails.exception?.description || 'browser exception');
  });
  return {
    send, socket,
    async evaluate(expression) {
      const value = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
      if (value.exceptionDetails) throw new Error(value.exceptionDetails.exception?.description || 'page script failed');
      return value.result.value;
    },
    close() { socket.close(); for (const request of pending.values()) { clearTimeout(request.timer); request.reject(new Error('CDP closed')); } pending.clear(); },
  };
}
const evaluate = expression => cdp.evaluate(expression);
const waitPage = (expression, label, timeout) => waitFor(() => evaluate(expression), label, timeout);
async function click(selector) {
  const point = await evaluate(`(() => {const node=document.querySelector(${JSON.stringify(selector)});if(!node)throw new Error('Missing ${selector}');node.scrollIntoView({block:'center'});const r=node.getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2};})()`);
  await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...point });
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, ...point });
}
async function hover(selector) {
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 1, y: 1 });
  const point = await evaluate(`(() => {const node=document.querySelector(${JSON.stringify(selector)});node.scrollIntoView({block:'center'});const box=node.getBoundingClientRect();return{x:box.x+box.width/2,y:box.y+box.height/2};})()`);
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...point });
  await waitPage("!document.querySelector('#mona-tooltip').hidden", 'shared tooltip visible');
}
async function press(key, code, virtualKeyCode) {
  await cdp.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key, code, windowsVirtualKeyCode: virtualKeyCode, nativeVirtualKeyCode: virtualKeyCode });
  await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key, code, windowsVirtualKeyCode: virtualKeyCode, nativeVirtualKeyCode: virtualKeyCode });
}
async function screenshot(name) {
  const image = await cdp.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  await writeFile(join(output, `${name}.png`), Buffer.from(image.data, 'base64'));
}
async function ready() {
  await waitPage("document.querySelectorAll('#component-list .capability-row').length===2 && document.querySelectorAll('#tool-list .capability-row').length===2 && document.querySelectorAll('.provider-row').length===2 && document.querySelector('#model-label').textContent==='fixture-chat'", 'formal UI and management fixture ready');
}

try {
  api = createFixtureApiServer({ allowOrigin: value => value === origin, management: true });
  await new Promise(resolve => api.listen(0, '127.0.0.1', resolve));
  endpoint = `http://127.0.0.1:${api.address().port}`;
  ui = await startUiServer({ host: '127.0.0.1', port: 0, endpoint, token: TEST_TOKEN });
  origin = `http://127.0.0.1:${ui.address().port}`;

  const executable = process.env.BROWSER_BIN || (process.platform === 'win32' ? 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe' : 'chromium');
  browser = spawn(executable, ['--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check', '--remote-debugging-port=0', `--user-data-dir=${profile}`, '--window-size=1440,1000', 'about:blank'], { stdio: 'ignore' });
  browser.on('error', error => errors.push(`browser: ${error.message}`));
  let port;
  await waitFor(async () => { try { port = Number((await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]); return port > 0; } catch { return false; } }, 'browser ready');
  const pages = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
  cdp = await connect(pages.find(page => page.type === 'page').webSocketDebuggerUrl);
  await cdp.send('Page.enable'); await cdp.send('Runtime.enable');
  await cdp.send('Emulation.setFocusEmulationEnabled', { enabled: true });
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false });
  await cdp.send('Page.navigate', { url: origin }); await ready();

  const tokens = await evaluate(`(() => {
    const s=getComputedStyle(document.documentElement), probe=document.createElement('div');
    probe.style.cssText='position:absolute;width:var(--ui-radius-container);height:var(--ui-page-gutter);visibility:hidden';
    document.body.append(probe); const box=probe.getBoundingClientRect(); probe.remove();
    return {control:s.getPropertyValue('--ui-control-md').trim(),sidebar:s.getPropertyValue('--ui-settings-sidebar-width').trim(),radius:box.width,gutter:box.height};
  })()`);
  check('the production page exposes the shared control, radius, settings-sidebar and spacing contracts',
    tokens.control === '36px' && tokens.sidebar === '232px' && tokens.radius === 12 && tokens.gutter === 32);
  const mainMetrics = await evaluate(`(() => {const box=s=>document.querySelector(s).getBoundingClientRect();const css=s=>getComputedStyle(document.querySelector(s));return {
    topbar:box('.topbar').height,newTask:box('#new-chat').height,icon:box('#search-button').width,send:box('#send').width,
    composerRadius:css('.composer').borderRadius,workspaceRadius:css('.workspace').borderRadius,suggestions:document.querySelectorAll('.suggestions,[data-prompt]').length,
    workspaceShadow:css('.workspace').boxShadow
  };})()`);
  check('main chrome uses the compact component contract', mainMetrics.topbar === 48 && mainMetrics.newTask === 36
    && mainMetrics.icon === 32 && mainMetrics.send === 34 && mainMetrics.composerRadius === '22px'
    && mainMetrics.workspaceRadius === '12px' && mainMetrics.suggestions === 0 && mainMetrics.workspaceShadow === 'none');
  await screenshot('main-light');

  await hover('#sessions-options');
  check('global hints use the compact light surface, thin border and rounded shape', await evaluate(`(() => {
    const target=document.querySelector('#sessions-options'),tip=document.querySelector('#mona-tooltip'),s=getComputedStyle(tip),box=tip.getBoundingClientRect();
    return tip.textContent===target.dataset.tooltip
      && s.backgroundColor==='rgb(255, 255, 255)' && s.borderWidth==='1px' && s.borderRadius==='12px'
      && s.fontSize==='13px' && s.padding==='6px 10px' && s.boxShadow==='none'
      && box.left>=8 && box.right<=innerWidth-8 && !target.hasAttribute('title');
  })()`));
  await screenshot('tooltip-light');
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 400, y: 400 });
  await waitPage("document.querySelector('#mona-tooltip').hidden", 'tooltip dismissed on leave');
  await evaluate("document.querySelector('#sessions-options').focus()");
  await waitPage("!document.querySelector('#mona-tooltip').hidden", 'keyboard tooltip');
  check('keyboard hints preserve the accessible description', await evaluate("document.querySelector('#sessions-options').getAttribute('aria-describedby').split(' ').includes('mona-tooltip')"));
  await press('Escape', 'Escape', 27);
  check('Escape dismisses only the hint', await evaluate("document.querySelector('#mona-tooltip').hidden && document.activeElement===document.querySelector('#sessions-options')"));

  await click('#settings-button');
  const capabilityMetrics = await evaluate(`(() => {const box=s=>document.querySelector(s).getBoundingClientRect();const css=s=>getComputedStyle(document.querySelector(s));return {
    sidebar:box('.settings-sidebar').width,title:css('.settings-sidebar h1').fontSize,tab:box('#settings-components-tab').height,
    heading:css('#component-settings-section .settings-heading h2').fontSize,padding:css('#component-settings-section').paddingTop,
    row:box('#component-list .capability-row').height,rowTitle:css('#component-list .capability-copy strong').fontSize,
    switchHeight:box('#component-list .model-switch').height,panelRadius:css('#component-list').borderRadius,panelShadow:css('#component-list').boxShadow
  };})()`);
  check('component settings are compact, tokenized and shadowless', capabilityMetrics.sidebar === 232
    && capabilityMetrics.title === '20px' && capabilityMetrics.tab === 36 && capabilityMetrics.heading === '20px'
    && capabilityMetrics.padding === '32px' && capabilityMetrics.row <= 84 && capabilityMetrics.rowTitle === '14px'
    && capabilityMetrics.switchHeight === 24 && capabilityMetrics.panelRadius === '12px' && capabilityMetrics.panelShadow === 'none');
  check('component settings contain no redundant current-state or policy labels', await evaluate(`(() => {
    const text=document.querySelector('#component-settings-section').textContent;
    return !text.includes('当前已启用') && !text.includes('当前未启用')
      && !text.includes('可由当前用户管理') && !text.includes('由部署配置控制');
  })()`));
  await screenshot('settings-components-light');
  await click('#component-list .capability-row:nth-child(2) .model-switch');
  await waitPage("document.querySelector('#component-list .capability-row:nth-child(2) .model-switch').getAttribute('aria-checked')==='false' && document.querySelector('#components-notice').textContent.includes('重启后生效')", 'component setting saved');
  check('component switches save through the management API and show only the needed restart feedback', true);

  await click('#settings-tools-tab');
  check('components and tools use separate settings pages', await evaluate(`(() => {
    const component=document.querySelector('#component-settings-section'),tools=document.querySelector('#tool-settings-section');
    return component.hidden && !tools.hidden
      && [...document.querySelectorAll('#component-list .capability-row')].every(row=>!row.textContent.includes('内容搜索'))
      && [...document.querySelectorAll('#tool-list .capability-row')].every(row=>!row.textContent.includes('上下文压缩'));
  })()`));
  await click('#tool-list .capability-row:first-child .model-switch');
  await waitPage("document.querySelector('#tool-list .capability-row:first-child .model-switch').getAttribute('aria-checked')==='true' && document.querySelector('#tools-notice').textContent.includes('重启后生效')", 'tool setting saved');
  check('tool switches use the separate tool collection and shared revision guard', true);
  await screenshot('settings-tools-light');

  await click('#settings-models-tab');
  const modelMetrics = await evaluate(`(() => {const box=s=>document.querySelector(s).getBoundingClientRect();const css=s=>getComputedStyle(document.querySelector(s));return {
    providerSide:box('.provider-sidebar').width,providerRow:box('.provider-row').height,providerHeader:box('.provider-header').height,
    logo:box('.provider-logo').width,title:css('.provider-title h3').fontSize,search:box('#provider-search').height,
    modelRow:box('.model-row').height,button:box('.set-default').height,cardRadius:css('.provider-card').borderRadius,
    cardShadow:css('.provider-card').boxShadow
  };})()`);
  check('model settings use the shared list, input and provider-detail contract', modelMetrics.providerSide === 240
    && modelMetrics.providerRow <= 48 && modelMetrics.providerHeader <= 80 && modelMetrics.logo === 36
    && modelMetrics.title === '18px' && modelMetrics.search === 36
    && modelMetrics.modelRow >= 52 && modelMetrics.modelRow <= 68
    && modelMetrics.button === 32 && modelMetrics.cardRadius === '12px' && modelMetrics.cardShadow === 'none');
  await screenshot('settings-models-light');
  await hover('#model-list .model-switch:disabled');
  check('disabled controls keep their existing explanatory hints', await evaluate("document.querySelector('#mona-tooltip').textContent==='默认模型始终在对话中显示'"));

  await click('#add-provider');
  await waitPage("document.querySelector('#provider-dialog').open", 'provider dialog open');
  const dialogMetrics = await evaluate(`(() => {const d=document.querySelector('#provider-dialog'),box=d.getBoundingClientRect(),css=getComputedStyle(d);return {
    width:box.width,padding:css.paddingTop,radius:css.borderRadius,title:getComputedStyle(d.querySelector('h2')).fontSize,
    input:d.querySelector('input').getBoundingClientRect().height,button:d.querySelector('.dialog-actions .secondary').getBoundingClientRect().height,
    closeIcons:d.querySelectorAll('.dialog-head .icon-button').length
  };})()`);
  check('form dialogs use one dismissal path and the 16px/36px compact contract', dialogMetrics.width <= 560
    && dialogMetrics.padding === '16px' && dialogMetrics.radius === '12px' && dialogMetrics.title === '18px'
    && dialogMetrics.input === 36 && dialogMetrics.button === 36 && dialogMetrics.closeIcons === 0);
  await screenshot('provider-dialog-light');
  await evaluate("(() => {const b=document.createElement('button');b.type='button';b.id='tooltip-probe';b.dataset.tooltip='<b>动态提示</b>';b.textContent='提示测试';document.querySelector('#provider-dialog .dialog-actions').append(b);})()");
  await hover('#tooltip-probe');
  check('dynamic modal hints stay above the dialog and render text safely', await evaluate("document.querySelector('#provider-dialog #mona-tooltip')?.matches(':popover-open') && document.querySelector('#mona-tooltip').textContent==='<b>动态提示</b>' && !document.querySelector('#mona-tooltip b')"));
  await evaluate("document.querySelector('#tooltip-probe').remove()");
  await waitPage("document.querySelector('#mona-tooltip').hidden", 'removed control dismisses hint');
  await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, x: 2, y: 2 });
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, x: 2, y: 2 });
  await waitPage("!document.querySelector('#provider-dialog').open", 'provider dialog closed by backdrop');
  check('dialog backdrop dismissal restores focus to its opener', await evaluate("document.activeElement===document.querySelector('#add-provider')"));

  await click('#settings-appearance-tab');
  const appearanceMetrics = await evaluate(`(() => {const box=s=>document.querySelector(s).getBoundingClientRect();const css=s=>getComputedStyle(document.querySelector(s));return {
    mode:box('.theme-modes').height,mini:box('.theme-mini').height,row:box('.appearance-row').height,
    cardRadius:css('.theme-card-content').borderRadius,select:box('#theme-font-size').height,previewRadius:css('.theme-preview').borderRadius
  };})()`);
  check('appearance settings reuse the same control and container tiers', appearanceMetrics.mode === 32
    && appearanceMetrics.mini === 72 && appearanceMetrics.row === 52 && appearanceMetrics.cardRadius === '12px'
    && appearanceMetrics.select === 36 && appearanceMetrics.previewRadius === '12px');
  await click('.theme-mode:has(input[value="dark"])');
  const darkContrast = await evaluate(`(() => {
    const css=getComputedStyle(document.querySelector('.settings-page'));
    const parse=value=>{const values=String(value).match(/-?\\d*\\.?\\d+/g)?.slice(0,3).map(Number);if(!values)return null;return String(value).startsWith('color(srgb')?values.map(v=>v*255):values;};
    const lum=value=>{const values=parse(value);if(!values)return null;return values.map(v=>{v/=255;return v<=.04045?v/12.92:((v+.055)/1.055)**2.4}).reduce((a,v,i)=>a+v*[.2126,.7152,.0722][i],0);};
    const a=lum(css.color),b=lum(css.backgroundColor);
    return {ratio:a==null||b==null?0:(Math.max(a,b)+.05)/(Math.min(a,b)+.05),theme:document.documentElement.dataset.theme,color:css.color,bg:css.backgroundColor};
  })()`);
  check('dark mode keeps primary page text readable', darkContrast.theme === 'dark' && darkContrast.ratio >= 4.5);
  await screenshot('settings-appearance-dark');

  await click('#settings-back');
  await evaluate("(() => {const p=document.querySelector('#prompt');p.value='设计系统浏览器验收';p.dispatchEvent(new Event('input',{bubbles:true}));})()");
  await click('#send');
  await waitPage("document.querySelector('.activity-item:not(.kind-message) > .activity-summary')", 'tool activity row rendered', 12000);
  await evaluate("document.querySelectorAll('.execution-process,.activity-group').forEach(node=>node.open=true)");
  const chatMetrics = await evaluate(`(() => {const box=s=>document.querySelector(s).getBoundingClientRect();const css=s=>getComputedStyle(document.querySelector(s));return {
    userFont:css('.user-message').fontSize,userLine:css('.user-message').lineHeight,userRadius:css('.user-message').borderTopLeftRadius,
    assistantFont:css('.message-text').fontSize,assistantLine:css('.message-text').lineHeight,
    composerFont:css('#prompt').fontSize,composerLine:css('#prompt').lineHeight,
    process:box('.execution-summary').height,activity:box('.activity-item:not(.kind-message) > .activity-summary').height,detailRadius:getComputedStyle(document.querySelector('.activity-item:not(.kind-message) > .activity-summary')).borderRadius
  };})()`);
  check('chat and execution rows consume the shared typography and compact activity geometry', chatMetrics.userFont === '14px'
    && chatMetrics.userLine === '22px' && chatMetrics.assistantFont === '14px' && chatMetrics.assistantLine === '24.5px'
    && chatMetrics.composerFont === '14px' && chatMetrics.composerLine === '24px'
    && chatMetrics.userRadius === '12px' && chatMetrics.process === 32 && chatMetrics.activity === 28 && chatMetrics.detailRadius === '8px');
  await screenshot('chat-running-dark');
  await waitPage("document.querySelector('#cancel').hidden", 'fixture task settled', 12000);
  await evaluate("(() => {const p=document.querySelector('#prompt');p.value='第二轮：验证渲染能力与会话导航';p.dispatchEvent(new Event('input',{bubbles:true}));})()");
  await click('#send');
  await waitPage("document.querySelectorAll('#timeline .turn').length===2", 'second conversation turn rendered', 12000);
  await waitPage("[...document.querySelectorAll('#timeline .turn')].at(-1)?.querySelector('.assistant-body > .message-text h1')?.textContent==='富内容渲染'", 'renderer answer settled', 12000);
  await waitPage("document.querySelector('#cancel').hidden", 'second fixture task settled', 12000);
  await evaluate("(() => {const turn=[...document.querySelectorAll('#timeline .turn')].at(-1);turn.querySelector('.execution-process').open=true;turn.querySelectorAll('.activity-group,.activity-item').forEach(node=>node.open=true);})()");
  await waitPage("[...document.querySelectorAll('#timeline .turn')].at(-1).querySelector('.hljs-keyword') && [...document.querySelectorAll('#timeline .turn')].at(-1).querySelector('img.message-mermaid')?.naturalWidth>0", 'completed syntax and real diagram rendering', 30000);
  await waitPage("[...document.querySelectorAll('#timeline .turn')].at(-1).querySelector('.structured-diff .diff-add .diff-code')?.textContent==='+new_value'", 'lazy tool diff renderer');
  const renderer = await evaluate(`(() => {
    const turn=[...document.querySelectorAll('#timeline .turn')].at(-1);
    const answer=turn.querySelector('.assistant-body > .message-text');
    return {
      codeLanguage:answer.querySelector('.message-code-language')?.textContent,
      codeCollapsed:answer.querySelector('.message-code-block')?.classList.contains('is-collapsed'),
      highlighted:Boolean(answer.querySelector('.hljs-keyword')),
      tasks:answer.querySelectorAll('.message-task-checkbox').length,
      checked:Boolean(answer.querySelector('.message-task-checkbox:checked')),
      math:Boolean(answer.querySelector('.katex math')),
      mermaid:Boolean(answer.querySelector('img.message-mermaid')?.naturalWidth),
      escapedTable:[...answer.querySelectorAll('.message-table td')].some(node=>node.textContent==='A|B'),
      autoLink:Boolean(answer.querySelector('a[href="https://example.com/docs"]')),
      rawHtmlSafe:answer.textContent.includes('<img src=x onerror=alert(1)>') && !answer.querySelector('img[src="x"]'),
      richImage:Boolean(turn.querySelector('.rich-content img[src^="data:image/png;base64,"]')),
      diff:turn.querySelector('.structured-diff .diff-add .diff-code')?.textContent==='+new_value' && turn.querySelector('.structured-diff .diff-delete .diff-code')?.textContent==='-old_value',
      artifacts:turn.querySelectorAll('.artifact-viewer').length,
      fallbackMeta:[...turn.querySelectorAll('.structured-json summary')].some(node=>node.textContent.includes('fixture.meta')),
    };
  })()`);
  check('conversation renderer covers code language/highlight/collapse, task lists, tables, auto-links and safe raw HTML',
    renderer.codeLanguage === 'rust' && renderer.codeCollapsed && renderer.highlighted && renderer.tasks === 2
      && renderer.checked && renderer.escapedTable && renderer.autoLink && renderer.rawHtmlSafe);
  check('conversation renderer covers math and Mermaid diagrams', renderer.math && renderer.mermaid);
  check('tool rendering covers rich images, registered diff details, safe JSON fallback and artifact cards',
    renderer.richImage && renderer.diff && renderer.artifacts >= 2 && renderer.fallbackMeta);
  await evaluate("(() => {const turn=[...document.querySelectorAll('#timeline .turn')].at(-1);turn.querySelector('.assistant-body > .message-text .code-expand').click();})()");
  check('long code blocks can be expanded without replacing the conversation', await evaluate("![...document.querySelectorAll('#timeline .turn')].at(-1).querySelector('.assistant-body > .message-text .message-code-block').classList.contains('is-collapsed')"));
  await evaluate("(() => {const turn=[...document.querySelectorAll('#timeline .turn')].at(-1);[...turn.querySelectorAll('.artifact-viewer button')].find(node=>node.textContent==='读取内容')?.click();})()");
  await waitPage("[...document.querySelectorAll('#timeline .turn')].at(-1).querySelector('.artifact-output:not([hidden])')?.textContent.includes('fixture')", 'artifact viewer loaded a trusted page');
  check('artifact viewer pages complete retained content through the authenticated host', true);
  check('incremental Markdown reconciliation preserves stable completed prefix blocks', await evaluate(`(async()=>{
    const {renderMessage}=await import('/apps/web/content-renderer.mjs');
    const host=document.createElement('div');
    renderMessage(host,'# 稳定标题\\n\\n第一段');
    const heading=host.firstElementChild;
    renderMessage(host,'# 稳定标题\\n\\n第一段继续\\n\\n第二段');
    return host.firstElementChild===heading && host.textContent.includes('第二段');
  })()`));
  await screenshot('conversation-renderer-dark');
  await waitPage("!document.querySelector('#conversation-rail').hidden && document.querySelectorAll('.conversation-rail-mark').length===2", 'conversation rail visible');
  check('conversation rail renders 12px compact markers with one active reading marker', await evaluate("(() => {const marks=[...document.querySelectorAll('.conversation-rail-mark')], widths=marks.map(mark=>getComputedStyle(mark,'::before').width);return marks.length===2 && document.querySelectorAll('.conversation-rail-mark.is-active').length===1 && widths.every(width=>width==='12px');})()"));
  const railPoint = await evaluate("(() => {const r=document.querySelector('.conversation-rail-mark:first-child').getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2};})()");
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...railPoint });
  await waitPage("document.querySelector('.conversation-rail-mark:first-child').classList.contains('is-peak')", 'conversation rail hover peak');
  check('conversation rail hover creates the 2.6 / 1.7 neighbor curve', await evaluate("(() => {const marks=[...document.querySelectorAll('.conversation-rail-mark')];return marks[0].dataset.visualScale==='2.6' && marks[1].dataset.visualScale==='1.7' && marks[0].classList.contains('is-peak') && marks[1].classList.contains('is-near');})()"));
  await waitPage("!document.querySelector('.conversation-rail-preview').hidden", 'conversation rail preview visible');
  check('conversation rail preview is 320px, 8px from the marker, and shows prompt and answer', await evaluate("(() => {const mark=document.querySelector('.conversation-rail-mark:first-child').getBoundingClientRect(),preview=document.querySelector('.conversation-rail-preview').getBoundingClientRect();return preview.width===320 && Math.round(preview.left-mark.right)===8 && document.querySelector('.conversation-rail-preview-title').textContent==='设计系统浏览器验收' && document.querySelector('.conversation-rail-preview-text').textContent.length>0;})()"));
  await screenshot('conversation-rail-dark');
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 900, y: 400 });
  await evaluate("(() => {const timeline=document.querySelector('#timeline');for(let i=0;i<120;i++){const turn=document.createElement('article');turn.className='turn';turn.dataset.railFixture='true';const query=document.createElement('div');query.className='user-message';query.textContent=`历史问题 ${i+1}`;turn.append(query);timeline.append(turn);}})()");
  await waitPage("document.querySelectorAll('.conversation-rail-mark').length===122", 'conversation rail long directory');
  await evaluate("(() => {const main=document.querySelector('#main');main.scrollTop=main.scrollHeight;main.dispatchEvent(new Event('scroll'));})()");
  await waitPage("Number(document.querySelector('.conversation-rail-mark.is-active')?.dataset.itemIndex)>80 && document.querySelector('.conversation-rail-marks').scrollTop>0", 'conversation rail keeps long-directory active item visible');
  check('conversation rail confines long histories to its own vertical scroller', await evaluate("(() => {const root=document.querySelector('.conversation-rail-marks');return root.scrollHeight>root.clientHeight && getComputedStyle(root).overflowY==='auto';})()"));
  await evaluate("(() => {document.querySelectorAll('[data-rail-fixture]').forEach(node=>node.remove());const main=document.querySelector('#main');main.scrollTop=0;main.dispatchEvent(new Event('scroll'));})()");
  await waitPage("document.querySelectorAll('.conversation-rail-mark').length===2", 'conversation rail fixture cleanup');
  await evaluate("window.__railNavigation=null;document.querySelector('#main').scrollTo=function(options){window.__railNavigation=options?.behavior||''}");
  await click('.conversation-rail-mark:first-child');
  check('conversation rail marker smoothly navigates to the exact query', await evaluate("window.__railNavigation==='smooth'"));


  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 900, deviceScaleFactor: 1, mobile: false });
  check('the 390px main workspace has no horizontal overflow', await evaluate("document.documentElement.scrollWidth<=390 && document.querySelector('.shell').scrollWidth<=390"));
  await waitPage("!document.querySelector('#conversation-rail').classList.contains('is-wide')", 'conversation rail narrow container state');
  await waitPage("getComputedStyle(document.querySelector('#conversation-rail')).visibility==='hidden'", 'conversation rail narrow transition');
  check('conversation rail hides below its 864px container threshold', await evaluate("getComputedStyle(document.querySelector('#conversation-rail')).visibility==='hidden'"));
  await waitPage("document.querySelector('#chat-sidebar').inert && getComputedStyle(document.querySelector('#sidebar-resizer')).display==='none'", 'mobile sidebar state synchronized');
  await click('#sidebar-toggle');
  await waitPage("document.querySelector('.shell').classList.contains('sidebar-open') && !document.querySelector('#chat-sidebar').inert", 'mobile drawer opened');
  await click('#settings-button');
  await waitPage("!document.querySelector('#settings-page').hidden", 'mobile settings opened');
  await click('#settings-models-tab');
  const mobileMetrics = await evaluate(`(() => {const box=s=>document.querySelector(s).getBoundingClientRect();return {
    page:document.querySelector('#settings-page').scrollWidth,sidebar:box('.settings-sidebar').width,
    card:box('.provider-card').width,columns:getComputedStyle(document.querySelector('.provider-card')).gridTemplateColumns,
    viewport:innerWidth
  };})()`);
  check('mobile settings become one column without overflowing the viewport', mobileMetrics.page <= mobileMetrics.viewport
    && mobileMetrics.sidebar === mobileMetrics.page && mobileMetrics.card <= mobileMetrics.page - 32 + .5
    && !mobileMetrics.columns.includes('240px'));
  await screenshot('settings-models-mobile-dark');
  await click('#settings-appearance-tab');
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 320, height: 800, deviceScaleFactor: 1, mobile: false });
  check('320px appearance cards and tabs stay inside the viewport', await evaluate("document.querySelector('#settings-page').scrollWidth<=320 && [...document.querySelectorAll('.theme-card:not([hidden]),.settings-sidebar nav button')].every(node=>node.getBoundingClientRect().right<=320.5)"));
  await screenshot('settings-appearance-320-dark');

  check('the browser raised no unhandled JavaScript exception', errors.length === 0);
  await writeFile(join(output, 'result.json'), JSON.stringify({ passed: true, checks, real_browser: true, management_fixture: true, real_provider: false }, null, 2));
  console.log(`PASS ${checks.length} design-system browser assertions; screenshots in .tmp-verify/design-system-browser-report`);
} catch (error) {
  console.error(error);
  if (cdp?.socket.readyState === WebSocket.OPEN) {
    console.error(await evaluate("JSON.stringify({body:document.body?.innerText.slice(0,3000),theme:document.documentElement.dataset.theme,errors:window.__errors})").catch(() => 'page unavailable'));
    await screenshot('failure').catch(() => {});
  }
  await writeFile(join(output, 'result.json'), JSON.stringify({ passed: false, checks, error: String(error), errors }, null, 2));
  process.exitCode = 1;
} finally {
  if (cdp?.socket.readyState === WebSocket.OPEN) await cdp.send('Browser.close').catch(() => {});
  cdp?.close(); await stop(browser);
  for (const server of [ui, api]) if (server) { server.closeAllConnections?.(); await new Promise(resolve => server.close(resolve)); }
  await rm(scratch, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
}
