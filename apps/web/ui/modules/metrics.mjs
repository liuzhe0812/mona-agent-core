import { ConversationMetrics } from '../../conversation-metrics.mjs';
import { element } from '../dom.mjs';
export const version = 1;
export function mount(ctx) {
  const root = element('div','conversation-metrics'); root.id = 'conversation-metrics'; root.setAttribute('aria-label','会话统计');
  ctx.register('status', { id:'metrics', order:10, value:root });
  const workspace = ctx.service('workspace'), ui = new ConversationMetrics(root, workspace.api);
  ctx.own(() => ui.dispose());
  ctx.on('session', session => ui.select(session, workspace.features?.statistics));
  ctx.on('workspace-ready', () => ui.select(ctx.shell.session(), workspace.features?.statistics));
  ctx.on('run', state => ui.observe(state)); ctx.on('cleared', () => ui.clear());
}
