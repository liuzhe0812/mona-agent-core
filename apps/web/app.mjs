import { RunView, requestId } from '../../packages/client/src/index.mjs';
import { HttpAgentClient } from '../../packages/client/src/http.mjs';
import { TauriAgentClient } from '../../packages/client/src/tauri.mjs';
import { ModelSettingsClient, createProviderId, parseContextWindow } from './model-settings.mjs';
import { CapabilitiesClient } from './capabilities.mjs';
import { SpillClient } from './spill.mjs';
import { TurnView } from './run-view.mjs';
import { ConversationUI } from './sessions-ui.mjs';
import { mountAppearance } from './appearance.mjs';
import './tooltip.mjs';
import './conversation-rail.mjs';

const $ = (selector, root = document) => root.querySelector(selector);
const timeline = $('#timeline');
const welcome = $('#welcome');
const prompt = $('#prompt');
const send = $('#send');
const cancel = $('#cancel');
const dialog = $('#connection-dialog');
const form = $('#connection-form');
const transport = $('#transport');
const endpoint = $('#endpoint');
const token = $('#token');
const connectionError = $('#connection-error');
const connectButton = $('#connect');
const httpFields = $('#http-fields');
const main = $('#main');
const chatSidebar = $('#chat-sidebar');
const chatWorkspace = $('#chat-workspace');
const settingsPage = $('#settings-page');
const componentSettingsSection = $('#component-settings-section');
const toolSettingsSection = $('#tool-settings-section');
const modelSettingsSection = $('#model-settings-section');
const componentList = $('#component-list');
const toolList = $('#tool-list');
const componentsNotice = $('#components-notice');
const toolsNotice = $('#tools-notice');
const componentsRefresh = $('#components-refresh');
const toolsRefresh = $('#tools-refresh');
const componentsTab = $('#settings-components-tab');
const toolsTab = $('#settings-tools-tab');
const modelsTab = $('#settings-models-tab');
const appearanceTab = $('#settings-appearance-tab');
const appearanceSection = $('#appearance-settings-section');
const appearance = mountAppearance();
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

let client = null;
let active = null;
let renderFrame = 0;
let workTimer = null;
let starting = false;
const modelSettings = new ModelSettingsClient();
const capabilities = new CapabilitiesClient();
const spillResults = new SpillClient();
let capabilitiesState = null;
let modelSettingsState = null;
let modelSettingsActiveId = null;
let settingsTransport = 'none';
let providerEditorId = null;
let providerEditorRevision = 0;
let providerEditorModels = [];
let modelEditor = null;
const conversations = new ConversationUI({
  busy: () => starting || Boolean(active && !active.done),
  changed: () => setBusy(Boolean(active && !active.done)),
  createTurn,
  openSettings: () => showSettings(),
  clear() {
    active?.subscription?.close(); clearInterval(workTimer);
    if (renderFrame) cancelAnimationFrame(renderFrame);
    renderFrame = 0; active = null; timeline.replaceChildren(); welcome.hidden = false;
  },
  async live(runId, dom) {
    const snapshot = await client.snapshot(runId);
    const view = new RunView(runId);
    view.apply({ kind: 'snapshot', reason: 'initial', snapshot });
    void subscribeRun(runId, view, dom);
  },
});

function normalizeModelSettings(value) {
  const providers = Array.isArray(value?.providers) ? value.providers.map((provider) => ({
    id: typeof provider?.id === 'string' ? provider.id : '',
    name: typeof provider?.name === 'string' ? provider.name : '',
    api_base: typeof provider?.api_base === 'string' ? provider.api_base : '',
    has_key: provider?.has_key === true,
    builtin: provider?.builtin === true,
    models: Array.isArray(provider?.models) ? provider.models
      .map((model) => ({ id: typeof model?.id === 'string' ? model.id : '', enabled: model?.enabled !== false,
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
}

function setSettingsNotice(message = '', kind = '') {
  settingsNotice.textContent = message;
  settingsNotice.className = `settings-notice${kind ? ` ${kind}` : ''}`;
}

function clearModelSettings() {
  conversations.clear();
  modelSettings.clear();
  capabilities.clear();
  spillResults.clear();
  capabilitiesState = null;
  modelSettingsState = null;
  modelSettingsActiveId = null;
  settingsTransport = 'none';
  setSettingsNotice('');
  setAgentSettingsNotice('all');
  updateModelLabel();
  renderCapabilities();
  renderModelSettings();
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
    context_window_tokens: previous.get(id)?.context_window_tokens ?? null }));
}

function providerSaveBody(provider, fields = {}) {
  const body = {
    revision: fields.revision ?? modelSettingsState?.revision ?? 0,
    id: provider?.id || fields.id,
    name: fields.name ?? provider?.name ?? '',
    api_base: fields.api_base ?? provider?.api_base ?? '',
    models: fields.models ?? provider?.models ?? [],
  };
  if (fields.api_key) body.api_key = fields.api_key;
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

function selectSettingsSection(section) {
  if (!['components', 'tools', 'models', 'appearance'].includes(section)) section = 'components';
  for (const [name, panel, tab] of [
    ['components', componentSettingsSection, componentsTab],
    ['tools', toolSettingsSection, toolsTab],
    ['models', modelSettingsSection, modelsTab],
    ['appearance', appearanceSection, appearanceTab],
  ]) {
    const selected = section === name;
    panel.hidden = !selected;
    tab.classList.toggle('active', selected);
    if (selected) tab.setAttribute('aria-current', 'page');
    else tab.removeAttribute('aria-current');
  }
  if (section === 'appearance') {
    appearance?.focus();
  } else if (section === 'models') {
    renderModelSettings();
    if (settingsTransport === 'http' && !modelSettingsState) void refreshModelSettings();
    modelSearch.focus();
  } else {
    renderCapabilities();
    if (settingsTransport === 'http' && !capabilitiesState) void refreshCapabilities();
    (section === 'tools' ? toolsRefresh : componentsRefresh).focus();
  }
}

function showSettings(section = 'components') {
  closeSidebar();
  chatSidebar.hidden = true;
  chatWorkspace.hidden = true;
  settingsPage.hidden = false;
  selectSettingsSection(section);
}

function hideSettings() {
  settingsPage.hidden = true;
  chatSidebar.hidden = false;
  chatWorkspace.hidden = false;
  prompt.focus();
}

function normalizeCapabilityEntries(value) {
  return Array.isArray(value) ? value
    .filter((item) => item && typeof item.id === 'string' && typeof item.name === 'string')
    .map((item) => ({
      id: item.id,
      name: item.name,
      description: typeof item.description === 'string' ? item.description : '',
      enabled: item.enabled === true,
      restart_required: item.restart_required === true,
    })) : [];
}

function normalizeCapabilities(value) {
  return {
    revision: Number.isSafeInteger(value?.revision) ? value.revision : 0,
    components: normalizeCapabilityEntries(value?.components),
    tools: normalizeCapabilityEntries(value?.tools),
  };
}

function setAgentSettingsNotice(group, message = '', kind = '') {
  const notices = group === 'all'
    ? [componentsNotice, toolsNotice]
    : [group === 'tools' ? toolsNotice : componentsNotice];
  for (const notice of notices) {
    notice.textContent = message;
    notice.className = `settings-notice${kind ? ` ${kind}` : ''}`;
  }
}

function capabilityGroup(id) {
  return capabilitiesState?.tools.some((item) => item.id === id) ? 'tools' : 'components';
}

async function refreshCapabilities({ silent = false } = {}) {
  if (!capabilities.configured) {
    renderCapabilities();
    return false;
  }
  componentsRefresh.disabled = true;
  toolsRefresh.disabled = true;
  try {
    capabilitiesState = normalizeCapabilities(await capabilities.get());
    if (!silent) setAgentSettingsNotice('all');
    renderCapabilities();
    return true;
  } catch (error) {
    if (error?.name === 'AbortError') return false;
    capabilitiesState = null;
    setAgentSettingsNotice('all', error?.status === 404
      ? '当前宿主没有提供组件与工具管理接口。'
      : (error?.message || '读取 Agent 设置失败。'), 'error');
    renderCapabilities();
    return false;
  } finally {
    componentsRefresh.disabled = false;
    toolsRefresh.disabled = false;
  }
}

async function setCapabilityEnabled(id, enabled) {
  if (!capabilitiesState) return;
  const group = capabilityGroup(id);
  try {
    capabilitiesState = normalizeCapabilities(await capabilities.update(id, {
      revision: capabilitiesState.revision,
      enabled,
    }));
    const item = capabilitiesState[group].find((entry) => entry.id === id);
    setAgentSettingsNotice(group, item?.restart_required ? '已保存，重启后生效。' : '已保存。');
    renderCapabilities();
  } catch (error) {
    if (error?.status === 409) await refreshCapabilities({ silent: true });
    setAgentSettingsNotice(group, error?.status === 409
      ? '设置已更新，已刷新，请重试。'
      : (error?.message || '保存失败。'), 'error');
    renderCapabilities();
  }
}

function renderCapabilityGroup(list, entries, group) {
  list.replaceChildren();
  if (!capabilities.configured) {
    list.append(text('p', 'capability-empty', settingsTransport === 'tauri'
      ? `Tauri 宿主尚未接入${group === 'tools' ? '工具' : '组件'}管理。`
      : `连接 HTTP Runtime 后管理 Agent ${group === 'tools' ? '工具' : '组件'}。`));
    return;
  }
  if (!capabilitiesState) {
    list.append(text('p', 'capability-empty', `正在读取 Agent ${group === 'tools' ? '工具' : '组件'}…`));
    return;
  }
  if (!entries.length) {
    list.append(text('p', 'capability-empty', `没有可管理的${group === 'tools' ? '工具' : '组件'}。`));
    return;
  }
  for (const item of entries) {
    const row = document.createElement('article');
    row.className = 'capability-row';
    const copy = document.createElement('div');
    copy.className = 'capability-copy';
    const title = document.createElement('div');
    title.append(text('strong', '', item.name));
    if (item.restart_required) title.append(text('span', 'capability-status pending', item.enabled ? '重启后启用' : '重启后停用'));
    copy.append(title);
    if (item.description) copy.append(text('p', '', item.description));

    const actions = document.createElement('div');
    actions.className = 'capability-actions';
    const toggle = document.createElement('button');
    toggle.type = 'button';
    toggle.className = `model-switch${item.enabled ? ' on' : ''}`;
    toggle.setAttribute('role', 'switch');
    toggle.setAttribute('aria-checked', String(item.enabled));
    toggle.setAttribute('aria-label', `${item.enabled ? '关闭' : '启用'}${item.name}`);
    toggle.append(document.createElement('span'));
    toggle.addEventListener('click', () => {
      toggle.disabled = true;
      void setCapabilityEnabled(item.id, !item.enabled);
    });
    actions.append(toggle);
    row.append(copy, actions);
    list.append(row);
  }
}

function renderCapabilities() {
  renderCapabilityGroup(componentList, capabilitiesState?.components || [], 'components');
  renderCapabilityGroup(toolList, capabilitiesState?.tools || [], 'tools');
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
  control.addEventListener('click', () => {
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
  });
  return control;
}

function renderProviderDetail(provider) {
  if (!provider) {
    providerLogo.textContent = 'M';
    $('#provider-name').textContent = '选择供应商';
    providerKind.textContent = 'OpenAI 兼容';
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
  providerKind.textContent = provider.builtin ? '内置 · OpenAI 兼容' : 'OpenAI 兼容';
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
    windowButton.addEventListener('click', () => openModelEditor(model));
    info.append(windowButton);
    row.append(info);
    const defaultButton = text('button', 'set-default', isDefault ? '当前默认' : '设为默认');
    defaultButton.type = 'button';
    defaultButton.disabled = isDefault;
    defaultButton.addEventListener('click', () => { void setDefaultModel(provider.id, model.id); });
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
    row.addEventListener('click', () => {
      modelSettingsActiveId = provider.id;
      modelSearch.value = '';
      renderModelSettings();
    });
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

function editorProviderFields() {
  const name = providerNameInput.value.trim();
  const apiBase = providerApiBaseInput.value.trim().replace(/\/$/, '');
  const apiKey = providerKeyInput.value;
  const models = providerModelsFromLines(providerModelsInput.value, providerEditorModels);
  return { name, api_base: apiBase, api_key: apiKey, clear_key: providerClearKey.checked, models };
}

async function discoverInProviderEditor() {
  const fields = editorProviderFields();
  if (!fields.api_base) {
    providerFormError.textContent = '请先填写 API Base。';
    return;
  }
  providerDiscover.disabled = true;
  providerFormError.textContent = '';
  try {
    const body = { api_base: fields.api_base };
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
    const result = await modelSettings.discover({ provider_id: provider.id, api_base: provider.api_base });
    const discovered = Array.isArray(result?.models) ? result.models.filter((model) => typeof model === 'string') : [];
    if (!discovered.length) throw new Error('供应商没有返回模型。');
    const existing = new Map(provider.models.map((model) => [model.id, model]));
    const models = [...new Set([...provider.models.map((model) => model.id), ...discovered])]
      .map((id) => ({ id, enabled: existing.get(id)?.enabled !== false,
        context_window_tokens: existing.get(id)?.context_window_tokens ?? null }));
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
  $('#model-dialog h2').textContent = model ? '模型上下文窗口' : '添加模型';
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
  let contextWindow;
  try { contextWindow = parseContextWindow($('#model-context-tokens').value); }
  catch (error) { modelFormError.textContent = error.message; return false; }
  if (!modelEditor.model_id && provider.models.some((model) => model.id === id)) {
    modelFormError.textContent = '该模型已经存在。';
    return false;
  }
  modelSave.disabled = true;
  const saved = await saveProviderRecord(provider, {
    revision: modelEditor.revision,
    models: modelEditor.model_id ? provider.models.map(model => model.id === modelEditor.model_id
      ? { ...model, context_window_tokens: contextWindow } : model)
      : [...provider.models, { id, enabled: true, context_window_tokens: contextWindow }],
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
    button.addEventListener('click', async () => {
      button.disabled = true;
      try {
        const changed = await setDefaultModel(item.provider.id, item.model.id);
        if (changed) modelPickerDialog.close();
        else modelPickerError.textContent = settingsNotice.textContent || '默认模型更新失败。';
      } catch (error) { modelPickerError.textContent = error?.message || '默认模型更新失败。'; }
      button.disabled = false;
    });
    modelPickerList.append(button);
  }
}

async function showModelPicker() {
  if (settingsTransport === 'http' && !modelSettingsState) await refreshModelSettings();
  renderModelPicker();
  modelPickerDialog.showModal();
}

function setBusy(busy) {
  send.disabled = starting || !client || !prompt.value.trim() || !conversations.canSend;
  conversations.controls();
  cancel.hidden = !busy;
  send.textContent = busy ? '+' : '↑';
  send.dataset.tooltip = busy ? '向当前任务追加指令' : '发送任务';
  send.setAttribute('aria-label', send.title);
}

function text(tag, className, value) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  node.textContent = value;
  return node;
}

function createTurn(userText) {
  welcome.hidden = true;
  const dom = new TurnView(userText, {
    readArtifact: (runId, uri, offset) => spillResults.readPage(runId, uri, offset),
  });
  timeline.append(dom.turn);
  main.scrollTo({ top: main.scrollHeight, behavior: 'smooth' });
  return dom;
}

function renderActive() {
  renderFrame = 0;
  if (!active) return;
  const nearBottom = main.scrollHeight - main.scrollTop - main.clientHeight < 120;
  const state = active.view.state;
  active.dom.update(state);
  if (state.outcome) {
    if (!active.done) {
      active.done = true;
      active.subscription?.close();
      clearInterval(workTimer);
      void conversations.settled();
    }
    setBusy(false);
  } else setBusy(true);
  if (nearBottom) main.scrollTo({ top: main.scrollHeight });
}

function scheduleRender() {
  if (renderFrame) return;
  renderFrame = requestAnimationFrame(renderActive);
}

async function subscribeRun(runId, view, dom) {
  const run = { runId, view, dom, subscription: null, done: false };
  active = run;
  clearInterval(workTimer);
  scheduleRender();
  if (view.state.outcome) return;
  const subscription = client.subscribe(runId, {
    after: view.state.seq,
    onFrame(frame) {
      if (active !== run) return;
      view.apply(frame);
      scheduleRender();
    },
  });
  run.subscription = subscription;
  workTimer = setInterval(() => dom.tick(), 1000);
  try {
    await subscription.closed;
    if (active !== run) return;
    if (!view.state.outcome) {
      const snapshot = await client.snapshot(runId);
      if (active !== run) return;
      view.apply({ kind: 'snapshot', reason: 'source_resync', snapshot });
      if (!snapshot.outcome) throw new Error('事件流提前结束，请刷新历史重新连接');
      scheduleRender();
    }
  } catch (error) {
    if (active !== run) return;
    clearInterval(workTimer);
    active = null;
    dom.fail(`连接中断：${error?.message || error}`);
    conversations.notice('连接中断不代表任务已结束。点击历史会话的“刷新”检查运行状态；不会自动重复执行。', true);
    setBusy(false);
  }
}

async function submitPrompt() {
  const value = prompt.value.trim();
  if (!value || !client || starting || !conversations.canSend) return;
  prompt.value = '';
  setBusy(Boolean(active && !active.done));
  if (active && !active.done) {
    const note = text('div', 'user-message supplement', value);
    active.dom.turn.append(note);
    try {
      await client.input(active.runId, { request_id: requestId(), text: value });
    } catch (error) {
      note.append(text('span', 'error-detail', `发送失败：${error?.message || error}`));
    }
    return;
  }

  if (!conversations.persistent) $('#thread-title').textContent = value.slice(0, 26) || '临时任务';
  const dom = createTurn(value);
  starting = true;
  setBusy(false);
  try {
    const request = { request_id: requestId(), prompt: value };
    const response = conversations.persistent ? await conversations.submit(value) : await client.start(request);
    starting = false;
    if (conversations.persistent && (response.reused || !response.live || !response.run_id)) {
      await conversations.open(response.session.id);
    } else {
      const view = new RunView(response.run_id);
      void subscribeRun(response.run_id, view, dom);
    }
  } catch (error) {
    starting = false;
    dom.fail(`启动失败：${error?.message || error}`);
    if (!prompt.value) prompt.value = value;
    setBusy(false);
  }
}

async function connectHttp(baseUrl, bearer) {
  clearModelSettings();
  const candidate = new HttpAgentClient({
    baseUrl,
    token: bearer,
    fetch: globalThis.fetch.bind(globalThis),
    maxEventBytes: 16 * 1024 * 1024,
  });
  const response = await fetch(baseUrl.replace(/\/$/, '') + '/v1/info', {
    headers: { Authorization: `Bearer ${bearer}` },
    credentials: 'omit',
    redirect: 'error',
    cache: 'no-store',
    signal: AbortSignal.timeout(12000),
  });
  if (!response.ok) throw new Error(`连接验证失败：HTTP ${response.status}`);
  const info = await response.json();
  if (info.protocol_version !== 2 || info.stream !== 'sse') throw new Error('服务未提供 Stream v2 / SSE。');
  client = candidate;
  settingsTransport = 'http';
  modelSettings.configure(baseUrl, bearer);
  capabilities.configure(baseUrl, bearer);
  spillResults.configure(baseUrl, bearer);
  setBusy(false);
  void refreshModelSettings({ silent: true });
  void refreshCapabilities({ silent: true });
  await conversations.configure(baseUrl, bearer);
}

async function connectTauri() {
  clearModelSettings();
  const native = globalThis.__MONA_TAURI__ ?? globalThis.__TAURI__?.core;
  if (!native?.invoke || !native?.Channel) throw new Error('当前页面不在已配置的 Tauri 宿主中。');
  client = new TauriAgentClient({ invoke: native.invoke, Channel: native.Channel });
  settingsTransport = 'tauri';
  renderCapabilities();
  renderModelSettings();
  conversations.ephemeral('当前 Tauri 宿主尚未装配会话管理；此连接仅支持临时任务。正式 Web 宿主支持本地保存。');
  setBusy(false);
}

async function autoConnect() {
  const native = globalThis.__MONA_TAURI__ ?? globalThis.__TAURI__?.core;
  if (native?.invoke && native?.Channel) {
    await connectTauri();
    return;
  }
  try {
    const response = await fetch('/__mona_dev_config__.json', { cache: 'no-store', credentials: 'same-origin' });
    if (!response.ok) throw new Error('no dev config');
    const config = await response.json();
    endpoint.value = config.endpoint;
    await connectHttp(config.endpoint, config.token);
  } catch {
    clearModelSettings();
  }
}

transport.addEventListener('change', () => { httpFields.hidden = transport.value !== 'http'; });
form.addEventListener('submit', async (event) => {
  event.preventDefault();
  connectionError.textContent = '';
  connectButton.disabled = true;
  try {
    if (starting || conversations.loading || (active && !active.done)) throw new Error('会话正在加载、保存或执行，请等待完成；运行中的任务需先停止并等待最终状态。');
    if (transport.value === 'tauri') await connectTauri();
    else await connectHttp(endpoint.value.trim().replace(/\/$/, ''), token.value);
    token.value = '';
    dialog.close();
  } catch (error) {
    connectionError.textContent = error?.message || String(error);
  } finally {
    connectButton.disabled = false;
  }
});

send.addEventListener('click', submitPrompt);
prompt.addEventListener('input', () => setBusy(Boolean(active && !active.done)));
prompt.addEventListener('keydown', (event) => {
  if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
    event.preventDefault();
    void submitPrompt();
  }
});
cancel.addEventListener('click', async () => {
  if (!active || active.done || !client) return;
  cancel.disabled = true;
  try {
    await client.cancel(active.runId);
  } catch (error) {
    active.dom.footer.textContent = `取消信号发送失败：${error?.message || error}`;
  } finally {
    cancel.disabled = false;
  }
});
$('#new-chat').addEventListener('click', () => {
  if (starting || (active && !active.done)) {
    alert('请先停止当前任务并等待最终状态。');
    return;
  }
  if (!conversations.newChat()) return;
  closeSidebar();
  prompt.focus();
});
for (const button of document.querySelectorAll('[data-prompt]')) {
  button.addEventListener('click', () => {
    prompt.value = button.dataset.prompt || '';
    prompt.focus();
    setBusy(false);
  });
}
$('#settings-button').addEventListener('click', () => showSettings());
componentsTab.addEventListener('click', () => selectSettingsSection('components'));
toolsTab.addEventListener('click', () => selectSettingsSection('tools'));
modelsTab.addEventListener('click', () => selectSettingsSection('models'));
appearanceTab.addEventListener('click', () => selectSettingsSection('appearance'));
componentsRefresh.addEventListener('click', () => { void refreshCapabilities(); });
toolsRefresh.addEventListener('click', () => { void refreshCapabilities(); });
const shell = $('.shell');
const sidebarToggle = $('#sidebar-toggle');
const sidebarScrim = $('#sidebar-scrim');
const sidebarResizer = $('#sidebar-resizer');
const mobileLayout = matchMedia('(max-width: 680px)');
function syncSidebar() {
  const open = mobileLayout.matches ? shell.classList.contains('sidebar-open') : !shell.classList.contains('sidebar-collapsed');
  chatSidebar.inert = !open;
  sidebarToggle.setAttribute('aria-expanded', String(open));
  sidebarScrim.hidden = !mobileLayout.matches || !open;
  chatWorkspace.inert = mobileLayout.matches && open;
}
function closeSidebar() {
  shell.classList.remove('sidebar-open');
  syncSidebar();
}
sidebarToggle.addEventListener('click', () => {
  shell.classList.toggle(mobileLayout.matches ? 'sidebar-open' : 'sidebar-collapsed');
  syncSidebar();
  if (mobileLayout.matches) $('#new-chat').focus();
});
sidebarScrim.addEventListener('click', () => { closeSidebar(); sidebarToggle.focus(); });
mobileLayout.addEventListener('change', closeSidebar);
// Sidebar width is a browser-local layout preference: dragged or arrow-keyed, applied at once,
// persisted on release. A storage failure keeps this session's width and never claims it was saved.
const SIDEBAR_WIDTH_KEY = 'mona.web.sidebar.v1';
const SIDEBAR_MIN_WIDTH = 208;
const SIDEBAR_MAX_WIDTH = 460;
const SIDEBAR_DEFAULT_WIDTH = 264;
const SIDEBAR_KEYBOARD_STEP = 16;
const clampSidebarWidth = value => Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, Math.round(value)));
function restoreSidebarWidth() {
  try {
    const saved = Number(localStorage.getItem(SIDEBAR_WIDTH_KEY));
    if (Number.isFinite(saved) && saved > 0) return clampSidebarWidth(saved);
  } catch { /* 存储被拒绝时使用响应式默认宽度 */ }
  return null;
}
let sidebarWidth = SIDEBAR_DEFAULT_WIDTH;
function renderedSidebarWidth() {
  return Math.round(chatSidebar.getBoundingClientRect().width) || sidebarWidth;
}
function applySidebarWidth(value) {
  sidebarWidth = clampSidebarWidth(value);
  shell.style.setProperty('--sidebar-width', `${sidebarWidth}px`);
  sidebarResizer.setAttribute('aria-valuenow', String(sidebarWidth));
}
function persistSidebarWidth() {
  try { localStorage.setItem(SIDEBAR_WIDTH_KEY, String(sidebarWidth)); }
  catch { /* 保存失败不回滚本次宽度，也不假装已保存 */ }
}
sidebarResizer.setAttribute('aria-valuemin', String(SIDEBAR_MIN_WIDTH));
sidebarResizer.setAttribute('aria-valuemax', String(SIDEBAR_MAX_WIDTH));
const storedSidebarWidth = restoreSidebarWidth();
if (storedSidebarWidth != null) applySidebarWidth(storedSidebarWidth);
else sidebarResizer.setAttribute('aria-valuenow', String(renderedSidebarWidth()));
let resizeGesture = null;
sidebarResizer.addEventListener('pointerdown', event => {
  if (mobileLayout.matches || shell.classList.contains('sidebar-collapsed')) return;
  resizeGesture = { pointer: event.pointerId, startX: event.clientX, startWidth: renderedSidebarWidth() };
  sidebarResizer.setPointerCapture(event.pointerId);
  shell.classList.add('sidebar-resizing');
  event.preventDefault();
});
sidebarResizer.addEventListener('pointermove', event => {
  if (!resizeGesture || event.pointerId !== resizeGesture.pointer) return;
  applySidebarWidth(resizeGesture.startWidth + event.clientX - resizeGesture.startX);
});
function endSidebarResize(event) {
  if (!resizeGesture || (event && event.pointerId !== resizeGesture.pointer)) return;
  resizeGesture = null;
  shell.classList.remove('sidebar-resizing');
  persistSidebarWidth();
}
sidebarResizer.addEventListener('pointerup', endSidebarResize);
sidebarResizer.addEventListener('pointercancel', endSidebarResize);
sidebarResizer.addEventListener('keydown', event => {
  const step = event.key === 'ArrowLeft' ? -SIDEBAR_KEYBOARD_STEP
    : event.key === 'ArrowRight' ? SIDEBAR_KEYBOARD_STEP : 0;
  if (!step) return;
  event.preventDefault();
  applySidebarWidth(renderedSidebarWidth() + step);
  persistSidebarWidth();
});
document.addEventListener('keydown', event => {
  if (event.key === 'Escape' && shell.classList.contains('sidebar-open')) { closeSidebar(); sidebarToggle.focus(); }
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'n' && !document.querySelector('dialog[open]')) {
    event.preventDefault(); $('#new-chat').click(); closeSidebar();
  }
});
syncSidebar();
$('#settings-back').addEventListener('click', hideSettings);
$('#model-button').addEventListener('click', () => { void showModelPicker(); });
providerSearch.addEventListener('input', renderModelSettings);
modelSearch.addEventListener('input', renderModelSettings);
settingsRefresh.addEventListener('click', () => { void refreshModelSettings(); });
addProvider.addEventListener('click', () => openProviderEditor());
editProvider.addEventListener('click', () => openProviderEditor(activeProvider()));
deleteProvider.addEventListener('click', () => { void deleteCurrentProvider(); });
discoverModels.addEventListener('click', () => { void discoverCurrentProvider(); });
addModel.addEventListener('click', () => openModelEditor());
toggleAllModels.addEventListener('click', () => {
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

providerDiscover.addEventListener('click', () => { void discoverInProviderEditor(); });
providerForm.addEventListener('submit', (event) => {
  event.preventDefault();
  void (async () => {
    const fields = editorProviderFields();
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
modelForm.addEventListener('submit', (event) => {
  event.preventDefault();
  void addManualModel().then((saved) => {
    if (!saved) modelFormError.textContent = settingsNotice.textContent || modelFormError.textContent || '保存失败，请重试。';
  });
});
openModelSettings.addEventListener('click', (event) => {
  event.preventDefault();
  modelPickerDialog.close();
  showSettings('models');
});
providerDialog.addEventListener('close', () => {
  providerKeyInput.value = '';
  providerClearKey.checked = false;
  providerFormError.textContent = '';
});
for (const [dialogElement, canDismiss] of [
  [dialog, () => !connectButton.disabled],
  [providerDialog, () => !providerSave.disabled],
  [modelDialog, () => !modelSave.disabled],
  [modelPickerDialog, () => true],
]) {
  for (const button of dialogElement.querySelectorAll('button[value="cancel"]')) {
    button.addEventListener('click', (event) => {
      event.preventDefault();
      if (canDismiss()) dialogElement.close('cancel');
    });
  }
  dialogElement.addEventListener('cancel', (event) => {
    if (!canDismiss()) event.preventDefault();
  });
  dialogElement.addEventListener('click', (event) => {
    if (event.target !== dialogElement || !canDismiss()) return;
    const rect = dialogElement.getBoundingClientRect();
    const inside = event.clientX >= rect.left && event.clientX <= rect.right
      && event.clientY >= rect.top && event.clientY <= rect.bottom;
    if (!inside) dialogElement.close('cancel');
  });
}

setBusy(false);
updateModelLabel();
renderCapabilities();
renderModelSettings();
void autoConnect();
