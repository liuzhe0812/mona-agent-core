import test from 'node:test';
import assert from 'node:assert/strict';
import { SessionsClient } from './sessions.mjs';

const json = (body, status = 200) => new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });
function setup(t, fetcher) {
  const old = globalThis.fetch; globalThis.fetch = fetcher; t.after(() => { globalThis.fetch = old; });
  const client = new SessionsClient(); client.configure('http://127.0.0.1:8787/', 'secret-in-memory'); return client;
}

test('history requests carry authorization only in the header and use bounded pagination', async t => {
  const calls = []; const client = setup(t, async (url, options) => { calls.push([url, options]); return json({ sessions: [], next_offset: null, unreadable: 0 }); });
  await client.list({ offset: 50, limit: 10, q: '大学生' });
  assert.match(calls[0][0], /\/api\/sessions\?offset=50&limit=10&q=/);
  assert.equal(calls[0][1].headers.Authorization, 'Bearer secret-in-memory');
  assert.equal(calls[0][1].credentials, 'omit'); assert.equal(calls[0][1].redirect, 'error'); assert.equal(calls[0][1].cache, 'no-store');
  assert.ok(!calls[0][0].includes('secret-in-memory'));
});
test('continuation sends only prompt, trusted identity and revision; no browser transcript', async t => {
  let received; const client = setup(t, async (_, options) => { received = JSON.parse(options.body); return json({ run_id: 'run' }); });
  const body = { request_id: 'turn-1', prompt: 'continue', revision: 7 };
  await client.start('s-test', body); assert.deepEqual(received, body);
});
test('HTTP storage errors preserve status and never retry a write', async t => {
  let calls = 0; const client = setup(t, async () => { calls++; return json({ message: 'disk failed' }, 500); });
  await assert.rejects(client.create('one'), e => e.status === 500 && e.message === 'disk failed'); assert.equal(calls, 1);
});
test('switching connections rejects a late response from the previous authority', async t => {
  let release; const client = setup(t, () => new Promise(resolve => { release = resolve; }));
  const pending = client.list(); client.configure('http://127.0.0.1:8790', 'new-secret');
  release(json({ sessions: [{ id: 'old-authority' }] }));
  await assert.rejects(pending, e => e.name === 'AbortError');
});
test('invalid session identifiers never reach fetch', async t => {
  let calls = 0; const client = setup(t, async () => { calls++; return json({}); });
  assert.throws(() => client.get('../secrets')); assert.throws(() => client.turn('s-ok', 'bad/path')); assert.equal(calls, 0);
});
test('oversized and malformed history responses are rejected', async t => {
  const client = setup(t, async () => new Response('{}', { headers: { 'content-length': String(17 * 1024 * 1024) } }));
  await assert.rejects(client.list(), /过大/);
  globalThis.fetch = async () => new Response('not json'); await assert.rejects(client.list(), /无效/);
});
test('rename and delete use revisions and deletion accepts an empty 204 response', async t => {
  const sent = []; const client = setup(t, async (url, options) => { sent.push([url, JSON.parse(options.body)]); return new Response(null, { status: 204 }); });
  await client.rename('s-one', 4, 'new title'); await client.delete('s-one', 5);
  assert.deepEqual(sent.map(([url, body]) => [new URL(url).pathname, body]), [
    ['/api/sessions/s-one/rename', { revision: 4, title: 'new title' }], ['/api/sessions/s-one/delete', { revision: 5 }],
  ]);
});
test('pin and archive toggle host flags and the archived view is an explicit request', async t => {
  const sent = []; const client = setup(t, async (url, options) => {
    sent.push([new URL(url).pathname + new URL(url).search, options.body === undefined ? null : JSON.parse(options.body)]);
    return json({ id: 's-one', pinned: true });
  });
  await client.pin('s-one', 3, true); await client.archive('s-one', 4, false);
  await client.unread('s-one', 5, true); await client.list({ archived: true });
  assert.deepEqual(sent.slice(0, 3), [
    ['/api/sessions/s-one/pin', { revision: 3, value: true }], ['/api/sessions/s-one/archive', { revision: 4, value: false }],
    ['/api/sessions/s-one/unread', { revision: 5, value: true }],
  ]);
  assert.equal(sent[3][0], '/api/sessions?offset=0&limit=50&q=&archived=true');
});
