#!/usr/bin/env node
import { createServer } from 'node:http';
import { randomUUID } from 'node:crypto';
import { pathToFileURL } from 'node:url';
import { startUiServer } from '../../../scripts/dev-web.mjs';
import { RunView } from '../../../packages/client/src/index.mjs';

export const HOST = '127.0.0.1';
export const UI_PORT = 4175;
export const API_PORT = 8789;
export const TEST_TOKEN = 'mona-ui-fixture-test-token-20260922';
export const UI_ORIGIN = `http://${HOST}:${UI_PORT}`;
export const API_ORIGIN = `http://${HOST}:${API_PORT}`;

const encoder = new TextEncoder();
const runs = new Map();
const requestRuns = new Map();

const MODEL_SETTINGS_FIXTURE = Object.freeze({
  revision: 7,
  providers: [
    {
      id: 'fixture-openai', name: 'Fixture OpenAI', api_base: 'http://127.0.0.1:9999/v1',
      has_key: true, builtin: false, protocol: 'chat_completions', generation: {},
      models: [{ id: 'fixture-chat', enabled: true }, { id: 'fixture-code', enabled: true }],
    },
    {
      id: 'fixture-local', name: 'Fixture Local', api_base: 'http://127.0.0.1:9998/v1',
      has_key: false, builtin: true, protocol: 'chat_completions', generation: {},
      models: [{ id: 'fixture-small', enabled: false }],
    },
  ],
  default: { provider_id: 'fixture-openai', model_id: 'fixture-chat' },
});
const CAPABILITIES_FIXTURE = Object.freeze({
  revision: 4,
  components: [
    {
      id: 'skills', name: 'Skills', description: '按需读取已安装技能的说明和资源。',
      enabled: false, restart_required: true,
    },
    {
      id: 'compaction', name: '上下文压缩', description: '接近请求预算时压缩较早的已结算历史。',
      enabled: true, restart_required: false,
    },
  ],
  tools: [
    {
      id: 'grep', name: '内容搜索', description: '递归搜索文件内容。',
      enabled: false, restart_required: false,
    },
    {
      id: 'find', name: '文件查找', description: '按名称模式递归查找文件。',
      enabled: true, restart_required: true,
    },
  ],
});

function clone(value) {
  return structuredClone(value);
}

function toolResult(callId, status, content, extras = {}) {
  return {
    call_id: callId,
    status,
    content,
    blocks: extras.blocks ?? null,
    redacted: false,
    truncated: false,
    original_bytes: encoder.encode(content).length,
    artifact: extras.artifact ?? null,
  };
}

function messageItem(run, state, text = '') {
  return {
    id: `step-${run.step}-message`,
    step: run.step,
    state,
    content: { kind: 'agent_message', text, truncated: false },
  };
}

function toolItem(run, index, state, content) {
  return {
    id: `step-${run.step}-tool-${index}`,
    step: run.step,
    state,
    content: {
      kind: 'tool_call',
      call_id: content.call_id ?? null,
      name: content.name ?? '',
      arguments_text: content.arguments_text ?? '',
      arguments: content.arguments ?? null,
      arguments_truncated: false,
      output: content.output ?? '',
      output_truncated: false,
      result: content.result ?? null,
      details: content.details ?? {},
    },
  };
}

function corsHeaders(request) {
  const origin = request.headers.origin;
  const allowed = request.uiFixtureOriginAllowed ?? (origin === UI_ORIGIN || origin === 'http://localhost:4175');
  return {
    'Cache-Control': 'no-store',
    'Referrer-Policy': 'no-referrer',
    'X-Content-Type-Options': 'nosniff',
    ...(allowed ? { 'Access-Control-Allow-Origin': origin, Vary: 'Origin' } : {}),
  };
}

function sendJson(request, response, status, payload) {
  const body = JSON.stringify(payload);
  response.writeHead(status, {
    ...corsHeaders(request),
    'Content-Type': 'application/json; charset=utf-8',
    'Content-Length': Buffer.byteLength(body),
  });
  response.end(body);
}

function sendError(request, response, status, code, message) {
  sendJson(request, response, status, { code, message });
}

async function readJson(request) {
  const chunks = [];
  let size = 0;
  for await (const chunk of request) {
    size += chunk.length;
    if (size > 1024 * 1024) throw Object.assign(new Error('请求体过大。'), { status: 413 });
    chunks.push(chunk);
  }
  if (!size) return {};
  try {
    return JSON.parse(Buffer.concat(chunks).toString('utf8'));
  } catch {
    throw Object.assign(new Error('请求体必须是 JSON。'), { status: 400 });
  }
}

function logRequest(request) {
  const pathname = new URL(request.url || '/', API_ORIGIN).pathname;
  console.log(`[fixture] ${request.method || 'GET'} ${pathname}`);
}

function frameForEvent(envelope) {
  return { kind: 'event', envelope };
}

function frameSequence(frame) {
  if (frame.kind === 'event') return frame.envelope.seq;
  if (frame.kind === 'snapshot') return frame.snapshot.seq;
  return null;
}

function writeSse(response, frame) {
  if (response.writableEnded) return;
  const sequence = frameSequence(frame);
  const lines = [];
  if (sequence != null) lines.push(`id: ${sequence}`);
  lines.push('event: agent', `data: ${JSON.stringify(frame)}`, '', '');
  try { response.write(lines.join('\n')); } catch { /* client disconnected */ }
}

function closeSubscriber(run, subscriber) {
  run.subscribers.delete(subscriber);
  clearInterval(subscriber.heartbeat);
  if (!subscriber.response.writableEnded) subscriber.response.end();
}

function emit(run, event) {
  if (run.completed) return;
  const envelope = {
    protocol_version: 2,
    run_id: run.runId,
    seq: run.nextSeq++,
    event,
  };
  run.events.push(envelope);
  run.view.apply(frameForEvent(envelope));
  run.snapshot = run.view.state;
  for (const subscriber of [...run.subscribers]) {
    if (envelope.seq <= subscriber.cursor) continue;
    subscriber.cursor = envelope.seq;
    writeSse(subscriber.response, frameForEvent(envelope));
    if (event.type === 'run/completed') setTimeout(() => closeSubscriber(run, subscriber), 0);
  }
  if (event.type === 'run/completed') {
    run.completed = true;
    for (const timer of run.timers) clearTimeout(timer);
    run.timers.clear();
  }
}

function schedule(run, delay, callback) {
  const timer = setTimeout(() => {
    run.timers.delete(timer);
    if (!run.completed) callback();
  }, delay);
  run.timers.add(timer);
}

function complete(run, status, output, error = null) {
  if (run.completed) return;
  emit(run, {
    type: 'run/completed',
    outcome: {
      status,
      output,
      error,
      task_usage: { model_calls: 2, reported_tokens: 0, usage_complete: false },
      steps: run.step,
    },
  });
}

function longCommand() {
  const payload = '本地 UI 测试数据：参数片段。'.repeat(220);
  return `node -e "console.log(${JSON.stringify(payload)})"`;
}

const TINY_PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9WlGQAAAAASUVORK5CYII=';
const RENDERER_DIFF = {
  path: 'src/example.rs',
  edits: 1,
  bytes: 42,
  summary: 'Updated renderer fixture.',
  diff: '--- a/src/example.rs\n+++ b/src/example.rs\n@@ -1 +1 @@\n-old_value\n+new_value',
  diffTruncated: false,
  firstChangedLine: 1,
};
const RENDERER_MARKDOWN = [
  '# 富内容渲染',
  '',
  '**粗体包含 _嵌套斜体_**，转义竖线：\\|，自动链接 https://example.com/docs 。',
  '',
  '- [x] 已完成任务',
  '- [ ] 待完成任务',
  '',
  '| 名称 | 值 |',
  '| --- | ---: |',
  '| A\\|B | 42 |',
  '',
  '> 引用第一层',
  '> > 引用第二层',
  '',
  '```rust',
  'fn main() {',
  ...Array.from({ length: 20 }, (_, index) => `    let value_${index + 1} = ${index + 1};`),
  '    println!("done");',
  '}',
  '```',
  '',
  '$$\\frac{a_1 + b^2}{\\sqrt{c}} \\le 42$$',
  '',
  '```mermaid',
  'flowchart LR',
  'A[开始] -->|执行| B{检查}',
  'B --> C[完成]',
  '```',
  '',
  '安全文本：<img src=x onerror=alert(1)>',
].join('\n');
const SPILL_FIXTURES = new Map([
  ['sp_fixture_resource', 'fixture resource content from trusted host\n'],
  ['sp_fixture_log', 'fixture archived output line 1\nfixture archived output line 2\n'],
]);

function scheduleTool(run, index, { failed = false, delay = 0 } = {}) {
  const callId = `fixture-call-${index}`;
  const renderer = run.mode === 'renderer';
  const name = renderer ? (index === 1 ? 'read' : 'edit') : (index === 1 ? 'read_file' : 'exec_command');
  const args = index === 1
    ? { path: 'README.md', purpose: '本地 UI 测试数据：读取参考文件' }
    : renderer ? { path: 'src/example.rs', edits: [{ oldText: 'old_value', newText: 'new_value' }] }
      : { command: longCommand(), cwd: 'D:\\mona-ui-fixture', purpose: '本地 UI 测试数据：验证长参数展开' };
  const output = failed
    ? '本地 UI 测试数据：工具故意失败。'
    : renderer && index === 1 ? '本地 UI 测试数据：返回图片和资源。'
      : renderer ? '本地 UI 测试数据：文件已修改。'
      : index === 1
      ? '本地 UI 测试数据：README.md 已读取。'
      : '本地 UI 测试数据：命令输出已回放。';
  schedule(run, delay, () => {
    const itemId = `step-${run.step}-tool-${index}`;
    emit(run, { type: 'item/started', item: toolItem(run, index, 'running', { call_id: callId, name }) });
    emit(run, {
      type: 'item/toolCall/argumentsDelta',
      item_id: itemId,
      call_id: callId,
      name,
      delta: JSON.stringify(args, null, 2),
    });
    schedule(run, 650, () => {
      emit(run, { type: 'item/toolCall/outputDelta', item_id: itemId, text: output });
      schedule(run, 250, () => {
        const result = renderer && index === 1
          ? toolResult(callId, 'success', '[image: image/png]\n[resource: text/plain]', {
            blocks: [
              { type: 'image', media_type: 'image/png', source: { kind: 'base64', data: TINY_PNG } },
              { type: 'resource', bytes: encoder.encode(SPILL_FIXTURES.get('sp_fixture_resource')).length, media_type: 'text/plain', name: 'fixture.txt', reference: { uri: 'spill:sp_fixture_resource', bytes: encoder.encode(SPILL_FIXTURES.get('sp_fixture_resource')).length } },
            ],
          })
          : renderer
            ? toolResult(callId, 'success', 'Successfully replaced 1 block in src/example.rs.', {
              artifact: { uri: 'spill:sp_fixture_log', bytes: encoder.encode(SPILL_FIXTURES.get('sp_fixture_log')).length },
            })
            : toolResult(callId, failed ? 'error' : 'success', output);
        emit(run, {
          type: 'item/completed',
          item: toolItem(run, index, failed ? 'failed' : 'completed', {
            call_id: callId,
            name,
            arguments: args,
            arguments_text: JSON.stringify(args, null, 2),
            output,
            result,
            details: renderer && index === 2 ? { 'coding.diff': RENDERER_DIFF, 'fixture.meta': { source: 'local' } } : {},
          }),
        });
      });
    });
  });
}

function scheduleRun(run) {
  emit(run, { type: 'step/started', step: run.step });
  const intermediate = messageItem(run, 'running');
  schedule(run, 350, () => {
    emit(run, { type: 'item/started', item: intermediate });
    schedule(run, 350, () => {
      emit(run, {
        type: 'item/agentMessage/delta',
        item_id: intermediate.id,
        text: '本地 UI 测试数据：正在按事件顺序执行任务。',
      });
      schedule(run, 300, () => {
        emit(run, {
          type: 'item/completed',
          item: messageItem(run, 'completed', '本地 UI 测试数据：正在按事件顺序执行任务。'),
        });
      });
    });
  });

  scheduleTool(run, 1, { delay: 1600 });
  scheduleTool(run, 2, { failed: run.mode === 'failed', delay: 3300 });

  if (run.mode === 'long') {
    schedule(run, 29500, () => finishSuccess(run));
  } else {
    schedule(run, 5100, () => finishSuccess(run));
  }
}

function finishSuccess(run) {
  if (run.mode === 'failed') {
    emit(run, { type: 'step/completed', step: run.step });
    complete(run, 'failed', '本地 UI 测试数据：任务失败。', {
      code: 'fixture_failed',
      message: '本地 UI 测试数据：故意失败场景。',
    });
    return;
  }
  emit(run, { type: 'step/completed', step: run.step });
  run.step = 2;
  emit(run, {type: 'step/started', step: 2});
  const finalText = run.mode === 'renderer' ? RENDERER_MARKDOWN
    : '本地 UI 测试数据：任务已完成。\n\n已检查文件与执行输出，执行过程可以展开，每个步骤也可以单独查看。\n\n1. 保留真实事件的先后顺序。\n2. 流式更新保持展开状态。\n3. 最终回复显示在过程区之外。\n\n纯文本检查：<img src=x onerror=alert(1)>';
  const final = messageItem(run, 'running');
  schedule(run, 350, () => {
    emit(run, { type: 'item/started', item: final });
    schedule(run, 350, () => {
      emit(run, { type: 'item/agentMessage/delta', item_id: final.id, text: finalText });
      schedule(run, 300, () => {
        emit(run, { type: 'item/completed', item: messageItem(run, 'completed', finalText) });
        complete(run, 'completed', finalText);
      });
    });
  });
}

function createRun(prompt, requestId) {
  const run = {
    runId: `fixture-${randomUUID()}`,
    requestId,
    prompt,
    mode: prompt.includes('渲染能力') ? 'renderer' : prompt.includes('长任务') ? 'long' : prompt.includes('失败') ? 'failed' : 'normal',
    step: 1,
    nextSeq: 1,
    events: [],
    timers: new Set(),
    subscribers: new Set(),
    snapshot: null,
    completed: false,
  };
  run.view = new RunView(run.runId);
  run.snapshot = run.view.state;
  runs.set(run.runId, run);
  requestRuns.set(requestId, run.runId);
  emit(run, { type: 'run/started' });
  scheduleRun(run);
  return run;
}

function cancelRun(run) {
  if (run.completed) return false;
  for (const timer of run.timers) clearTimeout(timer);
  run.timers.clear();
  for (const item of run.snapshot.items.filter(item => ['pending', 'running'].includes(item.state))) emit(run, {type: 'item/completed', item: {...item, state: 'cancelled'}});
  complete(run, 'cancelled', '本地 UI 测试数据：任务已取消。');
  return true;
}

function authenticate(request) {
  return request.headers.authorization === `Bearer ${TEST_TOKEN}`;
}

async function handleEvents(request, response, run) {
  const headerCursor = request.headers['last-event-id'];
  const cursor = headerCursor == null || headerCursor === '' ? null : Number(headerCursor);
  if (cursor != null && (!Number.isSafeInteger(cursor) || cursor < 0)) {
    sendError(request, response, 400, 'invalid_request', 'Last-Event-ID 必须是非负整数。');
    return;
  }
  response.writeHead(200, {
    ...corsHeaders(request),
    'Content-Type': 'text/event-stream; charset=utf-8',
    Connection: 'keep-alive',
    'X-Accel-Buffering': 'no',
  });
  response.flushHeaders();
  const subscriber = { response, cursor: cursor ?? -1, heartbeat: null };
  run.subscribers.add(subscriber);
  response.on('close', () => {
    clearInterval(subscriber.heartbeat);
    run.subscribers.delete(subscriber);
  });

  if (cursor == null) {
    subscriber.cursor = run.snapshot.seq;
    writeSse(response, { kind: 'snapshot', reason: 'initial', snapshot: clone(run.snapshot) });
  } else {
    for (const envelope of run.events) {
      if (envelope.seq > cursor) {
        subscriber.cursor = envelope.seq;
        writeSse(response, frameForEvent(envelope));
      }
    }
  }
  if (run.completed) {
    closeSubscriber(run, subscriber);
    return;
  }
  subscriber.heartbeat = setInterval(() => {
    if (!response.writableEnded) response.write(': fixture-heartbeat\n\n');
  }, 10000);
}

function findRun(request, response, runId) {
  const run = runs.get(runId);
  if (!run) {
    sendError(request, response, 404, 'not_found', '本地 UI 测试数据：任务不存在。');
    return null;
  }
  return run;
}

async function handleApiRequest(request, response) {
  logRequest(request);
  if (request.method === 'OPTIONS') {
    response.writeHead(204, {
      ...corsHeaders(request),
      'Access-Control-Allow-Methods': 'GET,PUT,POST,DELETE,OPTIONS',
      'Access-Control-Allow-Headers': 'Authorization, Content-Type, Last-Event-ID',
      'Access-Control-Max-Age': '60',
    });
    response.end();
    return;
  }
  if (!authenticate(request)) {
    sendError(request, response, 401, 'unauthorized', 'valid bearer authorization is required');
    return;
  }

  const url = new URL(request.url || '/', API_ORIGIN);
  if (url.pathname === '/v1/info' && request.method === 'GET') {
    sendJson(request, response, 200, { protocol_version: 2, stream: 'sse', replay: 'bounded_in_memory', durable: false });
    return;
  }
  if (url.pathname === '/api/ui' && request.method === 'GET') {
    if (request.uiFixtureManagement) sendJson(request, response, 200, { version: 1, modules: ['capabilities', 'models'], capabilities: {
      'model-management': { compiled: true, enabled: true, active: true, configurable: false, restart_required: false },
    } });
    else sendError(request, response, 404, 'not_found', '此测试宿主没有产品 UI 模块。');
    return;
  }
  if (url.pathname === '/api/model-settings' && request.method === 'GET') {
    if (request.uiFixtureManagement) sendJson(request, response, 200, clone(request.uiFixtureModels));
    else sendError(request, response, 404, 'not_found', '模型管理插件未安装。');
    return;
  }
  if (url.pathname === '/api/model-settings/default' && request.method === 'POST' && request.uiFixtureManagement) {
    const body = await readJson(request), state = request.uiFixtureModels;
    if (body.revision !== state.revision) { sendError(request, response, 409, 'conflict', '模型配置版本已变更。'); return; }
    const provider = state.providers.find(p => p.id === body.provider_id);
    if (!provider?.models.some(m => m.id === body.model_id)) { sendError(request, response, 400, 'invalid_request', '模型不存在。'); return; }
    state.default = { provider_id: body.provider_id, model_id: body.model_id }; state.revision++;
    sendJson(request, response, 200, clone(state)); return;
  }
  if (url.pathname === '/api/capabilities' && request.method === 'GET') {
    if (request.uiFixtureManagement) sendJson(request, response, 200, clone(CAPABILITIES_FIXTURE));
    else sendError(request, response, 404, 'not_found', '组件与工具管理接口未安装。');
    return;
  }
  const spillMatch = url.pathname.match(/^\/api\/spill\/([^/]+)\/(sp_[A-Za-z0-9_]+)$/);
  if (spillMatch && request.method === 'GET') {
    const run = runs.get(decodeURIComponent(spillMatch[1]));
    const content = SPILL_FIXTURES.get(spillMatch[2]);
    if (!run || content == null) {
      sendError(request, response, 404, 'not_found', '测试归档不存在。');
      return;
    }
    const bytes = Buffer.from(content, 'utf8');
    const offset = Number(url.searchParams.get('offset') || 0);
    const limit = Math.min(16 * 1024, Number(url.searchParams.get('limit') || 16 * 1024));
    if (!Number.isSafeInteger(offset) || offset < 0 || !Number.isSafeInteger(limit) || limit < 1) {
      sendError(request, response, 400, 'invalid_request', '归档分页参数无效。');
      return;
    }
    const end = Math.min(bytes.length, offset + limit);
    sendJson(request, response, 200, {
      id: spillMatch[2], offset, next_offset: end, total_bytes: bytes.length,
      eof: end >= bytes.length, text: bytes.subarray(offset, end).toString('utf8'),
    });
    return;
  }
  const capabilityMatch = url.pathname.match(/^\/api\/capabilities\/([^/]+)$/);
  if (capabilityMatch && request.method === 'PUT') {
    if (!request.uiFixtureManagement) {
      sendError(request, response, 404, 'not_found', '组件与工具管理接口未安装。');
      return;
    }
    try {
      const body = await readJson(request);
      if (!Number.isSafeInteger(body.revision) || body.revision < 0 || typeof body.enabled !== 'boolean') {
        sendError(request, response, 400, 'invalid_request', 'revision 和 enabled 无效。');
        return;
      }
      const id = decodeURIComponent(capabilityMatch[1]);
      const value = clone(CAPABILITIES_FIXTURE);
      const item = [...value.components, ...value.tools].find(entry => entry.id === id);
      if (!item) {
        sendError(request, response, 404, 'not_found', '设置项不存在。');
        return;
      }
      value.revision = body.revision + 1;
      item.enabled = body.enabled;
      item.restart_required = true;
      sendJson(request, response, 200, value);
    } catch (error) {
      sendError(request, response, error.status || 400, 'invalid_request', error.message);
    }
    return;
  }
  if (url.pathname === '/v1/runs' && request.method === 'POST') {
    try {
      const body = await readJson(request);
      if (typeof body.request_id !== 'string' || !body.request_id || typeof body.prompt !== 'string' || !body.prompt.trim()) {
        sendError(request, response, 400, 'invalid_request', 'request_id 和 prompt 必须存在。');
        return;
      }
      const existing = requestRuns.get(body.request_id);
      if (existing && runs.has(existing)) {
        sendJson(request, response, 200, { run_id: existing, reused: true });
        return;
      }
      const run = createRun(body.prompt.trim(), body.request_id);
      sendJson(request, response, 200, { run_id: run.runId, reused: false });
    } catch (error) {
      sendError(request, response, error.status || 400, 'invalid_request', error.message);
    }
    return;
  }

  const match = url.pathname.match(/^\/v1\/runs\/([^/]+)(?:\/(events|snapshot|result|cancel|input))?$/);
  if (!match) {
    sendError(request, response, 404, 'not_found', '本地 UI 测试数据：端点不存在。');
    return;
  }
  const runId = decodeURIComponent(match[1]);
  const action = match[2] || '';
  const run = findRun(request, response, runId);
  if (!run) return;

  if (action === 'events' && request.method === 'GET') {
    await handleEvents(request, response, run);
    return;
  }
  if (action === 'snapshot' && request.method === 'GET') {
    sendJson(request, response, 200, clone(run.snapshot));
    return;
  }
  if (action === 'result' && request.method === 'GET') {
    sendJson(request, response, 200, { run_id: run.runId, outcome: clone(run.snapshot.outcome) });
    return;
  }
  if (action === 'cancel' && request.method === 'POST') {
    sendJson(request, response, 200, { run_id: run.runId, signalled: cancelRun(run) });
    return;
  }
  if (action === 'input' && request.method === 'POST') {
    try {
      const body = await readJson(request);
      if (typeof body.request_id !== 'string' || typeof body.text !== 'string') {
        sendError(request, response, 400, 'invalid_request', 'request_id 和 text 必须存在。');
        return;
      }
      if (run.completed) {
        sendError(request, response, 409, 'closed', '本地 UI 测试数据：任务已结束。');
        return;
      }
      emit(run, { type: 'input/applied' });
      sendJson(request, response, 200, { request_id: body.request_id, applied: true });
    } catch (error) {
      sendError(request, response, error.status || 400, 'invalid_request', error.message);
    }
    return;
  }
  if (!action && request.method === 'DELETE') {
    if (!run.completed) cancelRun(run);
    runs.delete(run.runId);
    requestRuns.delete(run.requestId);
    sendJson(request, response, 204, {});
    return;
  }
  sendError(request, response, 405, 'method_not_allowed', '方法不支持。');
}

export function createFixtureApiServer({ allowOrigin, management = false } = {}) {
  const models = clone(MODEL_SETTINGS_FIXTURE);
  return createServer((request, response) => {
    // Tests can use isolated ephemeral ports; the default fixture keeps its original allowlist.
    if (allowOrigin) request.uiFixtureOriginAllowed = allowOrigin(request.headers.origin) === true;
    request.uiFixtureManagement = management === true;
    request.uiFixtureModels = models;
    void handleApiRequest(request, response).catch((error) => {
      if (!response.headersSent) sendError(request, response, 500, 'internal', error.message || 'fixture server error');
      else response.destroy();
    });
  });
}

export async function startFixtureServers({ host = HOST, uiPort = UI_PORT, apiPort = API_PORT } = {}) {
  const uiServer = await startUiServer({ host, port: uiPort, endpoint: `http://${host}:${apiPort}`, token: TEST_TOKEN });
  uiServer.on('request', (request) => {
    const pathname = new URL(request.url || '/', `http://${host}:${uiPort}`).pathname;
    console.log(`[fixture-ui] ${request.method || 'GET'} ${pathname}`);
  });
  const apiServer = createFixtureApiServer();
  await new Promise((resolve, reject) => {
    apiServer.once('error', reject);
    apiServer.listen(apiPort, host, () => { apiServer.off('error', reject); resolve(); });
  });
  return { uiServer, apiServer, uiOrigin: `http://${host}:${uiPort}`, apiOrigin: `http://${host}:${apiPort}` };
}

export async function main() {
  const servers = await startFixtureServers();
  let closed = false;
  const shutdown = async (signal) => {
    if (closed) return;
    closed = true;
    console.log(`[fixture] ${signal} shutting down`);
    for (const run of runs.values()) {
      for (const timer of run.timers) clearTimeout(timer);
      for (const subscriber of [...run.subscribers]) closeSubscriber(run, subscriber);
    }
    await Promise.all([
      new Promise((resolve) => servers.uiServer.close(() => resolve())),
      new Promise((resolve) => servers.apiServer.close(() => resolve())),
    ]);
  };
  process.once('SIGINT', () => { void shutdown('SIGINT').then(() => { process.exitCode = 130; }); });
  process.once('SIGTERM', () => { void shutdown('SIGTERM').then(() => { process.exitCode = 143; }); });
  console.log('Mona Web UI Stream v2 本地 fixture');
  console.log(`- Web UI: ${servers.uiOrigin}`);
  console.log(`- API: ${servers.apiOrigin}`);
  console.log('- 测试 token 已固定但不会写入请求日志');
  await new Promise(() => {});
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  });
}
