import { mountConversationRail } from '../../conversation-rail.mjs';

export const version = 1;
export function mount(ctx) {
  return mountConversationRail(ctx);
}
