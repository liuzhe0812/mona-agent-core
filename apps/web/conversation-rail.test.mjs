import test from 'node:test';
import assert from 'node:assert/strict';
import { previewText, railVisualState } from './conversation-rail.mjs';

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
