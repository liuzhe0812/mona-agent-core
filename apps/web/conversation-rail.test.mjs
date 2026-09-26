import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { previewText, railVisualState } from './conversation-rail.mjs';
import { catalog } from './ui/catalog.mjs';

test('turn navigator builds the same peak, near and mid interaction curve', () => {
  assert.deepEqual(railVisualState(4), { tone: 'idle', opacity: .58, scaleX: 1 });
  assert.deepEqual(railVisualState(4, 4), { tone: 'peak', opacity: 1, scaleX: 2.6 });
  assert.deepEqual(railVisualState(3, 4), { tone: 'near', opacity: .86, scaleX: 1.7 });
  assert.deepEqual(railVisualState(2, 4), { tone: 'mid', opacity: .72, scaleX: 1.25 });
  assert.deepEqual(railVisualState(1, 4), { tone: 'idle', opacity: .58, scaleX: 1 });
});

test('turn navigator previews normalize whitespace, keep two paragraphs and cap at 220 characters', () => {
  assert.equal(previewText('  第一段  多余空格\n\n第二段\n\n第三段', '回退'), '第一段 多余空格\n第二段');
  const long = previewText('a'.repeat(260), '回退');
  assert.equal(long.length, 220);
  assert.ok(long.endsWith('...'));
  assert.equal(previewText('', '回退'), '回退');
});

test('turn navigator is owned by the local UI module lifecycle', async () => {
  assert.equal(catalog['conversation-rail'].local, true);
  const module = await catalog['conversation-rail'].load();
  assert.equal(module.version, 1);
  assert.equal(typeof module.mount, 'function');
  const app = await readFile(new URL('./app.mjs', import.meta.url), 'utf8');
  assert.doesNotMatch(app, /import\s+['"]\.\/conversation-rail\.mjs['"]/);
});
