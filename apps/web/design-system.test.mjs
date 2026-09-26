import test from 'node:test';
import assert from 'node:assert/strict';
import { auditDesignSystem, auditFeatureCss, auditHtml, auditRuntimeJs } from '../../scripts/check-web-design.mjs';

test('the formal Web UI satisfies the Mona design-system gate', () => {
  const result = auditDesignSystem();
  assert.deepEqual(result.violations, []);
  assert.ok(result.requiredTokens >= 20);
});

test('the gate rejects raw colors, arbitrary type, radius and motion in feature CSS', () => {
  const source = `.bad { color: #fff; font-size: 17px; font-weight: 700; border-radius: 7px; transition: all 170ms; box-shadow: 0 2px 8px #0003; }`;
  const messages = auditFeatureCss(source).map(item => item.message);
  assert.ok(messages.length >= 6);
  assert.ok(messages.some(message => message.includes('color tokens')));
  assert.ok(messages.some(message => message.includes('typography')));
  assert.ok(messages.some(message => message.includes('weight ladder')));
  assert.ok(messages.some(message => message.includes('radius tiers')));
  assert.ok(messages.some(message => message.includes('motion tokens')));
  assert.ok(messages.some(message => message.includes('shadows')));
});

test('the gate rejects unregistered runtime inline styles and accepts the bounded layout hooks', () => {
  assert.ok(auditRuntimeJs("node.style.background = '#fff';", 'apps/web/app.mjs')
    .some(item => item.message.includes('inline styles')));
  const sidebarHook = "if (this.shell.style.getPropertyValue('--sidebar-width') !== width) this.shell.style.setProperty('--sidebar-width', width);";
  assert.deepEqual(auditRuntimeJs(sidebarHook, 'apps/web/shell-layout.mjs'), []);
  assert.ok(auditRuntimeJs(sidebarHook, 'apps/web/app.mjs').length > 0);
  assert.deepEqual(auditRuntimeJs("if (this.composer.style.getPropertyValue('--composer-draft-height') !== desired) this.composer.style.setProperty('--composer-draft-height', desired);", 'apps/web/shell-layout.mjs'), []);
  assert.deepEqual(auditRuntimeJs("menu.style.left = `${x}px`;", 'apps/web/sessions-ui.mjs'), []);
});

test('the gate accepts shared tokens and rejects duplicate dialog dismissal chrome', () => {
  assert.deepEqual(auditFeatureCss(`.ok { color: var(--text-primary); font-size: var(--ui-text-13); font-weight: var(--ui-weight-medium); border-radius: var(--ui-radius-inner); transition: color var(--motion-hover); box-shadow: 0 0 0 2px var(--focus-ring-soft); }`), []);
  const duplicate = `<dialog id="provider-dialog"><div class="dialog-head"><button aria-label="关闭"></button></div><div class="dialog-actions"><button value="cancel">取消</button></div></dialog>`
    + `<dialog id="model-dialog"></dialog><dialog id="model-picker-dialog"></dialog><dialog id="connection-dialog"></dialog>`;
  assert.ok(auditHtml(duplicate).some(item => item.message.includes('top-right close')));
});
