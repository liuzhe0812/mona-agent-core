// Browser-only regression helpers. Uses actual DOM/modules with isolated state, never a paid model.
import { UiHost } from '../ui/host.mjs';
import { UiRegistry, uiRegistry } from '../ui/registry.mjs';
import * as planner from '../ui/modules/planner.mjs';

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const deferred = () => { let resolve; const promise = new Promise(r => { resolve = r; }); return { promise, resolve }; };
async function waitFor(test, label) {
  const deadline = performance.now() + 5000;
  while (performance.now() < deadline) { if (test()) return; await sleep(10); }
  throw Error('UI lifecycle timeout: ' + label);
}
export async function runUiModuleChecks() {
  const checks = [], check = (name, condition) => { if (!condition) throw Error(name); checks.push(name); };
  const disposers = [];
  const register = (slot, contribution) => { const dispose = uiRegistry.register('lifecycle-check', slot, contribution); disposers.push(dispose); return dispose; };
  try {
    register('composer.actions', { id: 'lifecycle.bad', order: -20, value: {} });
    const control = document.createElement('button'); control.type = 'button'; control.textContent = '检查控件';
    register('composer.actions', { id: 'lifecycle.good', order: -10, value: control });
    check('a malformed slot contribution does not hide valid controls in the same slot', control.isConnected && document.querySelector('#sessions-notice')?.textContent.includes('lifecycle.bad'));
    const setting = name => {
      const tab = document.createElement('button'), panel = document.createElement('main');
      tab.type = 'button'; tab.textContent = name; tab.dataset.lifecycleSetting = name;
      panel.className = 'settings-content'; panel.textContent = 'temporary view';
      return { tab, panel };
    };
    const later = setting('later'), earlier = setting('earlier');
    const removeLater = register('settings', { id: 'lifecycle.later', order: 800, value: later });
    const removeEarlier = register('settings', { id: 'lifecycle.earlier', order: 700, value: earlier });
    register('settings', { id: 'lifecycle.invalid', order: 600, value: {} });
    const order = [...document.querySelectorAll('.settings-sidebar nav [data-lifecycle-setting]')].map(n => n.dataset.lifecycleSetting);
    check('settings use declared ordering independently of module arrival and reject malformed neighbors', order.join(',') === 'earlier,later' && earlier.panel.isConnected);
    removeEarlier(); removeLater();
    check('unregistered settings remove their owned DOM rather than retaining hidden pages', !earlier.tab.isConnected && !earlier.panel.isConnected && !later.tab.isConnected);
    let observedScope;
    register('right.pane', { id: 'lifecycle.scope', value: { label: '作用域检查', icon: 'plan', available: scope => { observedScope = scope; return scope.startsWith('session:'); }, run() {} } });
    check('right-pane companion callbacks receive the original scope arguments', typeof observedScope === 'string');
  } finally {
    for (const dispose of disposers.reverse()) dispose();
  }

  // The real Planner module runs under an independent UiHost. Only these two synthetic IDs are intercepted.
  const root = document.createElement('section'); root.hidden = true; document.body.append(root);
  const savedFetch = globalThis.fetch, registry = new UiRegistry(), mutations = [], submissions = [], gates = [];
  const state = (id, mode, proposal) => ({ session_id: id, revision: 3, status: 'completed', enabled: true, plan: {
    version: 1, revision: 2, mode, goal: id, steps: [{ id: 'step', text: id, status: 'pending' }], explanation: null, proposal,
  } });
  const states = new Map([['lifecycle-a', state('lifecycle-a', 'plan_only', '# Plan A\nOnly A')], ['lifecycle-b', state('lifecycle-b', 'normal', null)]]);
  let selected = { id: 'lifecycle-a', revision: 3, status: 'completed' }, draft = '', reads = 0, admissions = 0, admissionGate = null, readGate = null;
  const panelTabs = new Map();
  const pane = {
    renderActions() {},
    openTab(options) { panelTabs.set(options.scope, options); if (!options.node.isConnected) root.append(options.node); return options; },
  };
  const host = new UiHost({ registry, catalog: { planner: { load: async () => planner } }, shell: {
    session: () => selected, draft: () => draft, pane: () => pane, presentation: () => ({}), focusPrompt() {}, busyChanged() {},
    ensureSession: async () => { admissions++; return admissionGate ? admissionGate.promise : selected; },
    refreshSessionHeader: async id => { if (selected.id === id) { selected = { ...selected, revision: states.get(id).revision }; host.emit('session', selected); } },
    submitText: async (text, id) => { submissions.push({ text, id }); },
    releaseOwner: () => { for (const tab of panelTabs.values()) { tab.onClose?.(); tab.node.remove(); } panelTabs.clear(); },
  } });
  host.api.manifest = async () => ({ version: 1, modules: ['planner'], capabilities: { planner: { active: true } } });
  globalThis.fetch = async (url, options) => {
    const id = /^\/api\/sessions\/(lifecycle-[ab])\/plan$/.exec(new URL(url, location.href).pathname)?.[1];
    if (!id) return savedFetch(url, options);
    if (!options.body) {
      reads++; const snapshot = structuredClone(states.get(id)); if (readGate) await readGate.promise;
      return new Response(JSON.stringify(snapshot));
    }
    const body = JSON.parse(options.body), old = states.get(id); mutations.push({ id, body });
    if (body.revision !== old.revision || body.plan_revision !== old.plan.revision) return new Response(JSON.stringify({ message: 'revision conflict' }), { status: 409 });
    const next = structuredClone(old); next.revision++; next.plan.revision++;
    next.plan.mode = ['plan', 'refine'].includes(body.action) ? 'plan_only' : 'normal';
    if (['plan', 'refine'].includes(body.action)) next.plan.proposal = null;
    states.set(id, next); return new Response(JSON.stringify(next));
  };
  try {
    await host.connect(location.origin, 'isolated-ui-test');
    const chip = registry.resolve('composer.mode', 'planner');
    const command = registry.resolve('composer.commands', 'plan');
    const notice = registry.resolve('composer.dock', 'planner').querySelector('.planner-notice');
    await waitFor(() => !chip.hidden && !chip.disabled, 'initial plan');
    await registry.resolve('right.pane', 'plan').run();
    const resume = [...root.querySelectorAll('button')].find(n => n.textContent === '按此计划执行');
    draft = 'retained user draft'; resume.click(); await sleep(30);
    check('a retained draft prevents resume from changing the persisted mode', mutations.length === 0 && states.get('lifecycle-a').plan.mode === 'plan_only');
    draft = ''; selected = { id: 'lifecycle-b', revision: 3, status: 'completed' }; host.emit('session', selected);
    await waitFor(() => chip.hidden, 'replacement session');
    resume.click(); await sleep(30);
    check('an old pane action cannot switch or execute the newly selected session', mutations.length === 0 && submissions.length === 0);
    admissionGate = deferred(); gates.push(admissionGate);
    const beforeAdmissions = admissions;
    for (let i = 0; i < 2; i++) void command.run('');
    await sleep(20); const singleAdmission = admissions === beforeAdmissions + 1 && host.locks.size === 1;
    admissionGate.resolve(selected); admissionGate = null;
    await waitFor(() => !chip.hidden && !chip.disabled && host.locks.size === 0, 'single mode mutation');
    check('double mode clicks share one admission and cannot issue duplicate mutations', singleAdmission && mutations.length === 1);
    let beforeReads = reads;
    const stream = setInterval(() => host.emit('run', { run_id: 'controlled-stream' }), 20);
    try { await sleep(850); } finally { clearInterval(stream); }
    check('continuous model output refreshes plans before the stream ends', reads > beforeReads);
    await sleep(400);
    readGate = deferred(); gates.push(readGate); beforeReads = reads;
    for (let i = 0; i < 10; i++) host.emit('session', { ...selected, revision: selected.revision + 1 });
    await sleep(30); const sharedRead = reads === beforeReads + 1;
    readGate.resolve(); readGate = null; await sleep(30);
    check('repeated session notifications share an in-flight plan read without starving it', sharedRead);
    const revised = states.get('lifecycle-a'); revised.revision = 6; revised.plan.revision = 5; revised.plan.proposal = '# Revised A\nNew exact plan';
    selected = { id: 'lifecycle-a', revision: 6, status: 'completed' }; host.emit('session', selected);
    await waitFor(() => root.textContent.includes('New exact plan') && !chip.hidden && !chip.disabled, 'new displayed revision');
    resume.click(); await waitFor(() => notice.textContent.includes('已更新') && !chip.disabled, 'stale plan rejected');
    check('a queued pane action sends its displayed revision rather than silently approving a replacement plan', mutations.at(-1).id === 'lifecycle-a' && mutations.at(-1).body.plan_revision === 2 && states.get('lifecycle-a').plan.mode === 'plan_only' && submissions.length === 0);
  } finally {
    for (const gate of gates) gate.resolve();
    host.dispose(); globalThis.fetch = savedFetch; root.remove();
  }
  return checks;
}
