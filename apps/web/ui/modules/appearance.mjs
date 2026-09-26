import { mountAppearance } from '../../appearance.mjs';
import { claim, setting } from '../dom.mjs';
export const version = 1;
export function mount(ctx) {
  const panel = claim('appearance-settings-section');
  const ui = mountAppearance(); if (ui) ctx.own(() => ui.dispose());
  setting(ctx, 'appearance', panel, () => ui?.focus(), 60);
}
