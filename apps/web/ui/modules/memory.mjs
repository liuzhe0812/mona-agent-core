import { MemoryUI } from '../../memory-ui.mjs';
import { claim, scoped, setting } from '../dom.mjs';
export const version = 1;
export function mount(ctx) {
  const roots = ['memory-settings-section', 'memory-dialog'].map(claim);
  const ui = new MemoryUI({ query: scoped(roots), session: ctx.shell.session, open: ctx.shell.openSession });
  ctx.own(() => ui.dispose());
  setting(ctx, 'memory', roots[0], () => ui.refresh(), 40);
  ctx.on('connection', connection => { if (connection) ui.configure(connection.base, connection.bearer); else ui.clear(); });
}
