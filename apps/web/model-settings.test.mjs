import test from 'node:test';
import assert from 'node:assert/strict';
import { ModelSettingsClient, parseContextWindow } from './model-settings.mjs';

test('model capacity accepts unknown or positive bounded integers without guessing', () => {
  assert.equal(parseContextWindow(''), null); assert.equal(parseContextWindow(null), null);
  assert.equal(parseContextWindow(' 32768 '), 32768);
  for (const value of ['0', '-1', '1.5', 'Infinity', '8192 tokens', '1000000001']) assert.throws(() => parseContextWindow(value));
});

test('management requests use bearer headers without cookies or redirects', async () => {
  const previous = globalThis.fetch;
  const client = new ModelSettingsClient();
  try {
    globalThis.fetch = async (url, options) => {
      assert.equal(url, 'https://host.example/api/model-settings/providers');
      assert.equal(options.headers.Authorization, 'Bearer test-management-token');
      assert.equal(options.credentials, 'omit');
      assert.equal(options.redirect, 'error');
      assert.equal(options.cache, 'no-store');
      assert.equal(JSON.parse(options.body).api_key, 'new-test-provider-key');
      return Response.json({ revision: 1, providers: [], default: null });
    };
    client.configure('https://host.example/', 'test-management-token');
    const result = await client.saveProvider({ revision: 0, api_key: 'new-test-provider-key' });
    assert.equal(result.revision, 1);
  } finally { client.clear(); globalThis.fetch = previous; }
});

test('a late response from an old connection cannot populate the new host settings', async () => {
  const previous = globalThis.fetch;
  const client = new ModelSettingsClient();
  let respond;
  try {
    // Deliberately ignore abort to simulate a response already received by the browser.
    globalThis.fetch = () => new Promise(resolve => { respond = resolve; });
    client.configure('https://old.example', 'old-token');
    const pending = client.get();
    client.configure('https://new.example', 'new-token');
    respond(Response.json({ revision: 99, providers: [{ id: 'old-host' }] }));
    await assert.rejects(pending, error => error.name === 'AbortError');
  } finally { client.clear(); globalThis.fetch = previous; }
});

test('conflicts remain errors and disconnected settings never send a request', async () => {
  const previous = globalThis.fetch;
  const client = new ModelSettingsClient();
  let calls = 0;
  try {
    globalThis.fetch = async () => { calls++; return Response.json({ message: 'settings_conflict: refresh' }, { status: 409 }); };
    client.configure('https://host.example', 'test-token');
    await assert.rejects(client.setDefault({ revision: 1 }), error => error.status === 409);
    client.clear();
    await assert.rejects(client.get());
    assert.equal(calls, 1);
  } finally { client.clear(); globalThis.fetch = previous; }
});
