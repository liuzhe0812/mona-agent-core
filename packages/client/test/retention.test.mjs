import test from 'node:test';
import assert from 'node:assert/strict';
import { RunView, UI_RETAINED_ITEMS } from '../src/index.mjs';
import { frame, tool } from './helpers.mjs';

test('late authoritative completion restores an evicted active tool without retaining its deltas', () => {
  const view = new RunView('r');
  let seq = 0;
  for (let index = 0; index <= UI_RETAINED_ITEMS; index++) {
    const item = tool();
    item.id = `tool-${index}`;
    item.content.call_id = `call-${index}`;
    view.apply(frame(++seq, { type: 'item/started', item }));
  }
  assert.equal(view.state.items.some(item => item.id === 'tool-0'), false);
  assert.equal(view.state.pruned_items, 1);
  view.apply(frame(++seq, { type: 'item/toolCall/outputDelta', item_id: 'tool-0', text: 'intermediate' }));
  assert.equal(view.state.items.length, UI_RETAINED_ITEMS);
  const final = tool();
  final.id = 'tool-0';
  final.state = 'completed';
  final.content.call_id = 'call-0';
  final.content.output = 'authoritative full preview';
  final.content.result = { call_id: 'call-0', status: 'success', content: 'done' };
  final.content.details = { 'test.result': { accepted: true } };
  const event = frame(++seq, { type: 'item/completed', item: final });
  view.apply(event);
  assert.deepEqual(view.state.items.find(item => item.id === 'tool-0'), final);
  assert.equal(view.state.items.length, UI_RETAINED_ITEMS);
  assert.equal(view.state.pruned_items, 2);
  assert.equal(view.apply(event), false);
  const reconnected = new RunView('r');
  reconnected.apply({ kind: 'snapshot', reason: 'source_resync', snapshot: structuredClone(view.state) });
  assert.deepEqual(reconnected.state, view.state);
  const next = tool(); next.id = 'next';
  const started = frame(++seq, { type: 'item/started', item: next });
  view.apply(started); reconnected.apply(started);
  assert.deepEqual(reconnected.state, view.state);
  assert.equal(view.state.items.some(item => item.id === 'tool-0'), false, 'completed entries are evicted before active entries');
});
