import test from 'node:test';
import assert from 'node:assert/strict';
import { outcomeErrorText, partitionItems, durationText, itemBlocks, relativeTime } from './run-view.mjs';

const message = (id, value) => ({ id, state: 'completed', content: { kind: 'agent_message', text: value, truncated: false } });
const tool = { id: 'tool', state: 'completed', content: { kind: 'tool_call', name: 'read_file' } };
const toolCall = (id, step) => ({ id, step, state: 'completed', content: { kind: 'tool_call', name: 'exec_command' } });

test('task stamps read as relative time and reject missing timestamps', () => {
  const now = Date.UTC(2026, 8, 23, 12, 0, 0);
  assert.equal(relativeTime(now - 20_000, now), '刚刚');
  assert.equal(relativeTime(now - 5 * 60_000, now), '5分钟');
  assert.equal(relativeTime(now - 3 * 3_600_000, now), '3小时');
  assert.equal(relativeTime(now - 4 * 86_400_000, now), '4天');
  assert.equal(relativeTime(0, now), '');
  assert.equal(relativeTime(undefined, now), '');
});

test('intermediate message and tools remain in execution order; final answer appears once', () => {
  const items = [message('intro', '正在读取文件'), tool, message('final', '完成')];
  const result = partitionItems({ items, outcome: { status: 'completed', output: '完成' } });
  assert.deepEqual(result.process.map(item => item.id), ['intro', 'tool']);
  assert.equal(result.answer, '完成');
});

test('a streamed answer returns to the process when a later tool arrives', () => {
  const partial = message('message', '接下来读取配置');
  assert.equal(partitionItems({ items: [partial], outcome: null }).answer, partial.content.text);
  const next = partitionItems({ items: [partial, tool], outcome: null });
  assert.deepEqual(next.process.map(item => item.id), ['message', 'tool']);
  assert.equal(next.answer, '');
});

test('outcome output survives snapshots with pruned items and does not discard a different message', () => {
  assert.equal(partitionItems({ items: [], outcome: { status: 'completed', output: '完整结果' } }).answer, '完整结果');
  const result = partitionItems({ items: [message('partial', '截断结果')], outcome: { status: 'completed', output: '完整结果' } });
  assert.equal(result.process.length, 1);
});

test('failure does not present the last intermediate message as a successful final reply', () => {
  const result = partitionItems({ items: [message('m', '即将尝试'), tool], outcome: { status: 'failed', output: null } });
  assert.equal(result.process.length, 2);
  assert.equal(result.answer, '');
});

test('plain final replies do not need an empty execution group; elapsed time is stable at minute boundaries', () => {
  const result = partitionItems({ items: [message('final', '你好')], outcome: { status: 'completed', output: '你好' } });
  assert.equal(result.process.length, 0);
  assert.equal(durationText(59999), '59 秒');
  assert.equal(durationText(60000), '1 分 0 秒');
  assert.equal(durationText(72000), '1 分 12 秒');
});

test('a model call that returned no text does not become an empty process row', () => {
  const tool = toolCall('step-1-tool-0', 1);
  const result = partitionItems({ items: [message('step-1-message', ''), tool], outcome: null });
  assert.deepEqual(result.process.map(item => item.id), ['step-1-tool-0']);
  assert.deepEqual(partitionItems({ items: [message('step-1-message', '\n  \n'), tool], outcome: null }).process.map(item => item.id), ['step-1-tool-0']);
});

test('an intermediate message with text stays in the process', () => {
  const intro = message('step-1-message', '先读取配置');
  const result = partitionItems({ items: [intro, toolCall('step-1-tool-0', 1)], outcome: null });
  assert.deepEqual(result.process.map(item => item.id), ['step-1-message', 'step-1-tool-0']);
});

test('same-step tool calls collapse into one block while other items stay standalone', () => {
  const blocks = itemBlocks([
    toolCall('step-1-tool-0', 1),
    toolCall('step-1-tool-1', 1),
    message('step-1-message', '说明'),
    toolCall('step-2-tool-0', 2),
  ]);
  assert.deepEqual(blocks.map(block => [block.key, block.items.length]), [['step-1', 2], [null, 1], [null, 1]]);
});

test('items without a step are never merged into a group', () => {
  const blocks = itemBlocks([tool, toolCall('step-3-tool-0', 3)]);
  assert.deepEqual(blocks.map(block => [block.key, block.items.length]), [[null, 1], [null, 1]]);
});

test('redacted outcome errors use safe code-specific guidance', () => {
  const message = 'run stopped; inspect trusted host diagnostics for details';
  assert.equal(outcomeErrorText({ code: 'model_transport', message }), '模型请求没有正常结束，请检查供应商地址、网络连接，或供应商是否提前关闭了响应流。');
  assert.equal(outcomeErrorText({ code: 'model_protocol', message }), '模型响应格式异常，请检查供应商接口和模型协议。');
  assert.equal(outcomeErrorText({ code: 'model_authentication', message }), '模型服务拒绝了凭据，请检查 API Key 与账号权限。');
  assert.equal(outcomeErrorText({ code: 'model_quota', message }), '模型额度或账单不足，请检查供应商余额与套餐。');
  assert.equal(outcomeErrorText({ code: 'model_rate_limit', message }), '模型服务正在限流，请稍后重试或降低请求频率。');
  assert.equal(outcomeErrorText({ code: 'model_server', message }), '供应商服务暂时不可用，请稍后重试。');
  assert.equal(outcomeErrorText({ code: 'model_context_window', message }), '请求超出模型上下文上限，请减少上下文或改用更大窗口的模型。');
});

test('non-redacted outcome errors remain unchanged', () => {
  const message = '已有宿主提示';
  assert.equal(outcomeErrorText({ code: 'model_transport', message }), message);
});
