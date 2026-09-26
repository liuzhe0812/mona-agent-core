// Browser assertions against the actual mounted product menu, without issuing Agent tasks.
export async function commandMenuChecks(expectedIds) {
  const checks = [], check = (name, condition) => { if (!condition) throw Error(name); checks.push(name); };
  const wait = async predicate => { const end = performance.now() + 5000; while (performance.now() < end) { if (predicate()) return; await new Promise(r => setTimeout(r, 20)); } throw Error('command menu did not settle'); };
  const button = document.getElementById('composer-add'), root = document.getElementById('composer-command-menu');
  const prompt = document.getElementById('prompt'), original = prompt.value, start = prompt.selectionStart, end = prompt.selectionEnd;
  button.click(); await wait(() => root.matches(':popover-open') && root.querySelectorAll('[data-command]').length === expectedIds.length);
  const rows = [...root.querySelectorAll('[data-command]')];
  check('plus menu includes only registered active commands with names, aliases and descriptions', JSON.stringify(rows.map(n => n.dataset.command)) === JSON.stringify(expectedIds)
    && rows.every(n => n.querySelector('.command-label').textContent && n.querySelector('.command-alias').textContent === n.dataset.command && n.querySelector('.command-description').textContent && n.querySelector('.command-icon svg')));
  const box = root.getBoundingClientRect(), composer = document.querySelector('.composer').getBoundingClientRect();
  check('command overlay uses the full composer width and is above the card without clipping', Math.abs(box.width - composer.width) < 1 && Math.abs(box.left - composer.left) < 1 && box.bottom <= composer.top - 3 && root.matches(':popover-open'));
  check('command menu follows compact DSH heading, row and icon geometry', getComputedStyle(root).borderRadius === '16px'
    && rows.every(n => Math.round(n.getBoundingClientRect().height) === 34 && n.querySelector('.command-icon').getBoundingClientRect().width === 14));
  const expectedHeadings = expectedIds.includes('plan') ? ['添加', '指令'] : ['指令'];
  check('only nonempty add and command groups have headings', JSON.stringify([...root.querySelectorAll('.composer-command-heading')].map(n => n.textContent)) === JSON.stringify(expectedHeadings));
  rows[0].focus();
  rows[0].dispatchEvent(new KeyboardEvent('keydown', { key: 'End', bubbles: true, cancelable: true }));
  check('keyboard navigation reaches the final visible command', document.activeElement === rows.at(-1));
  document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
  check('Escape closes the menu and restores its opener without consuming the draft', root.hidden && !root.matches(':popover-open') && document.activeElement === button && prompt.value === original && prompt.selectionStart === start && prompt.selectionEnd === end);
  button.click(); await wait(() => root.matches(':popover-open'));
  prompt.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
  check('outside pointer closes only the command menu', root.hidden && prompt.value === original);
  button.click(); await wait(() => root.matches(':popover-open'));
  const row = root.querySelector('[data-command]');
  const { uiRegistry } = await import('../ui/registry.mjs'); uiRegistry.notify('composer.commands');
  check('unchanged module refresh preserves menu nodes instead of resetting focus and scroll', root.querySelector('[data-command]') === row);
  row.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
  return checks;
}
