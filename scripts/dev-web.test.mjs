import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { parseDotEnv, resolveDevConfig, ensureDevStoreKey, startUiServer } from './dev-web.mjs';

test('parseDotEnv handles comments, quotes and export', () => {
  assert.deepEqual(parseDotEnv(`
# comment
export AGENT_MODEL_NAME=test-model
AGENT_MODEL_KEY="a b"
VALUE=plain # trailing
INVALID LINE
`), { AGENT_MODEL_NAME: 'test-model', AGENT_MODEL_KEY: 'a b', VALUE: 'plain' });
});

test('resolveDevConfig starts empty model management without dotenv and constrains loopback binds', () => {
  const empty = resolveDevConfig({ MONA_DEV_STATE_DIR: join(tmpdir(), 'mona-empty-config-test') });
  assert.equal(empty.modelManagement, true);
  assert.equal(empty.endpoint, 'http://127.0.0.1:8787');
  const config = resolveDevConfig({
    AGENT_SERVER_ADDR: '127.0.0.1:8787',
    AGENT_SERVER_TOKEN: 'x'.repeat(32),
    MONA_WEB_PORT: '4173',
  });
  assert.equal(config.endpoint, 'http://127.0.0.1:8787');
  assert.equal(config.uiOrigin, 'http://127.0.0.1:4173');
  assert.equal(config.capabilityStatePath, join(config.stateDirectory, 'capabilities.json'));
  assert.equal(config.spillDirectory, join(config.stateDirectory, 'spill'));
  assert.equal(config.sessionsDirectory, join(config.stateDirectory, 'sessions'));
  assert.equal(empty.sessionsDirectory, join(tmpdir(), 'mona-empty-config-test', 'sessions'));
  const sessionsDirectory = join(tmpdir(), 'explicit-session-store');
  assert.equal(resolveDevConfig({ AGENT_SESSIONS_DIR: sessionsDirectory }).sessionsDirectory, sessionsDirectory);
  assert.throws(() => resolveDevConfig({ AGENT_MODEL_MANAGEMENT: '0' }), /固定模型模式缺少/);
  assert.throws(() => resolveDevConfig({
    AGENT_SERVER_ADDR: '0.0.0.0:8787',
  }), /环回地址/);
});

test('development model-store key is generated once outside the repository', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'mona-dev-state-'));
  const first = await ensureDevStoreKey(directory);
  const second = await ensureDevStoreKey(directory);
  assert.equal(first, second);
  assert.ok(first.length >= 32);
  assert.equal((await readFile(join(directory, 'model-store.key'), 'utf8')).trim(), first);
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

    const app = await fetch(`http://127.0.0.1:${port}/apps/web/app.mjs`);
    assert.equal(app.headers.get('content-type'), 'text/javascript; charset=utf-8');
    assert.match(await app.text(), /HttpAgentClient/);

    const client = await fetch(`http://127.0.0.1:${port}/packages/client/src/index.mjs`);
    assert.equal(client.status, 200);

    const response = await fetch(`http://127.0.0.1:${port}/__mona_dev_config__.json`);
    assert.equal(response.headers.get('cache-control'), 'no-store');
    assert.deepEqual(await response.json(), {
      endpoint: 'http://127.0.0.1:8787',
      token: 'z'.repeat(32),
    });

    const traversal = await fetch(`http://127.0.0.1:${port}/apps/web/../Cargo.toml`);
    assert.equal(traversal.status, 404);
  } finally {
    await new Promise((resolveClose) => server.close(resolveClose));
  }
});
