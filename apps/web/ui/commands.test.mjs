import test from 'node:test';
import assert from 'node:assert/strict';
import { commandGroups, runCommand } from './commands.mjs';
import { UiRegistry } from './registry.mjs';
import { UiHost } from './host.mjs';
const value = (label, section = 'command', extra = {}) => ({ run() {}, menu: { section, label, ...extra } });

test('command groups contain only registered visible capabilities and no empty headings', () => {
  const registry = new UiRegistry();
  registry.register('models', 'composer.commands', { id: 'model', order: 30, value: value('模型') });
  registry.register('sandbox', 'composer.commands', { id: 'permission', order: 20, value: value('权限') });
  registry.register('planner', 'composer.commands', { id: 'plan', order: 10, value: value('计划', 'add') });
  registry.register('side', 'composer.commands', { id: 'side', value: () => {} });
  registry.register('absent', 'composer.commands', { id: 'file', value: value('文件', 'add', { visible: () => false }) });
  const groups = commandGroups(registry.entries('composer.commands'));
  assert.deepEqual(groups.map(g => [g.label, g.rows.map(r => r.entry.id)]), [['添加', ['plan']], ['指令', ['permission', 'model']]]);
  registry.removeOwner('planner');
  assert.deepEqual(commandGroups(registry.entries('composer.commands')).map(g => g.id), ['command']);
});
test('malformed command metadata is bounded and cannot hide valid neighbors', () => {
  const errors = [];
  const entries = [
    { id: 'bad', value: value('坏', 'unknown') },
    { id: 'huge', value: value('大', 'add', { description: 'x'.repeat(241) }) },
    { id: 'throws', value: value('错误', 'add', { visible() { throw Error('controlled'); } }) },
    { id: 'model', value: value('模型') },
  ];
  assert.equal(commandGroups(entries, e => errors.push(e)).flatMap(g => g.rows).length, 1);
  assert.equal(errors.length, 3);
});
test('slash and menu activation share execution guards but can have different actions', async () => {
  const calls = [], v = { run: arg => calls.push(arg), menu: { select: () => calls.push('toggle') } };
  await runCommand(v, 'on'); await runCommand(v, '', true);
  assert.deepEqual(calls, ['on', 'toggle']);
  let disabled = false; v.menu.disabled = () => disabled; disabled = true;
  await assert.rejects(runCommand(v, '', true), /不可用/); assert.equal(calls.length, 2);
  v.menu.disabled = false; v.menu.visible = false;
  await assert.rejects(runCommand(v), /不可用/); assert.equal(calls.length, 2);
});
test('a synchronous or asynchronous command error remains observable', async () => {
  await assert.rejects(runCommand({ run() { throw Error('sync'); } }), /sync/);
  await assert.rejects(runCommand({ run: async () => { throw Error('async'); } }), /async/);
});
test('only actually registered slash commands are handled by UiHost', async () => {
  const registry = new UiRegistry(), host = new UiHost({ registry, catalog: {}, shell: {} });
  let calls = 0;
  registry.register('models', 'composer.commands', { id: 'model', value: { run: () => calls++ } });
  assert.equal(await host.command('/file'), false); assert.equal(await host.command('plain text'), false);
  assert.equal(await host.command('/model'), true); assert.equal(calls, 1);
  registry.removeOwner('models'); assert.equal(await host.command('/model'), false);
});
test('module command invalidation follows lifetime and cannot refresh a replacement owner', async () => {
  let ctx, refreshes = 0;
  const registry = new UiRegistry(), host = new UiHost({ registry, shell: {}, catalog: {
    one: { load: async () => ({ version: 1, mount(c) { ctx = c; } }) },
  } });
  const unsubscribe = registry.subscribe('composer.commands', () => refreshes++);
  await host.reconcile(['one']); ctx.refreshCommands(); assert.equal(refreshes, 2);
  host.unmount('one'); const before = refreshes; ctx.refreshCommands(); assert.equal(refreshes, before); unsubscribe();
});
