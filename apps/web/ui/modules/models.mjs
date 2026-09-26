// Product companion for Models/Providers. Registered UI, unchanged management behavior.
import { ModelSettingsClient, createProviderId, parseContextWindow, MODEL_PROTOCOLS, modelProtocol, normalizeCapabilities as normalizeModelCapabilities, parseGeneration } from '../../model-settings.mjs';
import { claim, scoped, element as text, setting, dialogDismiss } from '../dom.mjs';
import { paneIcon } from '../../pane-icons.mjs';
export const version = 1;
export function mount(ctx) {
const roots = ['model-settings-section','provider-dialog','model-dialog','model-picker-dialog'].map(claim);
const control = document.getElementById('model-button') || document.createElement('button');
control.id = 'model-button'; control.type = 'button';
const label = text('span', '', '选择模型'); label.id = 'model-label'; control.replaceChildren(label);
ctx.register('composer.actions', { id:'models', order:20, value:control }); roots.push(control);
const $ = scoped(roots), on = ctx.listen, showSettings = ctx.shell.showSettings;
const providerList = $('#provider-list');
const providerSearch = $('#provider-search');
const modelList = $('#model-list');
const modelSearch = $('#model-search');
const toggleAllModels = $('#toggle-all-models');
const modelLabel = $('#model-label');
const settingsRefresh = $('#settings-refresh');
const settingsNotice = $('#model-settings-notice');
const addProvider = $('#add-provider');
const editProvider = $('#edit-provider');
const deleteProvider = $('#delete-provider');
const discoverModels = $('#discover-models');
const addModel = $('#add-model');
const providerLogo = $('#provider-logo');
const providerKind = $('#provider-kind');
const providerApiBase = $('#provider-api-base');
const providerDialog = $('#provider-dialog');
const providerForm = $('#provider-form');
const providerDialogTitle = $('#provider-dialog-title');
const providerNameInput = $('#provider-name-input');
const providerProtocolInput = $('#provider-protocol-input');
const providerGenerationInput = $('#provider-generation-input');
const providerApiBaseInput = $('#provider-api-base-input');
const providerKeyInput = $('#provider-key-input');
const providerClearKey = $('#provider-clear-key');
const providerModelsInput = $('#provider-models-input');
const providerFormError = $('#provider-form-error');
const providerDiscover = $('#provider-discover');
const providerSave = $('#provider-save');
const modelDialog = $('#model-dialog');
const modelForm = $('#model-form');
const modelIdInput = $('#model-id-input');
const modelFormError = $('#model-form-error');
const modelSave = $('#model-save');
const modelPickerDialog = $('#model-picker-dialog');
const modelPickerList = $('#model-picker-list');
const modelPickerError = $('#model-picker-error');
const openModelSettings = $('#open-model-settings');
const modelSettings = new ModelSettingsClient();
let modelSettingsState = null, modelSettingsActiveId = null, settingsTransport = 'none';
let providerEditorId = null, providerEditorRevision = 0, providerEditorModels = [], modelEditor = null;
ctx.register('composer.commands', { id: 'model', order: 30, value: {
  menu: { section: 'command', label: '模型', description: '选择后续任务使用的模型', icon: () => paneIcon('models'),
    visible: () => ctx.capability('model-management')?.active === true,
    disabled: () => !ctx.connection() || !modelSettings.configured || modelPickerDialog.open,
  },
  run: raw => { if (String(raw ?? '').trim()) throw Error('请使用 /model 打开模型选择器。'); return showModelPicker(); },
} });
function normalizeModelSettings(value) {
  const providers = Array.isArray(value?.providers) ? value.providers.map((provider) => ({
    id: typeof provider?.id === 'string' ? provider.id : '',
    name: typeof provider?.name === 'string' ? provider.name : '',
    protocol: modelProtocol(provider?.protocol),
    generation: provider?.generation || {},
    api_base: typeof provider?.api_base === 'string' ? provider.api_base : '',
    has_key: provider?.has_key === true,
    builtin: provider?.builtin === true,
    models: Array.isArray(provider?.models) ? provider.models
      .map((model) => ({ id: typeof model?.id === 'string' ? model.id : '', enabled: model?.enabled !== false,
        capabilities: normalizeModelCapabilities(model?.capabilities),
        context_window_tokens: Number.isSafeInteger(model?.context_window_tokens) && model.context_window_tokens > 0 ? model.context_window_tokens : null }))
      .filter((model) => model.id.length > 0) : [],
  })).filter((provider) => provider.id && provider.name) : [];
  const defaultValue = value?.default && typeof value.default === 'object'
    && typeof value.default.provider_id === 'string' && typeof value.default.model_id === 'string'
    ? { provider_id: value.default.provider_id, model_id: value.default.model_id }
    : null;
  return {
    revision: Number.isFinite(Number(value?.revision)) ? Number(value.revision) : 0,
    providers,
    default: defaultValue,
  };
}

function activeProvider() {
  if (!modelSettingsState?.providers.length) return null;
  return modelSettingsState.providers.find((provider) => provider.id === modelSettingsActiveId)
    || modelSettingsState.providers.find((provider) => provider.id === modelSettingsState.default?.provider_id)
    || modelSettingsState.providers[0];
}

function defaultModel() {
  if (!modelSettingsState?.default) return null;
  const provider = modelSettingsState.providers.find((item) => item.id === modelSettingsState.default.provider_id);
  const model = provider?.models.find((item) => item.id === modelSettingsState.default.model_id);
  return provider && model ? { provider, model } : null;
}

function updateModelLabel() {
  const selected = defaultModel();
  modelLabel.textContent = selected ? selected.model.id : (settingsTransport === 'http' ? '选择模型' : '默认模型');
  ctx.refreshCommands();
}

function setSettingsNotice(message = '', kind = '') {
  settingsNotice.textContent = message;
  settingsNotice.className = `settings-notice${kind ? ` ${kind}` : ''}`;
}

function applyModelSettings(value) {
  modelSettingsState = normalizeModelSettings(value);
  const selected = activeProvider();
  modelSettingsActiveId = selected?.id || null;
  updateModelLabel();
  renderModelSettings();
}

async function refreshModelSettings({ silent = false } = {}) {
  if (!modelSettings.configured) {
    renderModelSettings();
    return false;
  }
  try {
    const value = await modelSettings.get();
    applyModelSettings(value);
    if (!silent) setSettingsNotice('');
    return true;
  } catch (error) {
    if (error?.name === 'AbortError' || !modelSettings.configured) return false;
    if (error?.status === 404) {
      settingsTransport = 'unavailable';
      modelSettingsState = null;
      modelSettingsActiveId = null;
      setSettingsNotice('当前宿主未启用模型管理，继续使用宿主默认模型。');
      updateModelLabel();
      renderModelSettings();
      return false;
    }
    setSettingsNotice(error?.message || '读取模型设置失败。', 'error');
    renderModelSettings();
    return false;
  }
}

async function handleModelSettingsError(error) {
  if (error?.name === 'AbortError') return;
  if (error?.status === 404) {
    settingsTransport = 'unavailable';
    modelSettingsState = null;
    modelSettingsActiveId = null;
    updateModelLabel();
    setSettingsNotice('当前宿主未启用模型管理，继续使用宿主默认模型。', 'error');
    renderModelSettings();
    return;
  }
  if (error?.status === 409) {
    await refreshModelSettings({ silent: true });
    setSettingsNotice('设置已被其他操作更新，已刷新，请重试。', 'error');
  } else {
    setSettingsNotice(error?.message || '模型设置操作失败。', 'error');
  }
}

function providerModelsFromLines(lines, previousModels = []) {
  const previous = new Map(previousModels.map((model) => [model.id, model]));
  const ids = [...new Set(String(lines || '').split(/\r?\n/).map((value) => value.trim()).filter(Boolean))];
  return ids.map((id) => ({ id, enabled: previous.get(id)?.enabled !== false,
    context_window_tokens: previous.get(id)?.context_window_tokens ?? null,
    capabilities: normalizeModelCapabilities(previous.get(id)?.capabilities) }));
}

function providerSaveBody(provider, fields = {}) {
  const body = {
    revision: fields.revision ?? modelSettingsState?.revision ?? 0,
    id: provider?.id || fields.id,
    name: fields.name ?? provider?.name ?? '',
    api_base: fields.api_base ?? provider?.api_base ?? '',
    protocol: modelProtocol(fields.protocol ?? provider?.protocol),
    models: fields.models ?? provider?.models ?? [],
  };
  if (fields.api_key) body.api_key = fields.api_key;
  if (fields.generation !== undefined) body.generation = fields.generation;
  if (fields.clear_key) body.clear_key = true;
  return body;
}

async function saveProviderRecord(provider, fields = {}, successMessage = '已保存') {
  if (!modelSettingsState) {
    setSettingsNotice(settingsTransport === 'unavailable'
      ? '当前宿主未启用模型管理，继续使用宿主默认模型。'
      : '请先连接 HTTP Runtime。', 'error');
    return false;
  }
  try {
    const value = await modelSettings.saveProvider(providerSaveBody(provider, fields));
    applyModelSettings(value);
    if (provider?.id || fields.id) modelSettingsActiveId = provider?.id || fields.id;
    renderModelSettings();
    setSettingsNotice(successMessage);
    return true;
  } catch (error) {
    await handleModelSettingsError(error);
    return false;
  }
}

function switchButton(provider, model) {
  const isDefault = modelSettingsState?.default?.provider_id === provider.id
    && modelSettingsState?.default?.model_id === model.id;
  const enabled = model.enabled !== false || isDefault;
  const control = document.createElement('button');
  control.type = 'button';
  control.className = `model-switch${enabled ? ' on' : ''}`;
  control.setAttribute('role', 'switch');
  control.setAttribute('aria-checked', String(enabled));
  control.setAttribute('aria-label', `${model.id} ${enabled ? '不在对话中显示' : '在对话中显示'}`);
  control.disabled = isDefault;
  control.dataset.tooltip = isDefault ? '默认模型始终在对话中显示' : (enabled ? '不在对话中显示' : '在对话中显示');
  control.append(text('span', '', ''));
  control.addEventListener('click', ctx.guard(() => {
    void (async () => {
      try {
        const value = await modelSettings.setVisibility({
          revision: modelSettingsState.revision,
          provider_id: provider.id,
          model_id: model.id,
          enabled: !enabled,
        });
        applyModelSettings(value);
      } catch (error) {
        await handleModelSettingsError(error);
      }
    })();
  }));
  return control;
}

function renderProviderDetail(provider) {
  if (!provider) {
    providerLogo.textContent = 'M';
    $('#provider-name').textContent = '选择供应商';
    providerKind.textContent = '接口协议';
    providerApiBase.textContent = settingsTransport === 'tauri'
      ? 'Tauri 宿主尚未接入模型设置。'
      : (settingsTransport === 'unavailable'
        ? '当前宿主未启用模型管理。'
        : (settingsTransport === 'http' ? '当前 Runtime 没有可管理的供应商。' : '连接 HTTP Runtime 后管理供应商与模型。'));
    editProvider.disabled = true;
    deleteProvider.disabled = true;
    discoverModels.disabled = true;
    addModel.disabled = true;
    toggleAllModels.disabled = true;
    modelList.replaceChildren(text('p', 'empty-models', settingsTransport === 'tauri'
      ? 'Tauri 宿主尚未接入模型设置。'
      : (settingsTransport === 'unavailable'
        ? '当前宿主未启用模型管理。'
        : (settingsTransport === 'http' ? '点击“添加供应商”开始配置。' : '先连接 HTTP Runtime。'))));
    return;
  }
  providerLogo.textContent = (provider.name.trim()[0] || 'M').toUpperCase();
  $('#provider-name').textContent = provider.name;
  providerKind.textContent = `${provider.builtin ? '内置 · ' : ''}${MODEL_PROTOCOLS[provider.protocol]}`;
  providerApiBase.textContent = provider.api_base || '未设置 API Base';
  editProvider.disabled = false;
  deleteProvider.disabled = provider.builtin;
  deleteProvider.dataset.tooltip = provider.builtin ? '内置供应商不可删除' : '';
  discoverModels.disabled = false;
  addModel.disabled = false;

  const query = modelSearch.value.trim().toLowerCase();
  const visibleModels = provider.models.filter((model) => model.id.toLowerCase().includes(query));
  modelList.replaceChildren();
  if (!visibleModels.length) {
    modelList.append(text('p', 'empty-models', provider.models.length ? '没有匹配的模型' : '当前供应商还没有模型，请获取或手动添加。'));
  }
  for (const model of visibleModels) {
    const row = document.createElement('div');
    row.className = 'model-row';
    const info = document.createElement('div');
    info.className = 'model-info';
    const title = document.createElement('div');
    title.append(text('strong', '', model.id), text('span', 'model-badge outline', '对话'));
    const isDefault = modelSettingsState?.default?.provider_id === provider.id
      && modelSettingsState?.default?.model_id === model.id;
    if (isDefault) title.append(text('span', 'model-badge', '默认对话'));
    if (!model.enabled && !isDefault) title.append(text('span', 'model-badge', '已隐藏'));
    info.append(title);
    const windowButton = text('button', 'text-button', model.context_window_tokens == null
      ? '上下文窗口：未知 · 设置' : `上下文窗口：${model.context_window_tokens.toLocaleString()} tokens`);
    windowButton.type = 'button'; windowButton.dataset.modelContext = model.id;
    windowButton.setAttribute('aria-label', `设置 ${model.id} 的上下文窗口`);
    windowButton.addEventListener('click', ctx.guard(() => openModelEditor(model)));
    info.append(windowButton);
    row.append(info);
    const defaultButton = text('button', 'set-default', isDefault ? '当前默认' : '设为默认');
    defaultButton.type = 'button';
    defaultButton.disabled = isDefault;
    defaultButton.addEventListener('click', ctx.guard(() => setDefaultModel(provider.id, model.id)));
    row.append(defaultButton, switchButton(provider, model));
    modelList.append(row);
  }
  const selectable = provider.models.filter((model) => !(modelSettingsState?.default?.provider_id === provider.id
    && modelSettingsState?.default?.model_id === model.id));
  const allHidden = selectable.length > 0 && selectable.every((model) => model.enabled === false);
  toggleAllModels.textContent = allHidden ? '全部显示' : '全部隐藏';
  toggleAllModels.disabled = selectable.length === 0;
}

function renderModelSettings() {
  if (!providerList || !modelList) return;
  providerList.replaceChildren();
  const providers = modelSettingsState?.providers || [];
  addProvider.disabled = !modelSettingsState;
  if (!modelSettings.configured) {
    providerList.append(text('p', 'empty-settings', settingsTransport === 'tauri'
      ? 'Tauri 宿主尚未接入模型设置。'
      : '先连接 HTTP Runtime。'));
    renderProviderDetail(null);
    return;
  }
  if (!modelSettingsState) {
    providerList.append(text('p', 'empty-settings', settingsTransport === 'unavailable'
      ? '当前宿主未启用模型管理。' : '正在读取模型设置…'));
    renderProviderDetail(null);
    return;
  }
  const providerQuery = providerSearch.value.trim().toLowerCase();
  const visibleProviders = providers.filter((provider) => provider.name.toLowerCase().includes(providerQuery));
  if (!visibleProviders.length) {
    providerList.append(text('p', 'empty-settings', providers.length ? '没有匹配的供应商' : '还没有供应商'));
  }
  for (const provider of visibleProviders) {
    const row = document.createElement('button');
    row.type = 'button';
    row.className = `provider-row${provider.id === activeProvider()?.id ? ' active' : ''}`;
    row.append(text('span', 'provider-row-logo', (provider.name.trim()[0] || 'M').toUpperCase()));
    row.append(text('strong', '', provider.name));
    row.append(text('small', '', `${provider.models.length} 个模型`));
    if (provider.has_key) row.append(text('i', 'configured-dot', ''));
    row.addEventListener('click', ctx.guard(() => {
      modelSettingsActiveId = provider.id;
      modelSearch.value = '';
      renderModelSettings();
    }));
    providerList.append(row);
  }
  renderProviderDetail(activeProvider());
}

async function setDefaultModel(providerId, modelId) {
  if (!modelSettingsState) return false;
  try {
    const value = await modelSettings.setDefault({
      revision: modelSettingsState.revision,
      provider_id: providerId,
      model_id: modelId,
    });
    applyModelSettings(value);
    setSettingsNotice('默认模型已更新');
    return true;
  } catch (error) {
    await handleModelSettingsError(error);
    return false;
  }
}

function openProviderEditor(provider = null) {
  if (!modelSettings.configured) {
    setSettingsNotice(settingsTransport === 'tauri' ? 'Tauri 宿主尚未接入模型设置。' : '请先连接 HTTP Runtime。', 'error');
    return;
  }
  providerEditorId = provider?.id || null;
  providerEditorRevision = modelSettingsState?.revision ?? 0;
  providerEditorModels = (provider?.models || []).map((model) => ({ ...model }));
  providerDialogTitle.textContent = provider ? '编辑供应商' : '添加供应商';
  providerNameInput.value = provider?.name || '';
  providerProtocolInput.value = provider?.protocol || 'chat_completions';
  providerGenerationInput.value = Object.keys(provider?.generation || {}).length ? JSON.stringify(provider.generation, null, 2) : '';
  $('#provider-generation').open = false;
  syncProviderProtocol();
  providerApiBaseInput.value = provider?.api_base || '';
  providerKeyInput.value = '';
  providerKeyInput.placeholder = provider?.has_key ? '留空以保留已保存的密钥' : '仅本次提交发送，不会回显';
  providerClearKey.checked = false;
  providerModelsInput.value = providerEditorModels.map((model) => model.id).join('\n');
  providerFormError.textContent = '';
  providerDiscover.disabled = false;
  providerSave.disabled = false;
  providerForm.dataset.revision = String(providerEditorRevision);
  providerDialog.showModal();
  providerNameInput.focus();
}

function syncProviderProtocol() {
  const protocol = providerProtocolInput.value;
  $('#provider-generation').hidden = protocol === 'chat_completions';
  $('#provider-generation-hint').textContent = protocol === 'responses'
    ? '支持 reasoning（effort、summary）；不填写则使用端点默认值。'
    : '支持 thinking 和 output_config；启用 thinking 的预算必须低于本轮输出上限。';
}
on(providerProtocolInput, 'change', () => { providerGenerationInput.value = ''; syncProviderProtocol(); });

function editorProviderFields() {
  const name = providerNameInput.value.trim();
  const apiBase = providerApiBaseInput.value.trim().replace(/\/$/, '');
  const apiKey = providerKeyInput.value;
  const models = providerModelsFromLines(providerModelsInput.value, providerEditorModels);
  const protocol = modelProtocol(providerProtocolInput.value);
  return { name, protocol, generation: parseGeneration(providerGenerationInput.value, protocol), api_base: apiBase, api_key: apiKey, clear_key: providerClearKey.checked, models };
}

async function discoverInProviderEditor() {
  let fields;
  try { fields = editorProviderFields(); } catch (error) { providerFormError.textContent = error.message; return; }
  if (!fields.api_base) {
    providerFormError.textContent = '请先填写 API Base。';
    return;
  }
  providerDiscover.disabled = true;
  providerFormError.textContent = '';
  try {
    const body = { api_base: fields.api_base, protocol: fields.protocol };
    if (providerEditorId) body.provider_id = providerEditorId;
    if (fields.api_key) body.api_key = fields.api_key;
    if (fields.clear_key) body.clear_key = true;
    const result = await modelSettings.discover(body);
    const discovered = Array.isArray(result?.models) ? result.models.filter((model) => typeof model === 'string') : [];
    if (!discovered.length) throw new Error('供应商没有返回模型，请手动添加。');
    const merged = [...new Set([...fields.models.map((model) => model.id), ...discovered])];
    providerModelsInput.value = merged.join('\n');
    providerFormError.textContent = `已获取 ${discovered.length} 个模型，请保存供应商。`;
  } catch (error) {
    providerFormError.textContent = error?.message || '获取模型失败，可手动填写模型 ID。';
  } finally {
    providerDiscover.disabled = false;
  }
}

async function discoverCurrentProvider() {
  const provider = activeProvider();
  if (!provider || !modelSettingsState) return;
  const revision = modelSettingsState.revision;
  discoverModels.disabled = true;
  setSettingsNotice('正在获取模型…');
  try {
    const result = await modelSettings.discover({ provider_id: provider.id, api_base: provider.api_base, protocol: provider.protocol });
    const discovered = Array.isArray(result?.models) ? result.models.filter((model) => typeof model === 'string') : [];
    if (!discovered.length) throw new Error('供应商没有返回模型。');
    const existing = new Map(provider.models.map((model) => [model.id, model]));
    const models = [...new Set([...provider.models.map((model) => model.id), ...discovered])]
      .map((id) => ({ id, enabled: existing.get(id)?.enabled !== false,
        context_window_tokens: existing.get(id)?.context_window_tokens ?? null,
        capabilities: normalizeModelCapabilities(existing.get(id)?.capabilities) }));
    await saveProviderRecord(provider, { revision, models }, `已获取并保存 ${discovered.length} 个模型`);
  } catch (error) {
    await handleModelSettingsError(error);
  } finally {
    discoverModels.disabled = false;
  }
}

async function deleteCurrentProvider() {
  const provider = activeProvider();
  if (!provider || provider.builtin || !modelSettingsState) return;
  if (!confirm(`确定删除供应商“${provider.name}”吗？`)) return;
  deleteProvider.disabled = true;
  try {
    const value = await modelSettings.deleteProvider({
      revision: modelSettingsState.revision,
      provider_id: provider.id,
    });
    applyModelSettings(value);
    setSettingsNotice('供应商已删除');
  } catch (error) {
    await handleModelSettingsError(error);
  } finally {
    deleteProvider.disabled = false;
  }
}

function openModelEditor(model = null) {
  if (!activeProvider() || !modelSettingsState) {
    setSettingsNotice('请先添加或选择供应商。', 'error');
    return;
  }
  modelEditor = { provider_id: activeProvider().id, revision: modelSettingsState.revision, model_id: model?.id ?? null };
  modelIdInput.value = model?.id ?? '';
  modelIdInput.disabled = Boolean(model);
  $('#model-context-tokens').value = model?.context_window_tokens ?? '';
  $('#model-dialog h2').textContent = model ? '模型设置' : '添加模型';
  const caps = normalizeModelCapabilities(model?.capabilities);
  for (const select of modelForm.querySelectorAll('[data-model-cap]')) select.value = caps[select.dataset.modelCap] == null ? '' : String(caps[select.dataset.modelCap]);
  $('#model-max-output').value = caps.max_output_tokens ?? '';
  $('#model-capabilities').open = false;
  modelSave.textContent = model ? '保存' : '添加';
  modelFormError.textContent = '';
  modelSave.disabled = false;
  modelDialog.showModal();
  modelIdInput.focus();
}

async function addManualModel() {
  const provider = activeProvider();
  const id = modelIdInput.value.trim();
  if (!provider || !modelSettingsState) return false;
  if (!id) {
    modelFormError.textContent = '请输入模型 ID。';
    return false;
  }
  if (!modelEditor || provider.id !== modelEditor.provider_id) {
    modelFormError.textContent = '供应商已切换，请重新打开模型编辑。'; return false;
  }
  let contextWindow, caps;
  try {
    contextWindow = parseContextWindow($('#model-context-tokens').value);
    caps = Object.fromEntries([...modelForm.querySelectorAll('[data-model-cap]')].map(node => [node.dataset.modelCap, node.value === '' ? null : node.value === 'true']));
    caps.max_output_tokens = parseContextWindow($('#model-max-output').value);
  }
  catch (error) { modelFormError.textContent = error.message; return false; }
  if (!modelEditor.model_id && provider.models.some((model) => model.id === id)) {
    modelFormError.textContent = '该模型已经存在。';
    return false;
  }
  modelSave.disabled = true;
  const saved = await saveProviderRecord(provider, {
    revision: modelEditor.revision,
    models: modelEditor.model_id ? provider.models.map(model => model.id === modelEditor.model_id
      ? { ...model, context_window_tokens: contextWindow, capabilities: caps } : model)
      : [...provider.models, { id, enabled: true, context_window_tokens: contextWindow, capabilities: caps }],
  }, modelEditor.model_id ? '模型窗口已保存，仅影响新任务' : '模型已添加');
  modelSave.disabled = false;
  if (saved) modelDialog.close();
  return saved;
}

function renderModelPicker() {
  modelPickerList.replaceChildren();
  modelPickerError.textContent = '';
  if (!modelSettings.configured) {
    modelPickerList.append(text('p', 'empty-models', settingsTransport === 'tauri'
      ? 'Tauri 宿主尚未接入模型设置。'
      : (settingsTransport === 'unavailable'
        ? '当前宿主未启用模型管理，继续使用宿主默认模型。' : '先连接 HTTP Runtime。')));
    return;
  }
  if (!modelSettingsState) {
    modelPickerList.append(text('p', 'empty-models', settingsTransport === 'unavailable'
      ? '当前宿主未启用模型管理，继续使用宿主默认模型。' : '正在读取模型设置…'));
    return;
  }
  const selected = defaultModel();
  const models = (modelSettingsState?.providers || []).flatMap((provider) => provider.models
    .filter((model) => model.enabled !== false || (selected?.provider.id === provider.id && selected?.model.id === model.id))
    .map((model) => ({ provider, model })));
  if (!models.length) {
    modelPickerList.append(text('p', 'empty-models', '没有可显示的模型，请先在模型设置中添加或开启模型。'));
    return;
  }
  for (const item of models) {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = `model-picker-row${selected?.provider.id === item.provider.id && selected?.model.id === item.model.id ? ' active' : ''}`;
    button.append(text('strong', '', item.model.id), text('small', '', item.provider.name));
    button.addEventListener('click', ctx.guard(async () => {
      button.disabled = true;
      try {
        const changed = await setDefaultModel(item.provider.id, item.model.id);
        if (changed) modelPickerDialog.close();
        else modelPickerError.textContent = settingsNotice.textContent || '默认模型更新失败。';
      } catch (error) { modelPickerError.textContent = error?.message || '默认模型更新失败。'; }
      button.disabled = false;
    }));
    modelPickerList.append(button);
  }
}

async function showModelPicker() {
  if (settingsTransport === 'http' && !modelSettingsState) await refreshModelSettings();
  if (ctx.signal.aborted) return;
  renderModelPicker();
  modelPickerDialog.showModal();
}

on($('#model-button'), 'click', ctx.guard(showModelPicker));
on(providerSearch, 'input', renderModelSettings);
on(modelSearch, 'input', renderModelSettings);
on(settingsRefresh, 'click', () => { void refreshModelSettings(); });
on(addProvider, 'click', () => openProviderEditor());
on(editProvider, 'click', () => openProviderEditor(activeProvider()));
on(deleteProvider, 'click', () => { void deleteCurrentProvider(); });
on(discoverModels, 'click', () => { void discoverCurrentProvider(); });
on(addModel, 'click', () => openModelEditor());
on(toggleAllModels, 'click', () => {
  const provider = activeProvider();
  if (!provider || !modelSettingsState) return;
  const selectable = provider.models.filter((model) => !(modelSettingsState.default?.provider_id === provider.id
    && modelSettingsState.default?.model_id === model.id));
  const allHidden = selectable.length > 0 && selectable.every((model) => model.enabled === false);
  void (async () => {
    toggleAllModels.disabled = true;
    try {
      const value = await modelSettings.setVisibility({
        revision: modelSettingsState.revision,
        provider_id: provider.id,
        enabled: allHidden,
      });
      applyModelSettings(value);
    } catch (error) {
      await handleModelSettingsError(error);
    } finally {
      renderModelSettings();
    }
  })();
});

on(providerDiscover, 'click', () => { void discoverInProviderEditor(); });
on(providerForm, 'submit', (event) => {
  event.preventDefault();
  void (async () => {
    let fields;
    try { fields = editorProviderFields(); } catch (error) { providerFormError.textContent = error.message; return; }
    if (!fields.name || !fields.api_base) {
      providerFormError.textContent = '请填写供应商名称和 API Base。';
      return;
    }
    let id = providerEditorId;
    if (!id) {
      try { id = createProviderId(); } catch (error) {
        providerFormError.textContent = error.message;
        return;
      }
    }
    providerSave.disabled = true;
    providerFormError.textContent = '';
    const provider = modelSettingsState?.providers.find((item) => item.id === providerEditorId) || null;
    const saved = await saveProviderRecord(provider, {
      ...fields,
      id,
      revision: providerEditorRevision,
    });
    providerSave.disabled = false;
    if (saved) providerDialog.close();
    else providerFormError.textContent = settingsNotice.textContent || '保存失败，请重试。';
  })();
});
on(modelForm, 'submit', (event) => {
  event.preventDefault();
  void addManualModel().then((saved) => {
    if (!saved) modelFormError.textContent = settingsNotice.textContent || modelFormError.textContent || '保存失败，请重试。';
  });
});
on(openModelSettings, 'click', (event) => {
  event.preventDefault();
  modelPickerDialog.close();
  showSettings('models');
});
on(providerDialog, 'close', () => {
  providerKeyInput.value = '';
  providerClearKey.checked = false;
  providerFormError.textContent = '';
});

for (const [dialog, ready] of [[providerDialog,()=>!providerSave.disabled],[modelDialog,()=>!modelSave.disabled],[modelPickerDialog,()=>true]]) dialogDismiss(ctx,dialog,ready);
setting(ctx,'models',roots[0],()=>{renderModelSettings();if(!modelSettingsState)void refreshModelSettings();modelSearch.focus();},30);
ctx.on('connection',async connection=>{settingsTransport=connection?'http':'none';if(connection){modelSettings.configure(connection.base,connection.bearer);await refreshModelSettings({silent:true});}else modelSettings.clear();});
ctx.own(()=>{modelSettings.clear();providerKeyInput.value='';for(const dialog of roots.filter(n=>n.tagName==='DIALOG'))if(dialog.open)dialog.close();modelSettingsState=null;});
renderModelSettings();updateModelLabel();
}
