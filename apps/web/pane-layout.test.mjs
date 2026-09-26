import test from 'node:test';
import assert from 'node:assert/strict';
import { PaneLayout } from './pane-layout.mjs';
import { PaneState } from './pane-state.mjs';
const file = path => ({ path, mode: 'source', wrap: false });
const storage = () => { const map = new Map(); return { getItem: k => map.get(k) || null, setItem: (k, v) => map.set(k, v), map }; };

test('two panes select independently and moving the last member collapses only the empty pane', () => {
  const layout = new PaneLayout(); for (const id of ['tree', 'a', 'b']) layout.add(id);
  assert.equal(layout.divide('a'), true); assert.deepEqual(layout.groups, [['tree', 'b'], ['a']]);
  layout.activate('tree'); assert.deepEqual(layout.selected, ['tree', 'a']);
  layout.add('c', 1); assert.deepEqual(layout.selected, ['tree', 'c']);
  layout.remove('c'); assert.equal(layout.selected[1], 'a'); layout.move('a', 0);
  assert.equal(layout.split, false); assert.deepEqual(layout.order, ['tree', 'b', 'a']);
});
test('split cannot invent an empty pane or a third pane and merge retains the selected tab', () => {
  const layout = new PaneLayout(); layout.add('a'); assert.equal(layout.divide(), false);
  layout.add('b'); assert.equal(layout.divide('b'), true); assert.equal(layout.divide('a'), false);
  layout.merge(); assert.equal(layout.active, 'b'); assert.deepEqual(layout.groups, [['a', 'b'], []]);
});
test('tab reordering has consistent before/after semantics within and across panes', () => {
  const layout = new PaneLayout(); ['a', 'b', 'c', 'd'].forEach(id => layout.add(id));
  layout.reorder('a', 'c', true); assert.deepEqual(layout.order, ['b', 'c', 'a', 'd']);
  layout.reorder('d', 'b'); assert.deepEqual(layout.order, ['d', 'b', 'c', 'a']);
  layout.divide('a'); layout.reorder('c', 'a', true); assert.deepEqual(layout.groups, [['d', 'b'], ['a', 'c']]);
});
test('bounded layout operations always maintain unique membership and valid selections', () => {
  const layout = new PaneLayout(); let seed = 42; const random = n => { seed = (seed * 1664525 + 1013904223) >>> 0; return seed % n; };
  for (let step = 0; step < 2000; step++) {
    const id = 'tab-' + random(16), action = random(6);
    if (action === 0) layout.add(id); else if (action === 1) layout.remove(id); else if (action === 2) layout.activate(id);
    else if (action === 3) layout.move(id, random(2), random(4)); else if (action === 4) layout.divide(id); else layout.merge();
    assert.equal(layout.order.length, new Set(layout.order).size);
    for (let side = 0; side < 2; side++) assert.ok(!layout.groups[side].length ? layout.selected[side] === null : layout.groups[side].includes(layout.selected[side]));
    assert.ok(!layout.split || layout.groups[0].length > 0); assert.ok(layout.groups[layout.focus].length || !layout.order.length);
  }
});
test('file layout restores both pane orders, selections, ratio and per-scope state', () => {
  const store = storage(), state = new PaneState(() => store); state.configure('http://localhost:8080');
  const layout = { expanded: true, filesPage: true, panes: [['page:files', 'file:a.md'], ['file:b.rs']], selected: ['page:files', 'file:b.rs'], focus: 1, ratio: 43, fullscreen: true };
  assert.equal(state.save('session:one', [file('a.md'), file('b.rs')], 'b.rs', layout), true);
  assert.equal(state.save('session:two', [file('second.md')], 'second.md', { expanded: false }), true);
  const restored = new PaneState(() => store); restored.configure('http://localhost:8080/');
  assert.deepEqual(restored.get('session:one').panes, layout.panes); assert.deepEqual(restored.get('session:one').selected, layout.selected);
  assert.equal(restored.get('session:one').ratio, 43); assert.equal(restored.get('session:one').fullscreen, true); assert.equal(restored.get('session:two').expanded, false);
});
test('layout persistence rejects duplicate or invented file membership and isolates endpoints', () => {
  const store = storage(), state = new PaneState(() => store); state.configure('http://localhost:8080');
  assert.equal(state.save('session:one', [file('a')], 'a', { panes: [['file:a'], ['file:a']] }), false);
  assert.equal(state.save('session:one', [file('a')], 'a', { panes: [['file:secret'], []] }), false);
  state.save('session:one', [file('a')], 'a'); state.configure('http://localhost:8090'); assert.equal(state.get('session:one'), undefined);
});
test('failed storage writes retain the previously confirmed layout and records stay bounded', () => {
  const store = storage(), state = new PaneState(() => store); state.configure('http://localhost:8080'); state.save('session:one', [file('old')], 'old');
  const write = store.setItem; store.setItem = () => { throw Error('quota'); };
  assert.equal(state.save('session:one', [file('new')], 'new'), false); assert.equal(state.get('session:one').activePath, 'old'); store.setItem = write;
  for (let i = 0; i < 40; i++) state.save('session:s' + i, [file('file')], 'file'); assert.equal(state.records.length, 16);
});
test('untrusted stored layout is bounded and cannot persist content, credentials or unsafe paths', () => {
  const store = storage(), state = new PaneState(() => store); state.configure('http://localhost:8080');
  state.save('session:one', [{ ...file('safe.md'), token: 'secret', text: 'body' }, file('../no'), file('/absolute')], 'safe.md');
  const raw = store.getItem('mona.web.file-tabs'); assert.ok(!raw.includes('secret') && !raw.includes('body') && !raw.includes('../no'));
  for (const record of [null, { scope: 'session:one', files: [null] }, { scope: 'session:one', files: [file('safe')], panes: [[], ['file:other']] }]) {
    store.setItem('mona.web.file-tabs', JSON.stringify({ version: 2, endpoint: 'http://localhost:8080', records: [record] })); state.configure('http://localhost:8080'); assert.equal(state.records.length, 0);
  }
});
