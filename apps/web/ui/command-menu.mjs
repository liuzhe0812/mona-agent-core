import { PaneMenu } from '../pane-controls.mjs';
import { commandGroups, runCommand } from './commands.mjs';

const COMMAND_MENU_GAP = 4;
const COMMAND_MENU_EDGE = 12;
const COMMAND_MENU_MAX_HEIGHT = 400;

// Reuse the existing top-layer menu lifecycle; only this menu follows the entire composer.
export class ComposerCommandMenu extends PaneMenu {
  constructor(registry, button, root, onError) {
    super(root, { gap: COMMAND_MENU_GAP, edge: COMMAND_MENU_EDGE, side: 'top', align: 'start' });
    this.registry = registry; this.button = button; this.anchor = root.closest('.composer'); this.onError = onError;
    this.records = []; this.signature = ''; this.pending = false;
    button.addEventListener('click', () => this.opened ? this.close(true) : this.open(), { signal: this.events.signal });
    button.addEventListener('keydown', event => {
      if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
      event.preventDefault(); this.open();
      if (event.key === 'ArrowUp') [...root.querySelectorAll('button:not(:disabled)')].at(-1)?.focus();
    }, { signal: this.events.signal });
    window.addEventListener('hashchange', () => this.close(), { signal: this.events.signal });
    this.unsubscribe = registry.subscribe('composer.commands', () => this.refresh());
  }
  report(error) { try { this.onError(error.message); } catch { /* Other controls remain usable. */ } }
  refresh() {
    const groups = commandGroups(this.registry.entries('composer.commands'), error => this.report(error));
    this.button.hidden = groups.length === 0;
    if (!groups.length) { this.close(); this.root.replaceChildren(); this.records = []; this.signature = ''; return; }
    if (!this.opened) return;
    const entries = groups.flatMap(group => group.rows.map(row => row.entry));
    const signature = JSON.stringify(groups.map(group => [group.id, group.rows.map(({ entry, disabled }) => [entry.id, entry.value.menu.label, entry.value.menu.description || '', disabled]) ]));
    if (signature === this.signature && entries.every((entry, index) => this.records[index] === entry)) return;
    const focused = this.root.contains(document.activeElement) ? document.activeElement.dataset.command : null;
    const scrollTop = this.root.scrollTop, nodes = [];
    for (const group of groups) {
      const section = document.createElement('div'); section.className = 'composer-command-group'; section.setAttribute('role', 'group'); section.setAttribute('aria-label', group.label);
      const heading = document.createElement('div'); heading.className = 'composer-command-heading'; heading.textContent = group.label; heading.setAttribute('aria-hidden', 'true'); section.append(heading);
      for (const { entry, disabled } of group.rows) {
        const { menu } = entry.value;
        const row = document.createElement('button'); row.type = 'button'; row.className = 'composer-command-item'; row.dataset.command = entry.id; row.setAttribute('role', 'menuitem'); row.disabled = disabled;
        const icon = document.createElement('span'); icon.className = 'command-icon'; icon.setAttribute('aria-hidden', 'true');
        try { const glyph = menu.icon?.(); if (glyph instanceof Element) icon.append(glyph); } catch (error) { this.report(error); }
        const label = document.createElement('span'); label.className = 'command-label'; label.textContent = menu.label;
        const alias = document.createElement('span'); alias.className = 'command-alias'; alias.textContent = entry.id;
        const description = document.createElement('span'); description.className = 'command-description'; description.textContent = menu.description || '';
        row.append(icon, label, alias, description);
        row.addEventListener('click', () => { void this.select(entry); });
        section.append(row);
      }
      nodes.push(section);
    }
    this.records = entries; this.signature = signature; this.root.replaceChildren(...nodes); this.root.scrollTop = scrollTop;
    if (focused) (this.root.querySelector(`[data-command="${focused}"]:not(:disabled)`) || this.root.querySelector('button:not(:disabled)') || this.button).focus({ preventScroll: true });
    this.position();
  }
  open() {
    if (this.pending || this.button.hidden || !this.anchor?.isConnected) return;
    // Populate before PaneMenu performs its initial keyboard focus selection.
    this.opened = true; this.refresh(); this.opened = false;
    if (this.button.hidden) return;
    super.open(this.button);
    this.size?.observe(this.anchor);
  }
  async select(entry) {
    if (!this.opened || this.pending || this.registry.resolve('composer.commands', entry.id) !== entry.value) return;
    this.pending = true;
    try {
      this.close(true);
      await runCommand(entry.value, '', true);
    } catch (error) { this.report(error); }
    finally { this.pending = false; }
  }
  position() {
    if (!this.opened || !this.anchor) return;
    const box = this.anchor.getBoundingClientRect();
    if (box.width === 0 || this.anchor.closest('[hidden],[inert]')) { this.close(); return; }
    const width = Math.max(0, Math.min(box.width, innerWidth - COMMAND_MENU_EDGE * 2));
    const available = box.top - COMMAND_MENU_EDGE - COMMAND_MENU_GAP;
    const height = Math.min(COMMAND_MENU_MAX_HEIGHT, available >= 60 ? available : innerHeight - COMMAND_MENU_EDGE * 2);
    this.root.style.setProperty('--composer-menu-width', `${width}px`);
    this.root.style.setProperty('--composer-menu-height', `${Math.max(0, height)}px`);
    const menuHeight = this.root.getBoundingClientRect().height;
    const x = Math.max(COMMAND_MENU_EDGE, Math.min(box.left, innerWidth - width - COMMAND_MENU_EDGE));
    const y = Math.max(COMMAND_MENU_EDGE, Math.min(box.top - menuHeight - COMMAND_MENU_GAP, innerHeight - menuHeight - COMMAND_MENU_EDGE));
    this.root.style.setProperty('--pane-menu-x', `${Math.round(x)}px`);
    this.root.style.setProperty('--pane-menu-y', `${Math.round(y)}px`);
  }
  dispose() { this.unsubscribe(); super.dispose(); this.root.replaceChildren(); this.button.hidden = true; }
}
