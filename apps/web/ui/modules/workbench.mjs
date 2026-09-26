import { WorkbenchUI } from '../../workbench-ui.mjs';
import { claim } from '../dom.mjs';
export const version = 1;
export function mount(ctx) {
  const ui = new WorkbenchUI({ pane: ctx.shell.pane(ctx.id), workspace: ctx.service('workspace'), header: claim('header-terminal') });
  ctx.own(() => ui.dispose());
  ctx.on('workspace-ready', () => ui.renderHeader());
  ctx.on('session', () => ui.renderHeader());
  ctx.on('connection', () => ui.renderHeader());
  // Actions and tabs are owned by this module and released by the shell, not a second tab manager.
}
