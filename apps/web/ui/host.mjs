import { runCommand } from './commands.mjs';
import { uiRegistry, UI_VERSION } from './registry.mjs';
import { ProductClient } from './client.mjs';

// Static catalog owns import paths. The host's manifest contains IDs, never executable URLs.
export class UiHost {
  constructor({ catalog, shell, registry = uiRegistry, onError = () => {} }) {
    this.catalog = catalog; this.shell = shell; this.registry = registry; this.onError = onError;
    this.api = new ProductClient(); this.modules = new Map(); this.services = new Map(); this.listeners = new Map();
    this.generation = 0; this.reconcileEpoch = 0; this.manifestRead = 0; this.connection = null; this.manifest = null; this.locks = new Set();
  }
  service(id) { return this.services.get(id)?.value; }
  report(owner, error) {
    if (error?.name === 'AbortError') return;
    try { this.onError(`${owner}: ${error?.message || '模块操作失败'}`); }
    catch { /* Error presentation cannot interrupt another module's cleanup. */ }
  }
  emit(event, value) {
    for (const entry of [...(this.listeners.get(event) || [])]) {
      if (entry.signal.aborted) continue;
      try { Promise.resolve(entry.fn(value)).catch(e => { if (!entry.signal.aborted) this.report(entry.owner, e); }); } catch (e) { this.report(entry.owner, e); }
    }
  }
  async broadcast(event, value, generation = this.generation, owner = null) {
    for (const entry of [...(this.listeners.get(event) || [])]) {
      if (generation !== this.generation) return;
      if (entry.signal.aborted || (owner && entry.owner !== owner)) continue;
      try { await entry.fn(value); } catch (error) { if (!entry.signal.aborted) this.report(entry.owner, error); }
    }
  }
  async initialize(generation = this.generation) {
    // Replaying only to newly connected owners avoids resetting existing editors or views.
    const connection = this.connection;
    for (const [id, record] of this.modules) {
      if (generation !== this.generation || !connection) return;
      if (!record.mounted || record.controller.signal.aborted) continue;
      if (record.connection !== connection) {
        record.connection = connection;
        record.connecting = (async () => {
          await this.broadcast('connection', connection, generation, id);
          if (generation === this.generation && this.modules.get(id) === record) {
            await this.broadcast('session', this.shell.session?.() || null, generation, id);
          }
        })();
      }
      await record.connecting;
    }
  }
  async init() { await this.reconcile([]); }
  async connect(base, bearer) {
    this.disconnect(); const generation = this.generation;
    this.connection = Object.freeze({ base, bearer }); this.api.configure(base, bearer);
    try { const manifest = await this.api.manifest(); if (generation !== this.generation) return;
      this.manifest = manifest; await this.reconcile(manifest.modules, generation);
    } catch (error) { if (generation === this.generation && error.status !== 404) this.report('UI 装配', error); }
    if (generation === this.generation) await this.initialize(generation);
  }
  async refreshManifest() {
    if (!this.connection) return;
    const generation = this.generation, read = ++this.manifestRead;
    const manifest = await this.api.manifest(); if (generation !== this.generation || read !== this.manifestRead) return;
    this.manifest = manifest; await this.reconcile(manifest.modules, generation);
    if (generation !== this.generation || read !== this.manifestRead) return;
    await this.initialize(generation);
    if (generation === this.generation && read === this.manifestRead) this.emit('assembly', manifest);
  }
  disconnect() {
    this.generation++; this.reconcileEpoch++; this.manifestRead++; this.api.clear(); this.connection = null; this.manifest = null;
    for (const [id, record] of [...this.modules].reverse()) {
      if (!record.local) this.unmount(id);
      else { record.connection = null; record.connecting = null; }
    }
    this.emit('connection', null); this.locks.clear();
  }
  unmount(id, expected = this.modules.get(id)) {
    const record = this.modules.get(id); if (!record || record !== expected) return;
    this.modules.delete(id); record.controller.abort();
    for (const cleanup of [...record.cleanups].reverse()) {
      try { Promise.resolve(cleanup()).catch(error => this.report(id, error)); } catch (error) { this.report(id, error); }
    }
    try { this.registry.removeOwner(id); } catch (error) { this.report(id, error); }
    try { this.shell.releaseOwner?.(id); } catch (error) { this.report(id, error); }
    for (const [key, service] of this.services) if (service.owner === id) this.services.delete(key);
  }
  async reconcile(ids, generation = this.generation) {
    const epoch = ++this.reconcileEpoch;
    const isCurrent = () => generation === this.generation && epoch === this.reconcileEpoch;
    const wanted = new Set([...Object.keys(this.catalog).filter(id => this.catalog[id].local), ...ids]);
    for (const id of wanted) if (!Object.hasOwn(this.catalog, id)) { wanted.delete(id); this.report(id, new Error('此构建未包含对应 UI 模块。')); }
    // A surviving child cannot retain services from a withdrawn parent.
    let removed;
    do { removed = false;
      for (const id of wanted) {
        const missing = (this.catalog[id].requires || []).find(dependency => !wanted.has(dependency));
        if (missing) { wanted.delete(id); removed = true; this.report(id, new Error(`缺少 UI 依赖 ${missing}`)); }
      }
    } while (removed);
    for (const [id] of [...this.modules].reverse()) if (!wanted.has(id)) this.unmount(id);
    const visiting = new Set();
    const mount = async id => {
      if (!isCurrent()) return;
      const existing = this.modules.get(id);
      if (existing) { await existing.ready; return; }
      if (visiting.has(id)) throw new Error('UI 模块依赖循环。'); visiting.add(id);
      const definition = this.catalog[id];
      for (const dependency of definition.requires || []) {
        if (!wanted.has(dependency)) throw new Error(`缺少 UI 依赖 ${dependency}`);
        await mount(dependency);
        if (!isCurrent()) return;
        if (!this.modules.get(dependency)?.mounted) throw new Error(`UI 依赖不可用 ${dependency}`);
      }
      const module = await definition.load();
      if (!isCurrent() || this.modules.has(id)) return;
      if (module.version !== UI_VERSION || typeof module.mount !== 'function') throw new Error('UI 模块契约版本不匹配。');
      const record = { local: definition.local === true, controller: new AbortController(), cleanups: [], mounted: false }; this.modules.set(id, record);
      const current = () => !record.controller.signal.aborted && this.modules.get(id) === record;
      const requireCurrent = () => { if (!current()) throw new DOMException('模块已卸载。', 'AbortError'); };
      const own = cleanup => {
        if (typeof cleanup !== 'function') throw new Error('UI 资源必须提供释放函数。');
        if (!current()) {
          Promise.resolve().then(cleanup).catch(error => this.report(id, error));
          throw new DOMException('模块已卸载。', 'AbortError');
        }
        let released = false;
        const release = () => { if (released) return; released = true; const index = record.cleanups.indexOf(release); if (index >= 0) record.cleanups.splice(index, 1); return cleanup(); };
        record.cleanups.push(release); return release;
      };
      const context = {
        id, signal: record.controller.signal, shell: this.shell,
        connection: () => current() ? this.connection : null,
        capability: key => current() ? this.manifest?.capabilities[key] || null : null,
        own, listen: (target, type, fn, options = {}) => { requireCurrent(); if (!target) throw new Error(`UI 节点不存在：${id}/${type}`); target.addEventListener(type, fn, { ...options, signal: record.controller.signal }); },
        register: (slot, contribution) => { if (!current()) throw new DOMException('模块已卸载。', 'AbortError'); return own(this.registry.register(id, slot, contribution)); },
        on: (event, fn) => { requireCurrent(); let entries = this.listeners.get(event); if (!entries) this.listeners.set(event, entries = new Set());
          const entry = { owner: id, fn, signal: record.controller.signal }; entries.add(entry);
          return own(() => { entries.delete(entry); if (!entries.size) this.listeners.delete(event); }); },
        emit: (event, value) => { if (current()) this.emit(event, value); },
        provide: (key, value) => { if (!current() || this.services.has(key)) throw new Error(`UI 服务重复或已失效：${key}`); this.services.set(key, { owner: id, value }); own(() => { if (this.services.get(key)?.owner === id) this.services.delete(key); }); },
        service: key => current() ? this.service(key) : undefined,
        refreshManifest: () => { requireCurrent(); return this.refreshManifest(); },
        refreshCommands: () => { if (current()) this.registry.notify('composer.commands'); },
        guard: fn => async (...args) => { if (!current()) return; try { return await fn(...args); } catch (error) { if (current()) this.report(id, error); } },
        lockComposer: () => { requireCurrent(); const token = {}; this.locks.add(token); this.shell.busyChanged?.(); const release = () => { this.locks.delete(token); this.shell.busyChanged?.(); }; return own(release); },
      };
      const aborted = new Promise(resolve => record.controller.signal.addEventListener('abort', resolve, { once: true }));
      const mounting = Promise.resolve().then(async () => {
        requireCurrent();
        const cleanup = await module.mount(context); if (cleanup) own(cleanup);
        if (current()) record.mounted = true;
      }).catch(error => {
        const owned = current(); this.unmount(id, record);
        if (owned) this.report(id, error);
      });
      // Other reconcilers await this same mount, not merely its registry placeholder.
      // Detaching the owner releases waiters even if a companion ignores cancellation.
      record.ready = Promise.race([mounting, aborted]);
      try { await record.ready; } finally { visiting.delete(id); }
    };
    for (const id of wanted) { try { await mount(id); } catch (e) { visiting.clear(); this.report(id, e); } }
  }
  presentation(session) {
    const result = {};
    for (const entry of this.registry.entries('presentation')) {
      try { Object.assign(result, entry.value(session) || {}); } catch (error) { this.report(entry.owner, error); }
    }
    return result;
  }
  async command(text) {
    const name = /^\/(\S+)(?:\s|$)/.exec(text)?.[1]; if (!name) return false;
    const entry = this.registry.entries('composer.commands').find(entry => entry.id === name);
    if (!entry) return false;
    await runCommand(entry.value, text.replace(/^\/\S+\s*/, '')); return true;
  }
  dispose() { this.disconnect(); for (const id of [...this.modules.keys()].reverse()) this.unmount(id); }
}
