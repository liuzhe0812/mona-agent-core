import { SideConversationUI } from '../../side-conversation.mjs';
export const version = 1;
export function mount(ctx) {
  const workspace = ctx.service('workspace');
  const ui = new SideConversationUI({ pane: ctx.shell.pane(ctx.id), session: ctx.shell.session, client: ctx.shell.client,
    artifactReader: ctx.service('artifacts'), presentation: ctx.shell.presentation,
    available: () => workspace.features?.side === true,
  });
  ctx.own(() => ui.dispose());
  ctx.provide('side', { open: (...args) => ui.open(...args), available: () => workspace.features?.side === true });
  for (const id of ['side','btw']) ctx.register('composer.commands', { id, value: text => ui.open(text) });
  ctx.on('connection', connection => { if (connection) ui.configure(connection.base, connection.bearer); else ui.clear(); });
}
