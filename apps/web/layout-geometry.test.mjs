import test from 'node:test';
import assert from 'node:assert/strict';
import { resolveColumns, draftHeight, CENTER_MIN_WIDTH, clamp } from './layout-geometry.mjs';
import { setBaseInert, setShellOverlay } from './overlay-scope.mjs';

test('right track shrinks before the center floor and overlays only when its minimum cannot fit', () => {
  assert.deepEqual(resolveColumns({ width: 1100, left: 270, right: 700 }), { right: 424, center: 400, overlay: false, maximum: 424 });
  const narrow = resolveColumns({ width: 900, left: 270, right: 700 });
  assert.equal(narrow.overlay, true); assert.equal(narrow.center, 630);
  assert.equal(resolveColumns({ width: 990, left: 264, right: 700 }).overlay, false);
  assert.equal(resolveColumns({ width: 989, left: 264, right: 700 }).overlay, true);
});

test('responsive fitting does not mutate width preferences and restores them on wider frames', () => {
  const requested = Object.freeze({ width: 1440, left: 466, right: 700 });
  const smaller = resolveColumns(requested), larger = resolveColumns({ ...requested, width: 1600 });
  assert.equal(smaller.right, 568); assert.equal(larger.right, 700); assert.equal(requested.right, 700);
});

test('all bounded column combinations retain nonnegative geometry without overflow', () => {
  for (let width = 0; width <= 3000; width += 29) for (const left of [0, 214, 270, 466]) for (const right of [0, 320, 420, 700, 10000, NaN]) {
    const c = resolveColumns({ width, left, right });
    assert.ok(c.center >= 0 && c.right >= 320 && c.right <= 720);
    assert.ok(Number.isFinite(c.center) && Number.isFinite(c.right));
    if (!c.overlay) { assert.ok(c.center >= CENTER_MIN_WIDTH); assert.ok(c.center + c.right + Math.min(left, width) + 6 <= width + 0.001); }
  }
  assert.equal(resolveColumns({ width: NaN }).center, 0);
  assert.equal(clamp(Infinity, 208, 460), 208);
});

test('draft auto-sizing reserves reading space on short screens and caps long text', () => {
  const base = { workspace: 900, header: 48, chrome: 110 };
  assert.equal(draftHeight({ ...base, natural: 24 }), 40);
  assert.equal(draftHeight({ ...base, natural: 180 }), 180);
  assert.equal(draftHeight({ ...base, natural: 50000 }), 220);
  assert.equal(draftHeight({ ...base, workspace: 320, natural: 50000 }), 66);
  assert.equal(draftHeight({ ...base, workspace: 240, natural: 50000 }), 40);
});

function fakeShell() {
  const shell = { children: [] };
  for (const name of ['navigation', 'conversation', 'right', 'scrim']) shell[name] = { parentElement: shell, inert: false };
  shell.children = [shell.navigation, shell.conversation, shell.right, shell.scrim]; return shell;
}
test('closing one modal cannot undo another owner or restore outdated navigation state', () => {
  const shell = fakeShell(), nav = {}, right = {};
  setShellOverlay(shell, nav, [shell.navigation, shell.scrim], true);
  assert.equal(shell.conversation.inert, true); assert.equal(shell.navigation.inert, false);
  setShellOverlay(shell, right, [shell.right, shell.scrim], true);
  assert.equal(shell.navigation.inert, true); assert.equal(shell.right.inert, false);
  setBaseInert(shell, shell.navigation, true);
  setShellOverlay(shell, nav, [], false);
  assert.equal(shell.conversation.inert, true); assert.equal(shell.right.inert, false);
  setShellOverlay(shell, right, [], false);
  assert.equal(shell.conversation.inert, false); assert.equal(shell.navigation.inert, true);
  setBaseInert(shell, shell.navigation, false); assert.equal(shell.navigation.inert, false);
});

test('overlay ownership captures newly mounted shell surfaces and drops removed ones', () => {
  const shell = fakeShell(), owner = {};
  setShellOverlay(shell, owner, [shell.right], true);
  const later = { parentElement: shell, inert: false }; shell.children.push(later);
  setShellOverlay(shell, owner, [shell.right], true); assert.equal(later.inert, true);
  shell.children = shell.children.filter(n => n !== shell.navigation); shell.navigation.parentElement = null;
  setShellOverlay(shell, owner, [], false); assert.equal(later.inert, false); assert.equal(shell.conversation.inert, false);
});
