import { browserTheme, SKINS, FONT_SIZES, MODES, SHAPES, MAX_THEME_BYTES, selectedSkin, paintPalette } from './theme.mjs';

/** Mount only appearance controls. Never replaces chat DOM or touches an Agent subscription. */
export function mountAppearance(root = document, manager = browserTheme) {
  const section = root.querySelector('#appearance-settings-section');
  if (!section || !manager) return null;
  const $ = selector => section.querySelector(selector);
  const grid = $('#theme-skins');
  const modes = $('#theme-modes');
  const size = $('#theme-font-size');
  const shape = $('#theme-shape');
  const notice = $('#theme-notice');
  const preview = $('#theme-preview');
  const reset = $('#theme-reset');
  const importButton = $('#theme-import');
  const fileInput = $('#theme-file');
  const exportButton = $('#theme-export');
  const toggle = root.querySelector('#theme-toggle');
  const modeInputs = new Map();
  const cards = new Map();
  const handlers = [];
  let disposed = false;
  const on = (element, type, fn) => { element.addEventListener(type, fn); handlers.push(() => element.removeEventListener(type, fn)); };
  const element = (tag, className, text) => {
    const node = root.createElement(tag); if (className) node.className = className;
    if (text !== undefined) node.textContent = text; return node;
  };
  for (const [value, label] of Object.entries(MODES)) {
    const choice = element('label', 'theme-mode');
    const input = element('input'); input.type = 'radio'; input.name = 'appearance-mode'; input.value = value; input.id = `theme-mode-${value}`;
    choice.append(input, element('span', '', label)); modes.append(choice); modeInputs.set(value, input);
    on(input, 'change', () => { if (input.checked) manager.update({ mode: value }); });
  }
  for (const value of FONT_SIZES) { const option = element('option', '', `${value} px${value === 15 ? ' · 默认' : ''}`); option.value = String(value); size.append(option); }
  for (const [value, label] of Object.entries(SHAPES)) { const option = element('option', '', label); option.value = value; shape.append(option); }
  function createCard(item) {
    const label = element('label', 'theme-card');
    const input = element('input'); input.type = 'radio'; input.name = 'appearance-skin'; input.value = item.id;
    input.setAttribute('aria-label', item.name);
    const content = element('span', 'theme-card-content');
    const sample = element('span', 'theme-mini'); sample.setAttribute('aria-hidden', 'true');
    const sidebar = element('span', 'theme-mini-sidebar'); sidebar.append(element('i'), element('i'), element('i'));
    const workspace = element('span', 'theme-mini-workspace'); workspace.append(element('i', 'theme-mini-user'), element('i', 'theme-mini-line'), element('i', 'theme-mini-line short'), element('i', 'theme-mini-composer'));
    sample.append(sidebar, workspace);
    const title = element('span', 'theme-card-title');
    const name = element('strong', '', item.name); title.append(name, element('span', 'theme-card-check', '✓'));
    const description = element('small', '', item.description);
    content.append(sample, title, description); label.append(input, content); grid.append(label);
    on(input, 'change', () => { if (input.checked) manager.update({ skin: item.id }); });
    const card = { label, input, sample, name, description }; cards.set(item.id, card); return card;
  }
  for (const item of SKINS) createCard(item);
  function render() {
    const value = manager.value, active = selectedSkin(value);
    for (const [id, input] of modeInputs) input.checked = id === value.mode;
    const available = value.custom ? [...SKINS, { ...value.custom, id: 'custom', description: '本地导入的皮肤' }] : SKINS;
    for (const item of available) {
      const card = cards.get(item.id) || createCard(item);
      card.label.hidden = false; card.input.checked = value.skin === item.id;
      card.name.textContent = item.name; card.input.setAttribute('aria-label', item.name);
      paintPalette(card.sample, item[manager.mode], value);
    }
    if (!value.custom && cards.has('custom')) cards.get('custom').label.hidden = true;
    size.value = String(value.fontSize); shape.value = value.shape;
    paintPalette(preview, active[manager.mode], value);
    $('#theme-current').textContent = `${active.name} · ${MODES[manager.mode]}`;
    $('#theme-mode-description').textContent = value.mode === 'system' ? `随设备切换，当前为${MODES[manager.mode]}。` : '浅色与深色共用同一套皮肤，切换后自动记住。';
    notice.textContent = manager.notice || '选择即生效，仅保存在此浏览器，不影响会话和任务。';
    notice.classList.toggle('error', manager.failed);
    if (toggle) {
      const label = `切换到${manager.mode === 'dark' ? '浅色' : '深色'}`;
      toggle.setAttribute('aria-label', label); toggle.setAttribute('aria-pressed', String(manager.mode === 'dark'));
      toggle.dataset.tooltip = `${label}；更多选项在“设置 → 外观”`;
    }
  }
  on(size, 'change', () => manager.update({ fontSize: Number(size.value) }));
  on(shape, 'change', () => manager.update({ shape: shape.value }));
  on(reset, 'click', () => { if (manager.reset()) manager.report('已恢复默认外观；导入的皮肤仍保留在列表中。'); });
  if (toggle) on(toggle, 'click', () => { if (!manager.toggleMode()) section.hidden ? root.defaultView.alert(manager.notice) : notice.focus(); });
  on(importButton, 'click', () => fileInput.click());
  on(fileInput, 'change', async () => {
    const file = fileInput.files?.[0]; if (!file) return;
    importButton.disabled = true; importButton.textContent = '正在读取…';
    try {
      if (file.size > MAX_THEME_BYTES) throw new Error('皮肤文件不能超过 32 KiB。');
      const text = await file.text();
      if (!disposed) manager.importSkin(text);
    } catch (error) { if (!disposed) manager.report(error.message || '皮肤文件读取失败，请重试。', true); }
    finally { fileInput.value = ''; importButton.disabled = false; importButton.textContent = '导入皮肤'; }
  });
  on(exportButton, 'click', () => {
    let url;
    try {
      url = URL.createObjectURL(new Blob([manager.exportSkin()], { type: 'application/json' }));
      const link = element('a'); link.href = url; link.download = `mona-theme-${manager.value.skin}.json`;
      link.hidden = true; section.append(link); link.click(); link.remove();
      // Allow the browser to consume the object URL before releasing it.
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      manager.report('已生成皮肤文件，仅含浅色与深色配色，不含会话或凭据。');
    } catch { if (url) URL.revokeObjectURL(url); manager.report('皮肤导出失败，请检查浏览器下载权限。', true); }
  });
  const unsubscribe = manager.subscribe(render);
  return { focus() { modeInputs.get(manager.value.mode)?.focus(); }, dispose() { disposed = true; unsubscribe(); handlers.forEach(remove => remove()); } };
}
