import test from 'node:test';
import assert from 'node:assert/strict';
import { UiRegistry } from './registry.mjs';
import { UiHost } from './host.mjs';
import { ProductClient, validateManifest } from './client.mjs';

const module = mount => ({ version: 1, mount });
const tick = () => new Promise(resolve => setImmediate(resolve));
test('slots are finite, ordered, owned, removable and reject duplicate declarations', () => {
  const errors = [], registry = new UiRegistry(e => errors.push(e.message)), seen = [];
  const unsub = registry.subscribe('settings', entries => seen.push(entries.map(e => e.id).join(',')));
  const one = registry.register('memory','settings',{id:'memory',order:20,value:{}});
  registry.register('models','settings',{id:'models',order:10,value:{}});
  assert.equal(seen.at(-1),'models,memory');
  assert.throws(()=>registry.register('other','settings',{id:'memory',value:{}}),/重复/);
  assert.throws(()=>registry.register('other','arbitrary.dom',{id:'x',value:{}}),/无效/);
  registry.subscribe('settings',()=>{throw Error('view failed');});
  one();one();assert.equal(seen.at(-1),'models');assert.ok(errors.includes('view failed'));
  registry.removeOwner('models');assert.equal(registry.entries('settings').length,0);unsub();
});
test('module mount rollback releases subscriptions and only its own contributions', async()=>{
  const errors=[],registry=new UiRegistry(), events=[];
  const host=new UiHost({registry,shell:{},onError:e=>errors.push(e),catalog:{
    base:{local:true,load:async()=>module(ctx=>{ctx.register('status',{id:'base',value:{}});ctx.on('ping',()=>events.push('base'));})},
    broken:{load:async()=>module(ctx=>{ctx.register('status',{id:'broken',value:{}});ctx.provide('bad',{});ctx.on('ping',()=>events.push('bad'));ctx.own(()=>events.push('clean'));throw Error('broken');})},
  }});
  await host.init();await host.reconcile(['broken']);host.emit('ping');
  assert.deepEqual(registry.entries('status').map(e=>e.id),['base']);assert.equal(host.service('bad'),undefined);
  assert.deepEqual(events,['clean','base']);assert.ok(errors.length);host.dispose();assert.equal(registry.entries('status').length,0);
});
test('late module loading cannot mount into a replacement connection',async()=>{
  let resolve, mounts=0;const host=new UiHost({shell:{},registry:new UiRegistry(),catalog:{slow:{load:()=>new Promise(r=>resolve=r)}}});
  const loading=host.reconcile(['slow']);await tick();host.disconnect();resolve(module(()=>{mounts++;}));await loading;
  assert.equal(mounts,0);assert.equal(host.modules.size,0);
});
test('reconciliation shares modules and disposer lifetime survives reassembly',async()=>{
  let mounts=0, disposes=0;const registry=new UiRegistry();
  const host=new UiHost({registry,shell:{},catalog:{one:{load:async()=>module(ctx=>{mounts++;ctx.register('settings',{id:'one',value:{}});return()=>{disposes++;};})}}});
  await Promise.all([host.reconcile(['one']),host.reconcile(['one'])]);
  assert.equal(mounts,1);await host.reconcile(['one']);assert.equal(mounts,1);
  await host.reconcile([]);assert.equal(disposes,1);assert.equal(registry.entries('settings').length,0);
});
test('dependencies mount in order, missing or cyclic dependencies do not half-mount owners',async()=>{
  const mounted=[], errors=[];const host=new UiHost({registry:new UiRegistry(),shell:{},onError:e=>errors.push(e),catalog:{
    parent:{load:async()=>module(()=>{mounted.push('parent');})},child:{requires:['parent'],load:async()=>module(()=>{mounted.push('child');})},
    a:{requires:['b'],load:async()=>module(()=>{})},b:{requires:['a'],load:async()=>module(()=>{})},
  }});
  await host.reconcile(['child']);assert.deepEqual(mounted,[]);
  await host.reconcile(['child','parent']);assert.deepEqual(mounted,['parent','child']);
  await host.reconcile(['a','b']);assert.equal(host.modules.size,0);assert.ok(errors.length>=2);
});
test('late failed mount never unmounts the replacement owner',async()=>{
  let rejectFirst, generation=0;const registry=new UiRegistry();
  const host=new UiHost({registry,shell:{},catalog:{one:{load:async()=>module(ctx=>{
    generation++;ctx.register('status',{id:'one',value:{generation}});
    if(generation===1)return new Promise((_,reject)=>rejectFirst=reject);
  })}}});
  const first=host.reconcile(['one']);await tick();host.disconnect();await host.reconcile(['one']);
  rejectFirst(Error('late old failure'));await first;
  assert.equal(host.modules.size,1);assert.equal(registry.resolve('status','one').generation,2);host.dispose();
});
test('withdrawing a dependency releases its existing children and observers',async()=>{
  const registry=new UiRegistry();let childClean=0;
  const host=new UiHost({registry,shell:{},catalog:{parent:{load:async()=>module(()=>{})},child:{requires:['parent'],load:async()=>module(ctx=>{ctx.register('status',{id:'child',value:{}});return()=>{childClean++;};})}}});
  await host.reconcile(['parent','child']);await host.reconcile(['child']);
  assert.equal(host.modules.size,0);assert.equal(childClean,1);assert.equal(registry.entries('status').length,0);
});
test('concurrent reconciliation waits for the dependency mount to finish', async () => {
  let release, children = 0;
  const gate = new Promise(resolve => { release = resolve; });
  const errors = [], host = new UiHost({ registry: new UiRegistry(), shell: {}, onError: e => errors.push(e), catalog: {
    parent: { load: async () => module(async ctx => { await gate; ctx.provide('ready-parent', { ready: true }); }) },
    child: { requires: ['parent'], load: async () => module(ctx => {
      children++; assert.equal(ctx.service('ready-parent')?.ready, true);
    }) },
  } });
  const first = host.reconcile(['parent', 'child']); await tick();
  const second = host.reconcile(['parent', 'child']); await tick();
  const prematureChildren = children; release(); await Promise.all([first, second]);
  try { assert.equal(prematureChildren, 0); assert.equal(children, 1); assert.deepEqual(errors, []); }
  finally { host.dispose(); }
});
test('a manifest refresh connects newly mounted modules once and replays the selected session', async () => {
  const events = [], make = id => ({ load: async () => module(ctx => {
    ctx.on('connection', value => { if (value) events.push(`${id}:connection:${value.base}`); });
    ctx.on('session', value => events.push(`${id}:session:${value?.id}`));
  }) });
  const host = new UiHost({ registry: new UiRegistry(), shell: { session: () => ({ id: 'selected' }) }, catalog: { base: make('base'), added: make('added') } });
  let ids = ['base']; host.api.manifest = async () => validateManifest({ version: 1, modules: ids, capabilities: {} });
  try {
    await host.connect('http://127.0.0.1:1', 'test'); ids = ['base', 'added']; await host.refreshManifest(); await host.refreshManifest();
    assert.deepEqual(events, ['base:connection:http://127.0.0.1:1', 'base:session:selected', 'added:connection:http://127.0.0.1:1', 'added:session:selected']);
  } finally { host.dispose(); }
});
test('disposed module contexts cannot observe a replacement connection or acquire resources', async () => {
  const contexts = [], host = new UiHost({ registry: new UiRegistry(), shell: {}, catalog: {
    one: { load: async () => module(ctx => { contexts.push(ctx); ctx.provide('secret-service', {}); }) },
  } });
  host.api.manifest = async () => validateManifest({ version: 1, modules: ['one'], capabilities: {} });
  try {
    await host.connect('http://127.0.0.1:1', 'old'); const stale = contexts[0];
    await host.connect('http://127.0.0.1:2', 'new');
    assert.equal(stale.connection(), null); assert.equal(stale.service('secret-service'), undefined);
    assert.throws(() => stale.lockComposer(), { name: 'AbortError' }); assert.equal(host.locks.size, 0);
    assert.throws(() => stale.on('never', () => {}), { name: 'AbortError' }); assert.equal(host.listeners.has('never'), false);
  } finally { host.dispose(); }
});
test('failed cleanup and error reporting do not prevent other modules from being released', async () => {
  const registry = new UiRegistry(() => { throw Error('error renderer failed'); });
  registry.subscribe('status', () => { throw Error('observer failed'); });
  let released = 0;
  const host = new UiHost({ registry, onError: () => { throw Error('notice failed'); }, shell: { releaseOwner: () => { throw Error('pane failed'); } }, catalog: {
    one: { load: async () => module(ctx => { ctx.register('status', { id: 'one', value: {} }); ctx.own(() => { released++; throw Error('cleanup failed'); }); }) },
    two: { load: async () => module(ctx => { ctx.register('status', { id: 'two', value: {} }); ctx.own(() => { released++; }); }) },
  } });
  await host.reconcile(['one', 'two']); host.dispose();
  assert.equal(released, 2); assert.equal(host.modules.size, 0); assert.equal(registry.entries('status').length, 0);
});
test('unmount releases pending mount waiters and contains a late asynchronous cleanup failure', async () => {
  let complete, cleaned = 0;
  const gate = new Promise(resolve => { complete = resolve; }), errors = [];
  const host = new UiHost({ registry: new UiRegistry(), shell: {}, onError: e => errors.push(e), catalog: {
    slow: { load: async () => module(async () => { await gate; return async () => { cleaned++; throw Error('late cleanup'); }; }) },
  } });
  const loading = host.reconcile(['slow']); await tick(); host.disconnect();
  let settled = false; loading.then(() => { settled = true; }); await tick();
  const releasedBeforeCompletion = settled;
  complete(); await loading; await tick();
  assert.equal(releasedBeforeCompletion, true); assert.equal(cleaned, 1); assert.equal(host.modules.size, 0);
  assert.ok(errors.some(e => e.includes('late cleanup'))); host.dispose();
});
test('UI inventory distinguishes installed, desired and actually active states',()=>{
  const caps={planner:{compiled:true,enabled:true,active:false,configurable:true,restart_required:true}};
  const value=validateManifest({version:1,modules:['planner'],capabilities:caps});assert.equal(value.capabilities.planner.active,false);
  assert.throws(()=>validateManifest({...value,modules:['https://evil.invalid/code.js']}));
  assert.throws(()=>validateManifest({...value,modules:['planner','planner']}));
  assert.throws(()=>validateManifest({...value,version:999}));
  assert.throws(()=>validateManifest({...value,capabilities:{planner:{...caps.planner,active:true,compiled:false}}}));
});
test('product transport is bounded, header-authenticated and rejects stale replies',async()=>{
  const saved=globalThis.fetch;const client=new ProductClient();let resolve;
  try {
    client.configure('http://127.0.0.1:1','secret');
    globalThis.fetch=async(url,opts)=>{assert.equal(opts.headers.Authorization,'Bearer secret');assert.equal(opts.redirect,'error');assert.equal(opts.credentials,'omit');assert.ok(!url.includes('secret'));return new Promise(r=>resolve=r);};
    const request=client.request('/api/ui');await tick();client.clear();resolve(new Response('{}'));await assert.rejects(request,{name:'AbortError'});
    client.configure('http://127.0.0.1:2','new');globalThis.fetch=async()=>new Response('x'.repeat(2048));
    await assert.rejects(client.request('/api/ui',null,{maxBytes:32}),/超过/);
    await assert.rejects(client.request('//evil.invalid/api'));await assert.rejects(client.request('/api/a',{value:'x'.repeat(20000)}),/超过/);
  }finally{client.clear();globalThis.fetch=saved;}
});
