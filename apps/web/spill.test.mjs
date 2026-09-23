import test from 'node:test';
import assert from 'node:assert/strict';
import { SpillClient } from './spill.mjs';

test('spill client sends only opaque ids and bearer authorization', async () => {
  const previousFetch = globalThis.fetch;
  let request;
  globalThis.fetch = async (url, options) => {
    request = { url, options };
    return new Response(JSON.stringify({ id: 'sp_abc', offset: 0, next_offset: 3, total_bytes: 3, eof: true, text: 'abc' }), {
      status: 200, headers: { 'content-type': 'application/json' },
    });
  };
  try {
    const client = new SpillClient();
    client.configure('http://127.0.0.1:8787/', 'secret');
    assert.equal((await client.readPage('run-1', 'spill:sp_abc', 0, 16)).text, 'abc');
    assert.equal(request.url, 'http://127.0.0.1:8787/api/spill/run-1/sp_abc?offset=0&limit=16');
    assert.equal(request.options.headers.Authorization, 'Bearer secret');
    await assert.rejects(() => client.readPage('run-1', 'file:C:/secret'));
  } finally { globalThis.fetch = previousFetch; }
});

