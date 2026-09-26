/** Browser-only appearance. No Agent client, model, tool, or host dependency. */
export const THEME_STORAGE_KEY = 'mona.web.appearance.v1';
export const MAX_THEME_BYTES = 32 * 1024;
export const FONT_SIZES = Object.freeze([14, 15, 16, 18]);
export const MODES = Object.freeze({ light: '浅色', dark: '深色', system: '跟随系统' });
export const SHAPES = Object.freeze({ standard: '标准', soft: '柔和', crisp: '利落' });
export const DEFAULT_APPEARANCE = Object.freeze({ version: 1, skin: 'graphite', mode: 'light', fontSize: 14, shape: 'standard', custom: null });

const light = {
  bg: '#f8f8f8', panel: '#ffffff', text: '#303030', muted: '#606060', line: '#dedede', soft: '#efefef',
  accent: '#171717', sidebar: '#ececee', selected: '#dedede', danger: '#a53030', success: '#236b43',
  'switch-on': '#345fc6', focus: '#315ecc', inverse: '#ffffff', 'activity-shimmer': '#ffffff',
};
const dark = {
  bg: '#181818', panel: '#222222', text: '#e5e5e5', muted: '#b0b0b0', line: '#383838', soft: '#292929',
  accent: '#f4f4f4', sidebar: '#202020', selected: '#343434', danger: '#ff9999', success: '#8bcda1',
  'switch-on': '#91b9ff', focus: '#91b9ff', inverse: '#171717', 'activity-shimmer': '#ffffff',
};
export const PALETTE_KEYS = Object.freeze(Object.keys(light));
function skin(id, name, description, day = {}, night = {}) {
  return Object.freeze({ id, name, description, light: Object.freeze({ ...light, ...day }), dark: Object.freeze({ ...dark, ...night }) });
}
export const SKINS = Object.freeze([
  skin('graphite', '石墨', '熟悉的中性灰白'),
  skin('ocean', '海盐蓝', '清透而专注', {
    bg: '#f5f8fc', text: '#24334a', muted: '#526078', line: '#d4dfed', soft: '#e7eef8', accent: '#275dc0',
    sidebar: '#eaf0f8', selected: '#dce7f7', focus: '#275dc0', 'switch-on': '#275dc0',
  }, {
    bg: '#111b2a', panel: '#172439', text: '#dce8ff', muted: '#a7b8d2', line: '#2c405c', soft: '#1e3049',
    accent: '#93baff', sidebar: '#142034', selected: '#2a3e5b', focus: '#93baff', 'switch-on': '#93baff', inverse: '#111b2a',
  }),
  skin('forest', '松林绿', '安静的自然气息', {
    bg: '#f5f8f5', text: '#273c31', muted: '#4e6557', line: '#d6e2d8', soft: '#e9f1e9', accent: '#276348',
    sidebar: '#eaf0e9', selected: '#dbe8dc', focus: '#276348', 'switch-on': '#276348',
  }, {
    bg: '#111d18', panel: '#182820', text: '#e1efe5', muted: '#a8c0b2', line: '#30473a', soft: '#203429',
    accent: '#98cfaf', sidebar: '#15241c', selected: '#2b4033', focus: '#98cfaf', 'switch-on': '#98cfaf', inverse: '#111d18',
  }),
  skin('iris', '鸢尾紫', '克制的一抹灵感', {
    bg: '#f8f6fc', text: '#393148', muted: '#635870', line: '#e0d9ed', soft: '#efe9f7', accent: '#6c45aa',
    sidebar: '#f0edf6', selected: '#e7dff2', focus: '#6c45aa', 'switch-on': '#6c45aa',
  }, {
    bg: '#1d1826', panel: '#282134', text: '#ece3f8', muted: '#c0b0d1', line: '#463853', soft: '#352b44',
    accent: '#c6a4ee', sidebar: '#231c2e', selected: '#40324f', focus: '#c6a4ee', 'switch-on': '#c6a4ee', inverse: '#1d1826',
  }),
  skin('sand', '暖砂', '纸张般温和', {
    bg: '#faf8f3', panel: '#fffffb', text: '#443b2d', muted: '#685c4b', line: '#dfd8c9', soft: '#f1ebde',
    accent: '#805523', sidebar: '#f1ecdf', selected: '#e6dfcf', focus: '#805523', 'switch-on': '#805523',
  }, {
    bg: '#211c16', panel: '#2c261f', text: '#f1e7d6', muted: '#c7b89f', line: '#494031', soft: '#382f23',
    accent: '#dbb984', sidebar: '#272119', selected: '#443827', focus: '#dbb984', 'switch-on': '#dbb984', inverse: '#211c16',
  }),
  skin('rose', '蔷薇', '柔和但不甜腻', {
    bg: '#fcf6f7', panel: '#fffafa', text: '#4b3139', muted: '#70545f', line: '#ead8df', soft: '#f6e8ee',
    accent: '#a13f66', sidebar: '#f5e9ed', selected: '#eedce4', focus: '#a13f66', 'switch-on': '#a13f66',
  }, {
    bg: '#23171e', panel: '#30212a', text: '#f5e2eb', muted: '#cbb1be', line: '#4c3441', soft: '#3b2833',
    accent: '#e9a8c0', sidebar: '#2a1d25', selected: '#49313e', focus: '#e9a8c0', 'switch-on': '#e9a8c0', inverse: '#23171e',
  }),
]);

function exactObject(value, keys, label) {
  if (!value || typeof value !== 'object' || Array.isArray(value)
    || Object.getPrototypeOf(value) !== Object.prototype
    || Object.keys(value).some(key => !keys.includes(key))
    || keys.some(key => !Object.hasOwn(value, key))) throw new Error(`${label}的字段不完整或不受支持。`);
}
function luminance(hex) {
  const values = [1, 3, 5].map(index => {
    const value = parseInt(hex.slice(index, index + 2), 16) / 255;
    return value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4;
  });
  return values[0] * .2126 + values[1] * .7152 + values[2] * .0722;
}
export function contrast(a, b) {
  const first = luminance(a), second = luminance(b);
  return (Math.max(first, second) + .05) / (Math.min(first, second) + .05);
}
export function validatePalette(value, label = '配色') {
  exactObject(value, PALETTE_KEYS, label);
  const result = {};
  for (const key of PALETTE_KEYS) {
    if (typeof value[key] !== 'string' || !/^#[0-9a-f]{6}$/i.test(value[key])) throw new Error(`${label}只接受 #RRGGBB 颜色，不接受 CSS、脚本或图片地址。`);
    result[key] = value[key].toLowerCase();
  }
  for (const surface of ['bg', 'panel', 'soft', 'sidebar', 'selected']) {
    for (const foreground of ['text', 'muted']) {
      if (contrast(result[foreground], result[surface]) < 4.5) throw new Error(`${label}的文字对比度不足，请调整 ${foreground} 与 ${surface}。`);
    }
  }
  for (const foreground of ['danger', 'success']) {
    if (['bg', 'panel'].some(surface => contrast(result[foreground], result[surface]) < 4.5)) throw new Error(`${label}的状态文字对比度不足。`);
  }
  if (contrast(result.inverse, result.accent) < 4.5
    || ['bg', 'panel', 'sidebar'].some(surface => contrast(result.focus, result[surface]) < 3)) throw new Error(`${label}的按钮或焦点对比度不足。`);
  return result;
}
export function validateSkin(value) {
  exactObject(value, ['format', 'version', 'name', 'light', 'dark'], '皮肤');
  if (value.format !== 'mona-theme' || value.version !== 1) throw new Error('不支持的皮肤格式或版本。');
  if (typeof value.name !== 'string' || !value.name.trim() || value.name.trim().length > 32
    || /[\u0000-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/u.test(value.name)) throw new Error('皮肤名称须为 1–32 个有效字符。');
  return { format: 'mona-theme', version: 1, name: value.name.trim(), light: validatePalette(value.light, '浅色配色'), dark: validatePalette(value.dark, '深色配色') };
}
function parseBounded(text) {
  if (typeof text !== 'string' || text.length > MAX_THEME_BYTES || new TextEncoder().encode(text).length > MAX_THEME_BYTES) throw new Error('外观文件不能超过 32 KiB。');
  try { return JSON.parse(text); } catch { throw new Error('外观文件不是有效的 JSON。'); }
}
export function parseSkin(text) { return validateSkin(parseBounded(text)); }
export function normalizeAppearance(value) {
  exactObject(value, Object.keys(DEFAULT_APPEARANCE), '外观设置');
  if (value.version !== 1 || typeof value.mode !== 'string' || typeof value.shape !== 'string'
    || !Object.hasOwn(MODES, value.mode) || !Object.hasOwn(SHAPES, value.shape)
    || !FONT_SIZES.includes(value.fontSize)) throw new Error('外观设置的版本或选项不受支持。');
  const custom = value.custom === null ? null : validateSkin(value.custom);
  if (!SKINS.some(item => item.id === value.skin) && !(value.skin === 'custom' && custom)) throw new Error('所选皮肤不存在。');
  return { version: 1, skin: value.skin, mode: value.mode, fontSize: value.fontSize, shape: value.shape, custom };
}
export function resolveMode(mode, systemDark = false) { return mode === 'system' ? (systemDark ? 'dark' : 'light') : mode; }
export function selectedSkin(preference) {
  return preference.skin === 'custom' ? { ...preference.custom, id: 'custom', description: '本地导入的皮肤' } : SKINS.find(item => item.id === preference.skin);
}
export function paintPalette(element, palette, preference) {
  if (!element) return;
  for (const key of PALETTE_KEYS) element.style.setProperty(`--${key}`, palette[key]);
  element.style.setProperty('--chat-font-size', `${preference.fontSize}px`);
  // Mona's visible-layer tiers are the default. Softer/crisper options remain bounded variants,
  // not permission for each feature to invent its own radii.
  const [composer, bubble, panel, detail] = {
    standard: [22, 12, 12, 8],
    soft: [28, 16, 16, 12],
    crisp: [12, 8, 8, 8],
  }[preference.shape];
  for (const [key, value] of Object.entries({ composer, bubble, panel, detail })) element.style.setProperty(`--skin-${key}-radius`, `${value}px`);
  element.style.setProperty('--switch-thumb', palette.inverse);
}

/** Persist first, publish second. A failed write cannot silently change the selected appearance. */
export class ThemeManager {
  #preference = { ...DEFAULT_APPEARANCE };
  #listeners = new Set();
  constructor({ root = null, storage = () => globalThis.localStorage, media = null, events = null } = {}) {
    this.root = root; this.storage = storage; this.media = media; this.events = events;
    this.notice = ''; this.failed = false;
    try {
      const saved = storage()?.getItem(THEME_STORAGE_KEY);
      if (saved != null) this.#preference = normalizeAppearance(parseBounded(saved));
    } catch {
      this.notice = '无法读取已保存的外观设置，本次使用默认外观；旧数据未被覆盖。可重新选择或恢复默认。'; this.failed = true;
    }
    this.apply();
    this.onSystemChange = () => { if (this.#preference.mode === 'system') { this.apply(); this.emit(); } };
    this.onStorage = event => {
      if (event.key !== THEME_STORAGE_KEY && event.key !== null) return;
      try {
        if (event.storageArea && event.storageArea !== this.storage()) return;
        // Re-read the current value, not a potentially stale queued event payload.
        const saved = this.storage()?.getItem(THEME_STORAGE_KEY);
        this.#preference = saved == null ? { ...DEFAULT_APPEARANCE } : normalizeAppearance(parseBounded(saved));
        this.notice = '已同步此浏览器其他页面的外观设置。'; this.failed = false; this.apply();
      } catch { this.notice = '其他页面的外观设置无效，已保留当前外观。'; this.failed = true; }
      this.emit();
    };
    media?.addEventListener('change', this.onSystemChange);
    events?.addEventListener('storage', this.onStorage);
  }
  get value() { return structuredClone(this.#preference); }
  get mode() { return resolveMode(this.#preference.mode, this.media?.matches === true); }
  apply() {
    if (!this.root) return;
    paintPalette(this.root, selectedSkin(this.#preference)[this.mode], this.#preference);
    this.root.dataset.theme = this.mode;
    this.root.dataset.skin = this.#preference.skin;
    this.root.style.colorScheme = this.mode;
  }
  emit() { for (const listener of this.#listeners) listener(); }
  subscribe(listener) { this.#listeners.add(listener); listener(); return () => this.#listeners.delete(listener); }
  report(message, failed = false) { this.notice = message; this.failed = failed; this.emit(); }
  update(patch) {
    let next;
    try { next = normalizeAppearance({ ...this.#preference, ...patch }); }
    catch (error) { this.report(error.message, true); return false; }
    try {
      const storage = this.storage();
      if (!storage) throw new Error('Storage unavailable');
      storage.setItem(THEME_STORAGE_KEY, JSON.stringify(next));
    } catch { this.report('外观保存失败，已保留原设置。请允许浏览器本地存储或释放空间后重试。', true); return false; }
    this.#preference = next; this.apply(); this.report('已保存到此浏览器。'); return true;
  }
  toggleMode() { return this.update({ mode: this.mode === 'dark' ? 'light' : 'dark' }); }
  reset() { return this.update({ ...DEFAULT_APPEARANCE, custom: this.#preference.custom }); }
  importSkin(text) {
    try { return this.update({ custom: parseSkin(text), skin: 'custom' }); }
    catch (error) { this.report(error.message, true); return false; }
  }
  exportSkin() {
    const { name, light, dark } = selectedSkin(this.#preference);
    return JSON.stringify({ format: 'mona-theme', version: 1, name, light, dark }, null, 2) + '\n';
  }
  dispose() {
    this.media?.removeEventListener('change', this.onSystemChange);
    this.events?.removeEventListener('storage', this.onStorage); this.#listeners.clear();
  }
}

// Loaded once from <head> and reused by the settings UI, before Agent auto-connect.
export const browserTheme = typeof document === 'undefined' ? null : new ThemeManager({
  root: document.documentElement, storage: () => window.localStorage,
  media: window.matchMedia('(prefers-color-scheme: dark)'), events: window,
});
