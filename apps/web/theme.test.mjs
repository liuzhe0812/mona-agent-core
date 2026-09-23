import test from 'node:test';
import assert from 'node:assert/strict';
import { SKINS, PALETTE_KEYS, ThemeManager, DEFAULT_APPEARANCE, THEME_STORAGE_KEY, MAX_THEME_BYTES, contrast, validatePalette, parseSkin, normalizeAppearance } from './theme.mjs';

class Storage {
  data = new Map(); writes = 0; fail = false;
  getItem(key) { return this.data.get(key) ?? null; }
  setItem(key, value) { if (this.fail) throw new Error('QuotaExceededError'); this.data.set(key, value); this.writes++; }
}
class Media extends EventTarget {
  matches = false;
  set(value) { this.matches = value; this.dispatchEvent(new Event('change')); }
}
function setup(storage = new Storage()) {
  const properties = new Map();
  const root = { dataset: { untouched: 'task' }, style: { setProperty: (key, value) => properties.set(key, value) } };
  const media = new Media(), events = new EventTarget();
  const manager = new ThemeManager({ root, storage: () => storage, media, events });
  return { manager, root, properties, storage, media, events };
}
function portable(item = SKINS[1]) { return { format: 'mona-theme', version: 1, name: item.name, light: { ...item.light }, dark: { ...item.dark } }; }
function storageEvent(events, storage, key = THEME_STORAGE_KEY, newValue = 'stale queued value') {
  const event = new Event('storage'); Object.assign(event, { key, newValue, storageArea: storage }); events.dispatchEvent(event);
}

test('all built-in day/night palettes have the complete token contract and readable text', () => {
  assert.equal(SKINS.length, 6); assert.equal(new Set(SKINS.map(item => item.id)).size, 6);
  for (const item of SKINS) for (const mode of ['light', 'dark']) {
    assert.deepEqual(Object.keys(validatePalette(item[mode])), PALETTE_KEYS, `${item.id}/${mode}`);
  }
  assert.equal(contrast('#ffffff', '#000000'), 21);
  assert.equal(contrast('#123456', '#123456'), 1);
});

test('a fresh browser uses the neutral default without writing to storage', () => {
  const { manager, root, storage } = setup();
  assert.deepEqual(manager.value, DEFAULT_APPEARANCE);
  assert.equal(root.dataset.theme, 'light'); assert.equal(root.dataset.skin, 'graphite');
  assert.equal(storage.writes, 0); assert.equal(root.dataset.untouched, 'task');
});

test('selected palette, mode, font size and shape survive a new manager instance', () => {
  const { manager, storage } = setup();
  assert.equal(manager.update({ skin: 'ocean', mode: 'dark', fontSize: 18, shape: 'soft' }), true);
  const restored = setup(storage);
  assert.deepEqual(restored.manager.value, manager.value);
  assert.equal(restored.properties.get('--bg'), SKINS[1].dark.bg);
  assert.equal(restored.properties.get('--chat-font-size'), '18px');
  assert.equal(restored.properties.get('--skin-composer-radius'), '18px');
  assert.equal(restored.properties.get('--skin-bubble-radius'), '16px');
  assert.equal(restored.properties.get('--skin-panel-radius'), '16px');
  assert.equal(restored.properties.get('--skin-detail-radius'), '12px');
  const detached = manager.value; detached.skin = 'rose'; assert.equal(manager.value.skin, 'ocean');
});

test('a failed storage write preserves both selected data and visible styles', () => {
  const { manager, storage, root, properties } = setup();
  manager.update({ skin: 'forest' }); const previous = manager.value; const bg = properties.get('--bg');
  storage.fail = true;
  assert.equal(manager.update({ mode: 'dark', skin: 'iris' }), false);
  assert.deepEqual(manager.value, previous); assert.equal(properties.get('--bg'), bg);
  assert.equal(root.dataset.theme, 'light'); assert.match(manager.notice, /保存失败/);
  assert.equal(manager.failed, true);
  storage.fail = false; assert.equal(manager.update({ mode: 'dark' }), true); assert.equal(manager.failed, false);
});

test('localStorage getter denial is nonfatal at bootstrap and never pretends to save', () => {
  const manager = new ThemeManager({ storage() { throw new Error('SecurityError'); } });
  assert.deepEqual(manager.value, DEFAULT_APPEARANCE); assert.equal(manager.failed, true);
  assert.equal(manager.update({ skin: 'rose' }), false); assert.equal(manager.value.skin, 'graphite');
});

test('corrupt, unknown-version or missing-skin stored settings fall back without overwriting data', () => {
  for (const source of ['{bad json', JSON.stringify({ ...DEFAULT_APPEARANCE, version: 2 }), JSON.stringify({ ...DEFAULT_APPEARANCE, skin: 'missing' })]) {
    const storage = new Storage(); storage.data.set(THEME_STORAGE_KEY, source);
    const { manager } = setup(storage);
    assert.deepEqual(manager.value, DEFAULT_APPEARANCE); assert.equal(manager.failed, true);
    assert.equal(storage.getItem(THEME_STORAGE_KEY), source); assert.equal(storage.writes, 0);
    assert.equal(manager.reset(), true); assert.deepEqual(JSON.parse(storage.getItem(THEME_STORAGE_KEY)), DEFAULT_APPEARANCE);
  }
});

test('follow-system reacts only while selected; a manual toggle preserves the skin', () => {
  const { manager, root, media, storage } = setup();
  manager.update({ skin: 'sand', mode: 'system' }); const writes = storage.writes;
  media.set(true); assert.equal(root.dataset.theme, 'dark'); assert.equal(storage.writes, writes);
  assert.equal(manager.value.mode, 'system');
  manager.toggleMode(); assert.equal(root.dataset.theme, 'light'); assert.equal(manager.value.mode, 'light');
  media.set(false); media.set(true); assert.equal(root.dataset.theme, 'light'); assert.equal(root.dataset.skin, 'sand');
});

test('export/import roundtrip is palette-only and works for every built-in skin', () => {
  const { manager } = setup();
  for (const item of SKINS) {
    manager.update({ skin: item.id }); const text = manager.exportSkin();
    assert.deepEqual(parseSkin(text), portable(item)); assert.ok(Buffer.byteLength(text) < MAX_THEME_BYTES);
    assert.deepEqual(Object.keys(JSON.parse(text)), ['format', 'version', 'name', 'light', 'dark']);
  }
});

test('imported skin is persisted, supports both modes, and reset retains the custom slot', () => {
  const { manager, storage, properties } = setup();
  const custom = portable(); custom.name = ' 我的主题 ';
  assert.equal(manager.importSkin(JSON.stringify(custom)), true);
  assert.equal(manager.value.skin, 'custom'); assert.equal(manager.value.custom.name, '我的主题');
  manager.update({ mode: 'dark' }); assert.equal(properties.get('--bg'), custom.dark.bg);
  assert.deepEqual(setup(storage).manager.value, manager.value);
  assert.equal(manager.reset(), true); assert.equal(manager.value.skin, 'graphite');
  assert.equal(manager.value.mode, 'light'); assert.equal(manager.value.custom.name, '我的主题');
});

test('untrusted skins reject CSS, URLs, unknown fields, prototype keys and malformed data', () => {
  const variants = [
    value => { value.light.bg = 'url(https://invalid.example/leak)'; },
    value => { value.dark.accent = '#ffffff; display:none'; },
    value => { value.light.extra = '#ffffff'; },
    value => { delete value.light.text; },
    value => { value.script = 'alert(1)'; },
    value => { value.version = 2; },
    value => { value.name = '\u202ehidden'; },
    value => { value.dark = []; },
    value => { Object.defineProperty(value.light, '__proto__', { value: '#ffffff', enumerable: true }); },
  ];
  const { manager, storage } = setup(); manager.update({ skin: 'forest' }); const original = manager.value, writes = storage.writes;
  for (const mutate of variants) {
    const value = portable(); mutate(value);
    assert.equal(manager.importSkin(JSON.stringify(value)), false);
    assert.deepEqual(manager.value, original); assert.equal(storage.writes, writes);
  }
  assert.equal(manager.importSkin('['), false);
  assert.equal(manager.importSkin(' '.repeat(MAX_THEME_BYTES + 1)), false);
  assert.equal(manager.importSkin('汉'.repeat(MAX_THEME_BYTES / 2)), false);
  assert.equal({}.polluted, undefined);
});

test('unreadable text, state, focus or button colors cannot be installed', () => {
  for (const change of [value => { value.light.text = value.light.bg; }, value => { value.dark.muted = value.dark.panel; },
    value => { value.light.danger = value.light.panel; }, value => { value.dark.focus = value.dark.bg; },
    value => { value.light.inverse = value.light.accent; }]) {
    const value = portable(); change(value); assert.throws(() => parseSkin(JSON.stringify(value)), /对比度/);
  }
});

test('appearance preferences reject non-string enums and arbitrary keys', () => {
  for (const patch of [{ mode: ['light'] }, { shape: ['soft'] }, { fontSize: '15' }, { fontSize: 99 }, { skin: 'custom' },
    { mode: 'auto' }, { custom: {} }, { endpoint: 'https://invalid.example' }]) {
    assert.throws(() => normalizeAppearance({ ...DEFAULT_APPEARANCE, ...patch }));
  }
});

test('cross-tab sync reads the latest durable value, ignores other storage and preserves state on invalid data', () => {
  const { manager, storage, events, root } = setup();
  const next = { ...DEFAULT_APPEARANCE, skin: 'iris', mode: 'dark' };
  storage.setItem(THEME_STORAGE_KEY, JSON.stringify(next)); const writes = storage.writes;
  storageEvent(events, storage);
  assert.deepEqual(manager.value, next); assert.equal(storage.writes, writes); assert.equal(root.dataset.theme, 'dark');
  storage.data.set(THEME_STORAGE_KEY, '{bad'); storageEvent(events, storage);
  assert.deepEqual(manager.value, next); assert.equal(manager.failed, true);
  storage.data.delete(THEME_STORAGE_KEY); storageEvent(events, new Storage()); assert.deepEqual(manager.value, next);
  storageEvent(events, storage, null); assert.deepEqual(manager.value, DEFAULT_APPEARANCE);
});

test('subscriptions can be removed and disposal detaches system and storage listeners', () => {
  const { manager, media, root } = setup(); let notifications = 0;
  const unsubscribe = manager.subscribe(() => notifications++); assert.equal(notifications, 1);
  manager.update({ mode: 'system' }); assert.equal(notifications, 2);
  unsubscribe(); media.set(true); assert.equal(notifications, 2); assert.equal(root.dataset.theme, 'dark');
  manager.dispose(); media.set(false); assert.equal(root.dataset.theme, 'dark');
});
