import test from 'node:test';
import assert from 'node:assert/strict';
import { MemoryClient, validateMemoryView } from './memory.mjs';
test('memory requests keep credentials in headers and never use cookies, redirects or persistent storage', async () => {
  const previous = globalThis.fetch, client = new MemoryClient();
  try {
    client.configure('https://host.example/', 'private-token');
    globalThis.fetch = async (url, options) => { assert.equal(url, 'https://host.example/api/memory/update'); assert.equal(options.headers.Authorization, 'Bearer private-token'); assert.equal(options.credentials, 'omit'); assert.equal(options.redirect, 'error'); assert.equal(options.cache, 'no-store'); return Response.json({ revision: 'a'.repeat(64) }); };
    assert.equal((await client.update({ scope: 'personal', operations: [] })).revision, 'a'.repeat(64));
  } finally { client.clear(); globalThis.fetch = previous; }
});
test('old connection replies cannot populate a new memory authority and write conflicts remain errors', async () => {
  const previous = globalThis.fetch, client = new MemoryClient(); let respond;
  try {
    client.configure('https://one', 'one'); globalThis.fetch = () => new Promise(r => respond = r); const pending = client.view();
    client.configure('https://two', 'two'); respond(Response.json({ enabled: true })); await assert.rejects(pending, e => e.name === 'AbortError');
    globalThis.fetch = async () => Response.json({ message: 'conflict' }, { status: 409 }); await assert.rejects(client.update({}), e => e.status === 409);
    client.clear(); await assert.rejects(client.search({ query: 'fact' }));
  } finally { client.clear(); globalThis.fetch = previous; }
});
test('memory transport bounds request and streamed response bodies without losing cancellation', async () => {
  const previous = globalThis.fetch, client = new MemoryClient();
  try {
    client.configure('https://host.example', 'token'); let requests = 0;
    globalThis.fetch = async () => { requests++; return Response.json({}); };
    await assert.rejects(client.update({ text: 'x'.repeat(65536) }), /大小上限/);
    assert.equal(requests, 0);
    globalThis.fetch = async () => new Response('x'.repeat(256 * 1024 + 1));
    await assert.rejects(client.view(), /读取上限/);
    let cancelled = false;
    globalThis.fetch = async () => new Response(new ReadableStream({ cancel() { cancelled = true; } }));
    const pending = client.view();
    await new Promise(r => setImmediate(r)); client.clear();
    await assert.rejects(pending, error => error.name === 'AbortError'); assert.ok(cancelled);
  } finally { client.clear(); globalThis.fetch = previous; }
});

test('memory view validates scopes and schema without guessing missing capability flags', () => {
  const view = { enabled: true, history_enabled: false, scopes: [{ name: 'personal', revision: 'f'.repeat(64), entries: [{ id: 'm-0', text: '<script>literal</script>' }] }] };
  assert.equal(validateMemoryView(view), view);
  assert.throws(() => validateMemoryView({})); assert.throws(() => validateMemoryView({ ...view, scopes: [{ ...view.scopes[0], name: '../../other' }] }));
});
