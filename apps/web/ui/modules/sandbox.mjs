import { ProductClient } from '../client.mjs';
import { element as el } from '../dom.mjs';
export const version = 1;
export const modes = Object.freeze({ 'read-only': '仅可查看', 'workspace-write': '工作区内修改', 'danger-full-access': '完全权限' });
const statuses = new Set(['idle', 'running', 'completed', 'failed', 'cancelled', 'timed_out', 'limited', 'interrupted']);
const sessionId = value => { if (typeof value !== 'string' || !/^[A-Za-z0-9_-]{1,64}$/.test(value)) throw Error('沙箱所属会话无效。'); return value; };
export function validateView(value, id = null) {
  if (value?.version !== 1 || value.session_id !== id || !Object.hasOwn(modes, value.mode)
      || typeof value.enabled !== 'boolean' || typeof value.locked !== 'boolean' || !statuses.has(value.status)
      || (id == null ? value.revision !== null : !Number.isSafeInteger(value.revision) || value.revision < 0)
      || (value.unavailable !== null && (typeof value.unavailable !== 'string' || value.unavailable.length > 4000))) throw Error('沙箱状态无效或版本不匹配。');
  if (value.backend !== null && (!['bubblewrap', 'landlock', 'seatbelt', 'windows-acl'].includes(value.backend?.backend)
      || !['full', 'partial'].includes(value.backend?.enforcement))) throw Error('沙箱后端状态无效。');
  if (value.backend && (!value.enabled || value.mode === 'danger-full-access' || value.unavailable)) throw Error('沙箱后端与实际模式不一致。');
  return value;
}
const SVG_NS = 'http://www.w3.org/2000/svg';
const svg = className => { const node=document.createElementNS(SVG_NS,'svg'); node.setAttribute('class',className); node.setAttribute('viewBox','0 0 16 16'); node.setAttribute('fill','none'); node.setAttribute('aria-hidden','true'); return node; };
const svgPath = (node, d, { fill='none', stroke='currentColor', join='' }={}) => { const path=document.createElementNS(SVG_NS,'path'); path.setAttribute('d',d); if(fill!=='none') path.setAttribute('fill',fill); if(stroke!=='none') { path.setAttribute('stroke',stroke); path.setAttribute('stroke-width','1.3'); } if(join) path.setAttribute('stroke-linejoin',join); node.append(path); };
function permissionGlyph(mode) {
  const node=svg('permission-glyph');
  if(mode==='read-only') {
    svgPath(node,'M5.08545 8.13775L7.18455 10.2368C7.26636 10.3187 7.4003 10.3142 7.47649 10.2271L11.5148 5.61194');
    svgPath(node,'M6.59624 2.14853C7.50155 1.80917 8.49914 1.80919 9.40444 2.14859L13.9245 3.84317V7.11961C13.9245 11.6089 10.5565 13.5975 8.00035 14.5779C5.44423 13.5975 2.07544 11.6089 2.07544 7.11961V3.84317L6.59624 2.14853Z',{join:'round'});
  } else if(mode==='workspace-write') {
    svgPath(node,'M6.4209 1.68067C7.43922 1.299 8.56177 1.29898 9.58008 1.68067L14.0996 3.375C14.2946 3.44811 14.4236 3.63455 14.4238 3.84278V6.89063C14.1115 6.71853 13.7761 6.58312 13.4238 6.48926V4.18946L9.22852 2.61621C8.43657 2.31947 7.56341 2.31939 6.77148 2.61621L2.5752 4.18946V7.11914C2.5752 11.1796 5.52369 13.056 8 14.0391C8.27653 13.9293 8.55827 13.8067 8.8418 13.6729C9.07101 13.9468 9.33228 14.1929 9.62012 14.4053C9.12409 14.6579 8.63578 14.8696 8.17871 15.0449C8.0637 15.0889 7.93628 15.0889 7.82129 15.0449C5.22011 14.0472 1.5752 11.9381 1.5752 7.11914V3.84278C1.57541 3.63469 1.70463 3.44821 1.89941 3.375L6.4209 1.68067Z',{fill:'currentColor',stroke:'none'});
    svgPath(node,'M5.26392 6.60339H10.7361'); svgPath(node,'M5.26392 9.86902H8.32833');
    svgPath(node,'M10.0317 13.2229C10.263 13.3929 10.4943 13.563 10.7256 13.733C10.7932 13.6482 10.8608 13.5634 10.9284 13.4786C12.1455 11.9522 13.3626 10.4258 14.5798 8.89935C14.6474 8.81455 14.715 8.72975 14.7826 8.64495C14.4143 8.37419 14.046 8.10344 13.6777 7.83268C13.6169 7.92252 13.5562 8.01236 13.4954 8.10219C12.4016 9.71926 11.3078 11.3363 10.214 12.9534C10.1532 13.0432 10.0924 13.1331 10.0317 13.2229Z',{fill:'currentColor',stroke:'none'});
    svgPath(node,'M12.6516 12.6696C12.6516 12.925 12.6516 13.1804 12.6516 13.4359C12.6952 13.4378 12.7387 13.4398 12.7823 13.4417C13.5663 13.4768 14.3504 13.5118 15.1345 13.5469C15.178 13.5488 15.2216 13.5508 15.2651 13.5527C15.2651 13.2194 15.2651 12.8861 15.2651 12.5527C15.2216 12.5547 15.178 12.5566 15.1345 12.5586C14.3504 12.5936 13.5663 12.6287 12.7823 12.6637C12.7387 12.6657 12.6952 12.6676 12.6516 12.6696Z',{fill:'currentColor',stroke:'none'});
  } else {
    svgPath(node,'M6.59624 2.14853C7.50155 1.80917 8.49914 1.80919 9.40444 2.14859L13.9245 3.84317V7.11961C13.9245 11.6089 10.5565 13.5975 8.00035 14.5779C5.44423 13.5975 2.07544 11.6089 2.07544 7.11961V3.84317L6.59624 2.14853Z',{join:'round'});
    svgPath(node,'M8 4.39209V9.89209'); svgPath(node,'M8 10.8081V11.8081');
  }
  return node;
}
function chevronGlyph() { const node=svg('permission-chevron'); svgPath(node,'M3 5.25L8 10.25L13 5.25'); return node; }
function checkGlyph() { const node=svg('permission-check'); svgPath(node,'M2.25 8.5L5.49732 11.7473C5.90519 12.1552 6.57263 12.1344 6.95426 11.7018L13.75 4'); return node; }

export function mount(ctx) {
  const api = new ProductClient();
  const control = el('span', 'permission-select');
  const trigger = el('button', 'permission-trigger'); trigger.id='sandbox-permission'; trigger.type='button'; trigger.setAttribute('aria-haspopup','menu'); trigger.setAttribute('aria-expanded','false'); trigger.setAttribute('aria-controls','sandbox-permission-menu');
  const triggerIcon=el('span','permission-trigger-icon'), triggerLabel=el('span','permission-trigger-label','读取沙箱…'); trigger.append(triggerIcon,triggerLabel,chevronGlyph());
  const menu=el('div','permission-menu'); menu.id='sandbox-permission-menu'; menu.setAttribute('role','menu'); menu.setAttribute('aria-label','文件权限'); menu.hidden=true;
  const rows=new Map();
  for(const [value,text] of Object.entries(modes)) {
    const row=el('button','permission-option'); row.type='button'; row.dataset.sandboxMode=value; row.setAttribute('role','menuitemradio');
    const icon=el('span','permission-option-icon'); icon.append(permissionGlyph(value)); const textNode=el('span','permission-option-label',text); const check=el('span','permission-option-check');
    row.append(icon,textNode,check); menu.append(row); rows.set(value,{row,check});
  }
  control.append(trigger,menu);
  const notice = el('p', 'sandbox-notice'); notice.id = 'sandbox-notice'; notice.setAttribute('role', 'status');
  ctx.register('composer.mode', { id: 'sandbox', order: 10, value: control });
  ctx.register('composer.dock', { id: 'sandbox', order: 20, value: notice });
  let current = null, view = null, generation = 0, pending = null, controller = null, busy = false, mutation = false, menuOpen=false;
  const setMenu = open => { menuOpen=Boolean(open && !trigger.disabled); trigger.setAttribute('aria-expanded',String(menuOpen)); menu.hidden=!menuOpen; if(menuOpen) queueMicrotask(()=>rows.get(view?.mode)?.row.focus()); };
  const active = () => ctx.capability('sandbox')?.active === true;
  ctx.register('composer.commands', { id: 'permission', order: 20, value: {
    menu: { section: 'command', label: '权限', description: '切换沙箱文件权限', icon: () => permissionGlyph('danger-full-access'),
      visible: active, disabled: () => trigger.disabled,
    },
    run: raw => { if (String(raw ?? '').trim()) throw Error('请使用 /permission 打开权限选择器。'); setMenu(true); },
  } });
  const path = id => id == null ? '/api/sandbox' : `/api/sessions/${sessionId(id)}/sandbox`;
  function render() {
    control.hidden = !active() && !(view && view.session_id && view.mode !== 'danger-full-access');
    const mode=view?.mode || 'workspace-write'; triggerIcon.replaceChildren(permissionGlyph(mode)); triggerLabel.textContent=view ? modes[mode] : '读取沙箱…';
    trigger.disabled = mutation || busy || !view || !view.enabled || view.locked || view.status === 'running';
    if(trigger.disabled) setMenu(false);
    for(const [value,{row,check}] of rows) { const selected=view?.mode===value; row.setAttribute('aria-checked',String(selected)); row.disabled=trigger.disabled; check.replaceChildren(...(selected?[checkGlyph()]:[])); }
    const detail=`${view?.locked ? '部署已锁定模式。' : ''}${view?.unavailable || (view && !view.enabled ? '当前宿主未启用沙箱；要求沙箱的历史会话不能直接继续。' : '限制 Agent Shell 与直接文件修改；不限制文件读取和联网。用户手动终端不在此范围内。')}${view?.backend?.enforcement === 'partial' ? ' 当前系统只能部分执行文件限制。' : ''}`;
    trigger.dataset.tooltip=detail; trigger.setAttribute('aria-label',`文件权限：${view ? modes[mode] : '读取中'}${view?.backend?.enforcement==='partial'?'，部分限制':''}`);
    notice.hidden = !notice.textContent;
    ctx.refreshCommands();
  }
  async function read(id = current) {
    if (!ctx.connection() || ctx.signal.aborted) return;
    if (pending?.id === id && pending.generation === generation) return pending.promise;
    controller?.abort(); controller = new AbortController();
    const job = { id, generation }; pending = job;
    job.promise = (async () => {
      try {
        const value = validateView(await api.request(path(id), null, { signal: controller.signal, maxBytes: 16 * 1024 }), id);
        if (ctx.signal.aborted || job.generation !== generation || current !== id || pending !== job) return;
        if (view && id != null && value.revision < view.revision) return;
        view = value; notice.textContent = ''; render();
      } catch (error) {
        if (error.name !== 'AbortError' && !ctx.signal.aborted && job.generation === generation && pending === job) { notice.textContent = error.message; render(); }
      } finally { if (pending === job) pending = null; }
    })();
    return job.promise;
  }
  async function change(mode) {
    if (mutation || busy || !view?.enabled || view.locked || !active() || !Object.hasOwn(modes, mode)) return;
    const initialId = ctx.shell.session()?.id || null, expected = view;
    mutation = true; render(); let unlock = () => {}, epoch = generation;
    try {
      const admission = ctx.shell.ensureSession(); unlock = ctx.lockComposer();
      const session = await admission;
      if (!session || ctx.signal.aborted || (initialId && session.id !== initialId) || ctx.shell.session()?.id !== session.id) return;
      if (current !== session.id) { generation++; current = session.id; view = null; }
      epoch = generation;
      const old = expected.session_id === current ? expected : validateView(await api.request(path(current)), current);
      if (epoch !== generation || ctx.signal.aborted || ctx.shell.session()?.id !== current) return;
      controller?.abort(); pending = null;
      const id = current;
      const result = validateView(await api.request(path(id), { revision: old.revision, mode }, { maxBytes: 16 * 1024 }), id);
      if (epoch !== generation || ctx.signal.aborted || current !== id || ctx.shell.session()?.id !== id) return;
      view = result; await ctx.shell.refreshSessionHeader(id);
      if (epoch !== generation || ctx.signal.aborted) return;
      notice.textContent = result.unavailable ? '模式已保存，但本机后端不可用；命令会拒绝执行，不会取消限制。' : '';
      ctx.shell.focusPrompt();
    } catch (error) {
      if (epoch === generation && !ctx.signal.aborted) {
        await read();
        if (epoch === generation && !ctx.signal.aborted) notice.textContent = error.status === 409 ? '会话已更新，请查看当前沙箱模式后重试。' : error.message;
      }
    } finally { mutation = false; unlock(); if (!ctx.signal.aborted) render(); }
  }
  ctx.listen(trigger,'click',()=>setMenu(!menuOpen));
  for(const [mode,{row}] of rows) ctx.listen(row,'click',ctx.guard(async()=>{setMenu(false);if(view?.mode!==mode)await change(mode);else ctx.shell.focusPrompt();}));
  ctx.listen(document,'pointerdown',event=>{if(menuOpen&&!control.contains(event.target))setMenu(false);});
  ctx.listen(document,'keydown',event=>{if(event.key==='Escape'&&menuOpen){event.preventDefault();setMenu(false);trigger.focus();}});
  ctx.on('connection', async connection => {
    generation++; controller?.abort(); pending = null; view = null; notice.textContent = ''; current = ctx.shell.session()?.id || null;
    if (connection) { api.configure(connection.base, connection.bearer); await read(); } else api.clear(); render();
  });
  ctx.on('session', session => {
    const id = session?.id || null;
    if (id !== current) { generation++; controller?.abort(); pending = null; current = id; view = null; notice.textContent = ''; }
    render(); if (!mutation && (!view || view.revision !== (session?.revision ?? null))) void read();
  });
  ctx.on('busy', value => { busy = value; render(); });
  ctx.on('assembly', () => { render(); void read(); });
  ctx.on('run', state => { if (state.outcome && !mutation) void read(); });
  ctx.own(() => { generation++; controller?.abort(); api.clear(); pending = null; view = null; notice.textContent = ''; setMenu(false); });
  render();
}
