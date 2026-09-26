// Pure application geometry; preferences are inputs, never overwritten by responsive constraints.
export const CENTER_MIN_WIDTH = 400;
export const SIDEBAR_MIN_WIDTH = 208;
export const SIDEBAR_MAX_WIDTH = 460;
export const SIDEBAR_DEFAULT_WIDTH = 264;
export const SIDEBAR_KEYBOARD_STEP = 16;
export const MOBILE_WIDTH = 680;
export function clamp(value, min, max) { return Math.min(max, Math.max(min, Number.isFinite(value) ? Math.round(value) : min)); }
export function resolveColumns({ width, left = 0, right = 420, rightMin = 320, rightMax = 720, centerMin = CENTER_MIN_WIDTH, gap = 6 }) {
  width = Math.max(0, Number.isFinite(width) ? width : 0); left = clamp(left, 0, width);
  const available = Math.max(0, width - left), maximum = Math.max(0, available - centerMin - gap);
  const overlay = maximum < rightMin;
  const rightWidth = overlay ? clamp(right, rightMin, rightMax) : Math.min(maximum, clamp(right, rightMin, rightMax));
  return { right: rightWidth, center: overlay ? available : Math.max(0, available - rightWidth - gap), overlay, maximum };
}
export function draftHeight({ natural, floor = 40, cap = 220, workspace, header, chrome }) {
  const reading = Math.min(160, Math.max(0, workspace * 0.3));
  return Math.max(Math.min(floor, cap), Math.min(natural, cap, workspace - header - chrome - reading));
}
