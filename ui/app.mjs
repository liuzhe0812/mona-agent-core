import { RunView, requestId } from '../clients/javascript/src/index.mjs';
import { HttpAgentClient } from '../clients/javascript/src/http.mjs';
import { TauriAgentClient } from '../clients/javascript/src/tauri.mjs';
import { ModelSettingsClient, createProviderId } from './model-settings.mjs';

const $ = (selector, root = document) => root.querySelector(selector);
const timeline = $('#timeline');
const welcome = $('#welcome');
const prompt = $('#prompt');
const send = $('#send');
const cancel = $('#cancel');
const connectionButton = $('#connection-button');
const connectionLabel = $('#connection-label');
const statusDot = $('#status-dot');
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
let connectionMode = 'none';
const modelSettings = new ModelSettingsClient();
let modelSettingsState = null;
let modelSettingsActiveId = null;
let settingsTransport = 'none';
let providerEditorId = null;
let providerEditorRevision = 0;
let providerEditorModels = [];

function normalizeModelSettings(value) {
  const providers = Array.isArray(value?.providers) ? value.providers.map((provider) => ({
    id: typeof provider?.id === 'string' ? provider.id : '',
    name: typeof provider?.name === 'string' ? provider.name : '',
    api_base: typeof provider?.api_base === 'string' ? provider.api_base : '',
    has_key: provider?.has_key === true,
    builtin: provider?.builtin === true,
    models: Array.isArray(provider?.models) ? provider.models
      .map((model) => ({ id: typeof model?.id === 'string' ? model.id : '', enabled: model?.enabled !== false }))
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
  modelSettings.clear();
  modelSettingsState = null;
  modelSettingsActiveId = null;
  settingsTransport = 'none';
  setSettingsNotice('');
  updateModelLabel();
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
    setSettingsNotice(`${error.message || '设置已发生变化。'} 已刷新，请重试。`, 'error');
  } else {
    setSettingsNotice(error?.message || '模型设置操作失败。', 'error');
  }
}

function providerModelsFromLines(lines, previousModels = []) {
  const previous = new Map(previousModels.map((model) => [model.id, model.enabled !== false]));
  const ids = [...new Set(String(lines || '').split(/\r?\n/).map((value) => value.trim()).filter(Boolean))];
  return ids.map((id) => ({ id, enabled: previous.has(id) ? previous.get(id) : true }));
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

function showSettings() {
  chatSidebar.hidden = true;
  chatWorkspace.hidden = true;
  settingsPage.hidden = false;
  renderModelSettings();
  if (settingsTransport === 'http' && !modelSettingsState) void refreshModelSettings();
  modelSearch.focus();
}

function hideSettings() {
  settingsPage.hidden = true;
  chatSidebar.hidden = false;
  chatWorkspace.hidden = false;
  prompt.focus();
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
  control.title = isDefault ? '默认模型始终在对话中显示' : (enabled ? '不在对话中显示' : '在对话中显示');
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
  deleteProvider.title = provider.builtin ? '内置供应商不可删除' : '';
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
    const existing = new Map(provider.models.map((model) => [model.id, model.enabled !== false]));
    const models = [...new Set([...provider.models.map((model) => model.id), ...discovered])]
      .map((id) => ({ id, enabled: existing.has(id) ? existing.get(id) : true }));
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

function openModelEditor() {
  if (!activeProvider() || !modelSettingsState) {
    setSettingsNotice('请先添加或选择供应商。', 'error');
    return;
  }
  modelIdInput.value = '';
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
  if (provider.models.some((model) => model.id === id)) {
    modelFormError.textContent = '该模型已经存在。';
    return false;
  }
  modelSave.disabled = true;
  const saved = await saveProviderRecord(provider, {
    models: [...provider.models, { id, enabled: true }],
  }, '模型已添加');
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

function setConnection(label, mode = 'none') {
  connectionMode = mode;
  connectionLabel.textContent = label;
  statusDot.dataset.connected = mode === 'http' || mode === 'tauri' ? 'true' : 'false';
}

function setBusy(busy) {
  send.disabled = !client || !prompt.value.trim();
  cancel.hidden = !busy;
  send.textContent = busy ? '+' : '↑';
  send.title = busy ? '向当前任务追加指令' : '发送任务';
}

function text(tag, className, value) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  node.textContent = value;
  return node;
}

function statusText(status) {
  return ({ pending: '等待', running: '执行中', completed: '完成', failed: '失败', cancelled: '已取消', denied: '已拒绝', skipped: '已跳过', unknown: '状态未知' })[status] || status;
}

function outcomeText(outcome) {
  return ({ completed: '任务完成', failed: '任务失败', cancelled: '任务已取消', timed_out: '任务超时', limited: '达到运行限制' })[outcome.status] || outcome.status;
}

function createTurn(userText) {
  welcome.hidden = true;
  const turn = document.createElement('article');
  turn.className = 'turn';
  const user = text('div', 'user-message', userText);
  const body = document.createElement('div');
  body.className = 'assistant-body';
  const items = document.createElement('div');
  items.className = 'items';
  const footer = text('div', 'run-footer', '正在启动…');
  body.append(items, footer);
  turn.append(user, body);
  timeline.append(turn);
  main.scrollTo({ top: main.scrollHeight, behavior: 'smooth' });
  return { turn, items, footer };
}

function renderTool(item) {
  const card = document.createElement('details');
  card.className = `tool-card state-${item.state}`;
  if (item.state === 'running' || item.state === 'failed' || item.state === 'unknown') card.open = true;
  const summary = document.createElement('summary');
  summary.append(text('span', 'tool-icon', '⌘'), text('strong', '', item.content.name || 'tool'), text('span', 'tool-state', statusText(item.state)));
  card.append(summary);

  if (item.content.arguments_text) {
    const section = document.createElement('section');
    section.append(text('label', '', '参数'), text('pre', '', item.content.arguments_text));
    card.append(section);
  }
  if (item.content.output) {
    const section = document.createElement('section');
    section.append(text('label', '', '执行输出'), text('pre', '', item.content.output));
    card.append(section);
  }
  if (item.content.result) {
    const result = item.content.result;
    const section = document.createElement('section');
    section.append(text('label', '', `结果 · ${result.status}`), text('pre', '', result.content || '无文本结果'));
    card.append(section);
  }
  return card;
}

function renderActive() {
  renderFrame = 0;
  if (!active) return;
  const state = active.view.state;
  active.dom.items.replaceChildren();
  for (const item of state.items) {
    if (item.content.kind === 'agent_message') {
      const block = document.createElement('div');
      block.className = `assistant-message state-${item.state}`;
      block.append(text('div', 'assistant-mark', 'M'), text('div', 'message-text', item.content.text || (item.state === 'running' ? '…' : '')));
      active.dom.items.append(block);
    } else {
      active.dom.items.append(renderTool(item));
    }
  }
  if (state.pruned_items) {
    active.dom.items.append(text('p', 'muted-note', `较早的 ${state.pruned_items} 个显示项已从 UI 视图裁剪。`));
  }
  if (state.outcome) {
    const outcome = state.outcome;
    active.dom.footer.className = `run-footer outcome-${outcome.status}`;
    active.dom.footer.textContent = `${outcomeText(outcome)} · ${outcome.steps} 步 · ${outcome.task_usage.model_calls} 次模型请求`;
    if (outcome.error) active.dom.footer.append(text('span', 'error-detail', ` · ${outcome.error.message}`));
    active.done = true;
    active.subscription?.close();
    setBusy(false);
  } else {
    active.dom.footer.className = 'run-footer';
    active.dom.footer.textContent = state.started ? `第 ${state.step || 1} 步执行中` : '正在启动…';
    setBusy(true);
  }
  main.scrollTo({ top: main.scrollHeight, behavior: 'smooth' });
}

function scheduleRender() {
  if (renderFrame) return;
  renderFrame = requestAnimationFrame(renderActive);
}

async function subscribeRun(runId, view, dom) {
  const subscription = client.subscribe(runId, {
    after: 0,
    onFrame(frame) {
      view.apply(frame);
      scheduleRender();
    },
  });
  active = { runId, view, dom, subscription, done: false };
  try {
    await subscription.closed;
    if (!active?.done) {
      const snapshot = await client.snapshot(runId);
      view.apply({ kind: 'snapshot', reason: 'source_resync', snapshot });
      scheduleRender();
    }
  } catch (error) {
    dom.footer.className = 'run-footer outcome-failed';
    dom.footer.textContent = `连接中断：${error?.message || error}`;
    setBusy(false);
  }
}

async function submitPrompt() {
  const value = prompt.value.trim();
  if (!value || !client) return;
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

  $('#thread-title').textContent = value.slice(0, 26) || '新对话';
  const dom = createTurn(value);
  try {
    const request = { request_id: requestId(), prompt: value };
    const response = await client.start(request);
    const view = new RunView(response.run_id);
    void subscribeRun(response.run_id, view, dom);
  } catch (error) {
    dom.footer.className = 'run-footer outcome-failed';
    dom.footer.textContent = `启动失败：${error?.message || error}`;
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
  setConnection(`HTTP · ${new URL(baseUrl).host}`, 'http');
  setBusy(false);
  void refreshModelSettings({ silent: true });
}

async function connectTauri() {
  clearModelSettings();
  const native = globalThis.__MONA_TAURI__ ?? globalThis.__TAURI__?.core;
  if (!native?.invoke || !native?.Channel) throw new Error('当前页面不在已配置的 Tauri 宿主中。');
  client = new TauriAgentClient({ invoke: native.invoke, Channel: native.Channel });
  settingsTransport = 'tauri';
  renderModelSettings();
  setConnection('Tauri · 本机 Runtime', 'tauri');
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
    setConnection('未连接', 'none');
  }
}

connectionButton.addEventListener('click', () => {
  connectionError.textContent = '';
  transport.value = connectionMode === 'tauri' ? 'tauri' : 'http';
  httpFields.hidden = transport.value !== 'http';
  dialog.showModal();
});
transport.addEventListener('change', () => { httpFields.hidden = transport.value !== 'http'; });
form.addEventListener('submit', async (event) => {
  event.preventDefault();
  connectionError.textContent = '';
  connectButton.disabled = true;
  try {
    if (active && !active.done) throw new Error('当前任务仍在运行，请先停止并等待最终状态。');
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
  if (active && !active.done) {
    alert('请先停止当前任务并等待最终状态。');
    return;
  }
  timeline.replaceChildren();
  welcome.hidden = false;
  $('#thread-title').textContent = '新对话';
  active = null;
  prompt.focus();
});
for (const button of document.querySelectorAll('[data-prompt]')) {
  button.addEventListener('click', () => {
    prompt.value = button.dataset.prompt || '';
    prompt.focus();
    setBusy(false);
  });
}
$('#theme-toggle').addEventListener('click', () => {
  const dark = document.documentElement.dataset.theme === 'dark';
  document.documentElement.dataset.theme = dark ? 'light' : 'dark';
});
$('#settings-button').addEventListener('click', showSettings);
$('#settings-back').addEventListener('click', hideSettings);
$('#model-button').addEventListener('click', () => { void showModelPicker(); });
providerSearch.addEventListener('input', renderModelSettings);
modelSearch.addEventListener('input', renderModelSettings);
settingsRefresh.addEventListener('click', () => { void refreshModelSettings(); });
addProvider.addEventListener('click', () => openProviderEditor());
editProvider.addEventListener('click', () => openProviderEditor(activeProvider()));
deleteProvider.addEventListener('click', () => { void deleteCurrentProvider(); });
discoverModels.addEventListener('click', () => { void discoverCurrentProvider(); });
addModel.addEventListener('click', openModelEditor);
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
  showSettings();
});
providerDialog.addEventListener('close', () => {
  providerKeyInput.value = '';
  providerClearKey.checked = false;
  providerFormError.textContent = '';
});
for (const [dialogElement, formElement] of [[providerDialog, providerForm], [modelDialog, modelForm], [modelPickerDialog, $('#model-picker-form')]]) {
  for (const button of dialogElement.querySelectorAll('button[value="cancel"]')) {
    button.addEventListener('click', (event) => {
      if (button.type === 'button') event.preventDefault();
      dialogElement.close();
    });
  }
}

setBusy(false);
updateModelLabel();
renderModelSettings();
void autoConnect();
