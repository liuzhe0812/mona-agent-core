// Application layout only: two independent tab groups, no content or execution state.
export class PaneLayout {
  constructor() { this.groups = [[], []]; this.selected = [null, null]; this.focus = 0; this.ratio = 50; }
  get split() { return this.groups[1].length > 0; }
  get order() { return this.groups.flat(); }
  get active() { return this.selected[this.focus] || this.selected[0]; }
  side(id) { return this.groups[1].includes(id) ? 1 : 0; }
  add(id, side = this.focus, activate = true) {
    if (this.order.includes(id)) { if (activate) this.activate(id); return; }
    side = side === 1 && this.groups[0].length ? 1 : 0;
    this.groups[side].push(id); this.selected[side] ||= id;
    if (activate) { this.selected[side] = id; this.focus = side; }
  }
  activate(id) { if (!this.order.includes(id)) return false; this.focus = this.side(id); this.selected[this.focus] = id; return true; }
  remove(id) {
    const side = this.side(id), group = this.groups[side], index = group.indexOf(id); if (index < 0) return;
    group.splice(index, 1); if (this.selected[side] === id) this.selected[side] = group[Math.min(index, group.length - 1)] || null;
    this.normalize();
  }
  normalize() {
    if (!this.groups[0].length && this.groups[1].length) { this.groups = [this.groups[1], []]; this.selected = [this.selected[1], null]; this.focus = 0; }
    if (!this.groups[1].length) { this.focus = 0; this.selected[1] = null; }
    for (let side = 0; side < 2; side++) if (!this.groups[side].includes(this.selected[side])) this.selected[side] = this.groups[side][0] || null;
  }
  divide(id = this.active) {
    if (this.split || this.groups[0].length < 2 || !this.groups[0].includes(id)) return false;
    this.move(id, 1); this.ratio = 50; return true;
  }
  merge() {
    const active = this.active; this.groups = [this.order, []]; this.selected = [active, null]; this.focus = 0; this.normalize();
  }
  move(id, side, index) {
    if (!this.order.includes(id) || ![0, 1].includes(side)) return false;
    const from = this.side(id), source = this.groups[from], before = source.indexOf(id);
    source.splice(before, 1);
    if (this.selected[from] === id) this.selected[from] = source[Math.min(before, source.length - 1)] || null;
    const destination = this.groups[side]; destination.splice(Math.max(0, Math.min(index ?? destination.length, destination.length)), 0, id);
    this.selected[side] = id; this.focus = side; this.normalize(); return true;
  }
  reorder(source, target, after = false) {
    if (source === target || !this.order.includes(source) || !this.order.includes(target)) return;
    const side = this.side(target), peers = this.groups[side].filter(id => id !== source), index = peers.indexOf(target) + Number(after);
    this.move(source, side, index);
  }
}
