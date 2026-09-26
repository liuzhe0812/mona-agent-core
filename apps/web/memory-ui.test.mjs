import test from 'node:test';
import assert from 'node:assert/strict';
import { MemoryUI } from './memory-ui.mjs';

// State/ownership tests only. Real browser layout and interaction are verified separately.
function fixture() {
  const previous = globalThis.document;
  const nodes = new Map();
  const element = () => ({ value: '', textContent: '', disabled: false, hidden: false, open: false, children: [],
    replaceChildren(...children) { this.children = children; },
    close() { this.open = false; },
  });
  globalThis.document = { querySelector(selector) {
    if (!nodes.has(selector)) nodes.set(selector, element());
    return nodes.get(selector);
  } };
  const ui = Object.assign(Object.create(MemoryUI.prototype), {
    $: selector => document.querySelector(selector),
    generation: 1, queryGeneration: 0, readGeneration: 0,
    hooks: { session: () => null }, api: { clear() {} }, render() {},
    state: { history_enabled: true, scopes: [] }, saving: null, searching: null,
  });
  return { ui, get: s => document.querySelector(s), restore: () => { globalThis.document = previous; } };
}
function deferred() { let resolve; const promise = new Promise(r => { resolve = r; }); return { promise, resolve }; }

test('old save completion cannot unlock or render a new authority mutation', async () => {
  const f = fixture();
  try {
    const response = deferred(); f.ui.api.update = () => response.promise;
    f.get('#memory-text').value = 'old fact';
    f.ui.editor = { scope: 'personal', revision: 'a'.repeat(64), generation: 1 };
    const pending = f.ui.save(); assert.ok(f.ui.saving);
    f.ui.clear();
    const newOperation = {}; f.ui.saving = newOperation;
    f.get('#memory-text').value = 'new authority draft'; f.get('#memory-save').disabled = true;
    response.resolve({}); await pending;
    assert.equal(f.ui.saving, newOperation);
    assert.equal(f.get('#memory-save').disabled, true);
    assert.equal(f.get('#memory-text').value, 'new authority draft');
    assert.equal(f.get('#memory-notice').textContent, '');
  } finally { f.restore(); }
});

test('old search cleanup cannot release a newer search after reconnect', async () => {
  const f = fixture();
  try {
    const old = deferred(), current = deferred(); let count = 0;
    f.ui.api.search = () => (++count === 1 ? old.promise : current.promise);
    f.get('#history-query').value = 'old'; const pending = f.ui.search();
    f.ui.clear(); f.ui.state = { history_enabled: true, scopes: [] };
    f.get('#history-query').value = 'current'; const next = f.ui.search();
    const currentOperation = f.ui.searching;
    old.resolve({ hits: [], next_offset: 8 }); await pending;
    assert.equal(f.ui.searching, currentOperation);
    assert.equal(f.get('#history-search-button').disabled, true);
    assert.equal(f.get('#history-more').hidden, true);
    current.resolve({ hits: [], next_offset: null }); await next;
    assert.equal(f.ui.searching, null);
    assert.equal(f.ui.lastQuery, 'current');
    assert.equal(f.get('#history-search-button').disabled, false);
  } finally { f.restore(); }
});

test('reset invalidates original-read locators and clears retained private text', async () => {
  const f = fixture();
  try {
    const response = deferred(); f.ui.api.read = () => response.promise;
    f.get('#history-read-text').textContent = 'old private content';
    f.ui.readLocator = { session_id: 'old' }; f.ui.nextRead = 100;
    const pending = f.ui.read({ session_id: 'old', revision: 1, message_index: 0, message_hash: 'b'.repeat(64) });
    f.ui.clear();
    response.resolve({ text: 'must not reappear', message_hash: 'b'.repeat(64) }); await pending;
    assert.equal(f.get('#history-read-text').textContent, '');
    assert.equal(f.ui.readLocator, null); assert.equal(f.ui.nextRead, null);
    assert.equal(f.get('#history-original').hidden, true);
  } finally { f.restore(); }
});
