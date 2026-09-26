import test from 'node:test';
import assert from 'node:assert/strict';
import { validateView, modes } from './modules/sandbox.mjs';
const base = () => ({ version: 1, session_id: null, revision: null, status: 'idle', enabled: true, locked: false, mode: 'workspace-write', backend: { backend: 'windows-acl', enforcement: 'partial' }, unavailable: null });
test('sandbox view exposes exact modes and does not invent full enforcement', () => {
  assert.deepEqual(Object.keys(modes), ['read-only', 'workspace-write', 'danger-full-access']);
  assert.equal(validateView(base()).backend.enforcement, 'partial');
  const value = { ...base(), session_id: 'a', revision: 3, locked: true, mode: 'read-only' };
  assert.equal(validateView(value, 'a').locked, true);
  assert.throws(() => validateView(value, 'b'));
});
test('sandbox state rejects malformed policy, missing facts and authority confusion', () => {
  for (const patch of [{ version: 2 }, { mode: 'toString' }, { mode: 'read_only' }, { revision: 1 }, { status: 'approved' }, { locked: undefined }, { unavailable: {} }, { backend: { backend: 'docker', enforcement: 'full' } }]) {
    assert.throws(() => validateView({ ...base(), ...patch }));
  }
  assert.throws(() => validateView({ ...base(), mode: 'danger-full-access' }));
  assert.throws(() => validateView({ ...base(), enabled: false }));
  const unavailable = { ...base(), backend: null, unavailable: 'SANDBOX_UNAVAILABLE: missing backend' };
  assert.equal(validateView(unavailable).unavailable, unavailable.unavailable);
  assert.equal(validateView({ ...base(), enabled: false, backend: null }).enabled, false);
});
