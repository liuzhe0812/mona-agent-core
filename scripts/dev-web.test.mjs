import test from 'node:test';
import assert from 'node:assert/strict';
import { parseDotEnv, resolveDevConfig, startUiServer } from './dev-web.mjs';

test('parseDotEnv handles comments, quotes and export', () => {
  assert.deepEqual(parseDotEnv(`
# comment
export AGENT_MODEL_NAME=test-model
AGENT_MODEL_KEY="a b"
VALUE=plain # trailing
INVALID LINE
`), { AGENT_MODEL_NAME: 'test-model', AGENT_MODEL_KEY: 'a b', VALUE: 'plain' });
});

test('resolveDevConfig requires real model configuration and loopback binds', () => {
  assert.throws(() => resolveDevConfig({}), /AGENT_MODEL_ENDPOINT/);
  const config = resolveDevConfig({
    AGENT_MODEL_ENDPOINT: 'https://model.example/v1/chat/completions',
    AGENT_MODEL_NAME: 'model',
    AGENT_SERVER_ADDR: '127.0.0.1:8787',
    AGENT_SERVER_TOKEN: 'x'.repeat(32),
    MONA_WEB_PORT: '4173',
  });
  assert.equal(config.endpoint, 'http://127.0.0.1:8787');
  assert.equal(config.uiOrigin, 'http://127.0.0.1:4173');
  assert.throws(() => resolveDevConfig({
    AGENT_MODEL_ENDPOINT: 'https://model.example/v1/chat/completions',
    AGENT_MODEL_NAME: 'model',
    AGENT_SERVER_ADDR: '0.0.0.0:8787',
  }), /环回地址/);
});

test('UI server serves modular UI, clients and in-memory config', async () => {
  const server = await startUiServer({
    host: '127.0.0.1',
    port: 0,
    endpoint: 'http://127.0.0.1:8787',
    token: 'z'.repeat(32),
  });
  try {
    const { port } = server.address();
    const page = await fetch(`http://127.0.0.1:${port}/`);
    assert.equal(page.status, 200);
    assert.match(await page.text(), /Mona Agent/);

    const app = await fetch(`http://127.0.0.1:${port}/ui/app.mjs`);
    assert.equal(app.headers.get('content-type'), 'text/javascript; charset=utf-8');
    assert.match(await app.text(), /HttpAgentClient/);

    const client = await fetch(`http://127.0.0.1:${port}/clients/javascript/src/index.mjs`);
    assert.equal(client.status, 200);

    const response = await fetch(`http://127.0.0.1:${port}/__mona_dev_config__.json`);
    assert.equal(response.headers.get('cache-control'), 'no-store');
    assert.deepEqual(await response.json(), {
      endpoint: 'http://127.0.0.1:8787',
      token: 'z'.repeat(32),
    });

    const traversal = await fetch(`http://127.0.0.1:${port}/ui/../Cargo.toml`);
    assert.equal(traversal.status, 404);
  } finally {
    await new Promise((resolveClose) => server.close(resolveClose));
  }
});
