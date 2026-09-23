import test from 'node:test';
import assert from 'node:assert/strict';
import { CapabilitiesClient } from './capabilities.mjs';

test('capability requests use bearer headers without cookies or redirects', async () => {
  const previous = globalThis.fetch;
  const client = new CapabilitiesClient();
  try {
    globalThis.fetch = async (url, options) => {
      assert.equal(url, 'https://host.example/api/capabilities');
      assert.equal(options.method, 'GET');
      assert.equal(options.headers.Authorization, 'Bearer test-management-token');
      assert.equal(options.credentials, 'omit');
      assert.equal(options.redirect, 'error');
      assert.equal(options.cache, 'no-store');
      assert.equal(options.body, undefined);
      return Response.json({ revision: 3, components: [], tools: [] });
    };
    client.configure('https://host.example/', 'test-management-token');
    const result = await client.get();
    assert.equal(result.revision, 3);
  } finally {
    client.clear();
    globalThis.fetch = previous;
  }
});

test('updates a capability with an encoded id and revision guarded body', async () => {
  const previous = globalThis.fetch;
  const client = new CapabilitiesClient();
  try {
    globalThis.fetch = async (url, options) => {
      assert.equal(url, 'https://host.example/api/capabilities/model%2Fmanagement');
      assert.equal(options.method, 'PUT');
      assert.equal(options.headers.Authorization, 'Bearer test-management-token');
      assert.equal(options.headers['Content-Type'], 'application/json');
      assert.equal(options.credentials, 'omit');
      assert.equal(options.redirect, 'error');
      assert.deepEqual(JSON.parse(options.body), { enabled: false, revision: 8 });
      return Response.json({ revision: 9, components: [], tools: [] });
    };
    client.configure('https://host.example', 'test-management-token');
    const result = await client.update('model/management', { enabled: false, revision: 8 });
    assert.equal(result.revision, 9);
  } finally {
    client.clear();
    globalThis.fetch = previous;
  }
});

test('returns structured HTTP errors and rejects requests without a connection', async () => {
  const previous = globalThis.fetch;
  const client = new CapabilitiesClient();
  let calls = 0;
  try {
    await assert.rejects(client.get(), /Agent 设置需要先连接 HTTP Runtime/);
    globalThis.fetch = async () => {
      calls += 1;
      return Response.json({ code: 'capability_conflict', message: 'refresh capabilities first' }, { status: 409 });
    };
    client.configure('https://host.example', 'test-management-token');
    await assert.rejects(client.update('skills', { enabled: true, revision: 2 }), error => (
      error.status === 409
      && error.code === 'capability_conflict'
      && error.message === 'refresh capabilities first'
      && error.payload.code === 'capability_conflict'
    ));
    assert.equal(calls, 1);
  } finally {
    client.clear();
    globalThis.fetch = previous;
  }
});

test('rejects malformed update input before issuing a request', async () => {
  const previous = globalThis.fetch;
  const client = new CapabilitiesClient();
  let calls = 0;
  try {
    globalThis.fetch = async () => { calls += 1; return Response.json({}); };
    client.configure('https://host.example', 'test-management-token');
    assert.throws(() => client.update('', { enabled: true, revision: 0 }), /设置项 ID 不能为空/);
    assert.throws(() => client.update('skills', { enabled: 'true', revision: 0 }), /启用状态必须是布尔值/);
    assert.throws(() => client.update('skills', { enabled: true, revision: -1 }), /设置版本无效/);
    assert.equal(calls, 0);
  } finally {
    client.clear();
    globalThis.fetch = previous;
  }
});
