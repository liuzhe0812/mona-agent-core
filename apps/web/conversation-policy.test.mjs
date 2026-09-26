import test from 'node:test';
import assert from 'node:assert/strict';
import { workSummary, workPolicy, canFoldTurn } from './conversation-policy.mjs';
import { validateStatistics, contextPercent, tokenLabel, cachePercent, cachePercentLabel, speedLabel, estimateTokenLabel, durationLabel } from './conversation-metrics.mjs';
import { PaneState } from './pane-state.mjs';
const tool = (name, state = 'completed', args = {}) => ({ state, content: { kind: 'tool_call', name, arguments: args } });
test('process summaries use actual categories and never treat argument preparation as execution', () => {
  assert.equal(workSummary([tool('read'), tool('read'), tool('shell')]), '2 次读取、1 条命令');
  assert.match(workSummary([tool('edit', 'failed')]), /1 项未成功/);
  assert.equal(workSummary([tool('read', 'pending', { path: 'not-dispatched' })], { running: true }), '正在准备读取文件');
  assert.equal(workSummary([tool('shell', 'running', { command: 'cargo test' })], { running: true }), '正在运行命令 · cargo test');
  assert.equal(workSummary([tool('shell', 'running', { command: 'private' })], { running: true, detail: false }), '正在运行命令');
  assert.match(workSummary([tool('read')], { partial: true }), /^已加载：/);
});
test('failure, interruption and supplemental input stay outside successful whole-turn hiding', () => {
  for (const status of ['failed', 'cancelled', 'timed_out', 'limited']) assert.equal(canFoldTurn({ outcome: { status } }), false);
  assert.equal(canFoldTurn({ outcome: null }), false);
  assert.equal(canFoldTurn({ outcome: { status: 'completed' } }, true), false);
  assert.equal(canFoldTurn({ outcome: { status: 'completed' } }), true);
  assert.equal(workPolicy().unfold, false); assert.equal(workPolicy().detail, true);
});
const reading = { session_id: 's-one', revision: 2, turns: 2, steps: 8, latest_step: 3, active: false, reported_tokens: 107477, model_calls: 8, usage_complete: true, latest_run_id: 'r', input_tokens: 105025, output_tokens: 2452, cache_read_tokens: 82560, cache_write_tokens: 0, model_time_ms: 58200, tool_time_ms: 60000, ttft_ms: 1600, ttft_samples: 1, decode_ms: 42000, decode_tokens: 2452, context: { run_id: 'r', tokens: 12600, capacity: 262000, system_tokens: 2000, tool_tokens: 7300, message_tokens: 3300, provider_anchored: true, observed_at: 1 } };
test('statistics derive cache, latency and speed only from validated source counters', () => {
  const v = validateStatistics({ ...reading, cache_hit_percent: 0, tokens_per_second: 0 }, 's-one');
  assert.equal(v.cache_complete, true); assert.equal(v.uncached_input_tokens, 22465); assert.ok(v.cache_hit_percent > 78 && v.cache_hit_percent < 79); assert.equal(cachePercentLabel(v.cache_hit_percent), '79%');
  assert.equal(v.tokens_per_second, 2452 / 42); assert.equal(v.average_ttft_ms, 1600); assert.equal(contextPercent(v.context), 5);
  assert.equal(estimateTokenLabel(v.context.tokens), '12.6K'); assert.equal(estimateTokenLabel(v.context.capacity), '262K');
  assert.equal(speedLabel(100, 1000), '100 tok/s'); assert.equal(durationLabel(58200), '58.2秒'); assert.equal(durationLabel(60000), '1分0秒');
  assert.equal(cachePercentLabel(99.9), '>99%'); assert.equal(cachePercentLabel(100), '100%');
  const unknown = validateStatistics({ ...reading, cache_read_tokens: null, cache_write_tokens: null }, 's-one');
  assert.equal(unknown.cache_complete, false); assert.equal(unknown.uncached_input_tokens, null); assert.equal(unknown.cache_hit_percent, null);
  const inconsistent = validateStatistics({ ...reading, cache_read_tokens: 100000, cache_write_tokens: 6000 }, 's-one');
  assert.equal(inconsistent.cache_complete, false); assert.equal(inconsistent.cache_hit_percent, null);
  assert.equal(contextPercent(null), null); assert.equal(contextPercent({ tokens: 0, capacity: 32000 }), 0);
  assert.equal(contextPercent({ tokens: null, capacity: 32000 }), null); assert.equal(tokenLabel(null), '—');
  assert.throws(() => validateStatistics(reading, 's-two'));
  assert.throws(() => validateStatistics({ ...reading, reported_tokens: -1 }, 's-one'));
  assert.throws(() => validateStatistics({ ...reading, ttft_samples: -1 }, 's-one'));
  assert.throws(() => validateStatistics({ ...reading, context: { ...reading.context, run_id: 'wrong' } }, 's-one'));
});
test('file page and expansion preferences persist per scope without restoring executable resources', () => {
  const data = new Map(), storage = { getItem: k => data.get(k), setItem: (k, v) => data.set(k, v) };
  const state = new PaneState(() => storage); state.configure('http://127.0.0.1:8787');
  state.save('session:s-one', [{ path: 'readme.md', mode: 'preview', wrap: false }], 'readme.md', { filesPage: true, expanded: true });
  state.save('session:s-two', [], null, { expanded: false });
  const restored = new PaneState(() => storage); restored.configure('http://127.0.0.1:8787');
  assert.equal(restored.get('session:s-one').filesPage, true); assert.equal(restored.get('session:s-one').expanded, true); assert.equal(restored.get('session:s-two').expanded, false);
  assert.ok(![...data.values()].join('').includes('token')); assert.ok(![...data.values()].join('').includes('terminal'));
});
