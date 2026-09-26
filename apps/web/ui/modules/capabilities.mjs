import { CapabilitiesClient } from '../../capabilities.mjs';
import { claim, scoped, element as text, setting } from '../dom.mjs';
export const version = 1;
export function mount(ctx) {
const roots = ['component-settings-section','tool-settings-section'].map(claim);
const $ = selector => scoped(roots)(selector) || document.querySelector(selector), on = ctx.listen;
const capabilities = new CapabilitiesClient(); let capabilitiesState = null, settingsTransport = 'none';
const componentSettingsSection = $('#component-settings-section');
const toolSettingsSection = $('#tool-settings-section');
const componentList = $('#component-list');
const toolList = $('#tool-list');
const componentsNotice = $('#components-notice');
const toolsNotice = $('#tools-notice');
const componentsRefresh = $('#components-refresh');
const toolsRefresh = $('#tools-refresh');
const componentsTab = $('#settings-components-tab');
const toolsTab = $('#settings-tools-tab');
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
    void ctx.refreshManifest().catch(()=>{});
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
    toggle.addEventListener('click', ctx.guard(async () => {
      toggle.disabled = true;
      await setCapabilityEnabled(item.id, !item.enabled);
    }));
    actions.append(toggle);
    row.append(copy, actions);
    list.append(row);
  }
}

function renderCapabilities() {
  renderCapabilityGroup(componentList, capabilitiesState?.components || [], 'components');
  renderCapabilityGroup(toolList, capabilitiesState?.tools || [], 'tools');
}


on(componentsRefresh,'click',()=>{void refreshCapabilities();}); on(toolsRefresh,'click',()=>{void refreshCapabilities();});
for(const [id,root,order] of [['components',roots[0],10],['tools',roots[1],20]])setting(ctx,id,root,()=>{renderCapabilities();if(!capabilitiesState)void refreshCapabilities();},order);
ctx.on('connection',async connection=>{settingsTransport=connection?'http':'none';if(connection){capabilities.configure(connection.base,connection.bearer);await refreshCapabilities({silent:true});}else capabilities.clear();});
ctx.own(()=>{capabilities.clear();capabilitiesState=null;componentList.replaceChildren();toolList.replaceChildren();});
renderCapabilities();
}
