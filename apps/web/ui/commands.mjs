// Product commands are trusted contributions, not instructions obtained from model output.
export const COMMAND_SECTIONS = Object.freeze({ add: '添加', command: '指令' });
const valueOf = value => typeof value === 'function' ? value() : value;

export function commandState(value) {
  const menu = value?.menu;
  return { visible: !menu || valueOf(menu.visible) !== false, disabled: Boolean(menu && valueOf(menu.disabled)) };
}

export function commandGroups(entries, onError = () => {}) {
  const groups = new Map(Object.keys(COMMAND_SECTIONS).map(id => [id, []]));
  for (const entry of entries) {
    try {
      const { value } = entry;
      if (typeof value === 'function' || value?.menu == null) continue;
      const menu = value.menu;
      if (typeof value.run !== 'function' || !groups.has(menu.section)
          || typeof menu.label !== 'string' || !menu.label.trim() || menu.label.length > 80
          || (menu.description != null && (typeof menu.description !== 'string' || menu.description.length > 240))
          || (menu.icon != null && typeof menu.icon !== 'function')
          || (menu.select != null && typeof menu.select !== 'function')) throw Error(`命令 ${entry.id} 的菜单声明无效。`);
      const state = commandState(value);
      if (state.visible) groups.get(menu.section).push({ entry, disabled: state.disabled });
    } catch (error) { onError(error); }
  }
  return [...groups].filter(([, rows]) => rows.length).map(([id, rows]) => ({ id, label: COMMAND_SECTIONS[id], rows }));
}

export async function runCommand(value, argumentsText = '', fromMenu = false) {
  const { visible, disabled } = commandState(value);
  if (!visible || disabled) throw Error('该操作当前不可用，请等待任务结束或重新连接。');
  const run = fromMenu && value?.menu?.select ? value.menu.select : typeof value === 'function' ? value : value?.run;
  if (typeof run !== 'function') throw Error('命令当前不可执行。');
  return await run(argumentsText);
}
