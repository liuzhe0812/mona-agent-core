import { element as el, button } from './content-dom.mjs';
import { PaneMenu } from './pane-controls.mjs';

const number = value => Number.isSafeInteger(value) && value >= 0;
const countFields = ['revision', 'turns', 'steps', 'latest_step', 'reported_tokens', 'model_calls', 'input_tokens', 'output_tokens', 'model_time_ms', 'tool_time_ms', 'ttft_ms', 'ttft_samples', 'decode_ms', 'decode_tokens'];
const contextFields = ['tokens', 'capacity', 'system_tokens', 'tool_tokens', 'message_tokens'];

export function validateStatistics(v, id) {
  if (!v || v.session_id !== id || !countFields.every(key => number(v[key])) || v.latest_step > v.steps || typeof v.active !== 'boolean' || typeof v.usage_complete !== 'boolean') throw new Error('统计响应不完整。');
  if (v.latest_run_id != null && (typeof v.latest_run_id !== 'string' || v.latest_run_id.length > 256)) throw new Error('统计运行身份无效。');
  for (const key of ['cache_read_tokens', 'cache_write_tokens']) if (v[key] != null && !number(v[key])) throw new Error('缓存用量无效。');
  const c = v.context;
  if (c && (c.run_id !== v.latest_run_id || !number(c.observed_at) || typeof c.provider_anchored !== 'boolean' || contextFields.some(key => c[key] != null && !number(c[key])) || (c.capacity != null && c.capacity === 0))) throw new Error('上下文统计无效。');
  const cacheComplete = v.cache_read_tokens != null && v.cache_write_tokens != null && v.cache_read_tokens + v.cache_write_tokens <= v.input_tokens;
  const cacheHit = cachePercent(v.input_tokens, v.cache_read_tokens, v.cache_write_tokens);
  return {
    ...v,
    cache_complete: cacheComplete,
    uncached_input_tokens: cacheComplete ? v.input_tokens - v.cache_read_tokens - v.cache_write_tokens : null,
    cache_hit_percent: cacheComplete ? cacheHit : null,
    tokens_per_second: v.decode_ms > 0 && v.decode_tokens > 0 ? v.decode_tokens * 1000 / v.decode_ms : null,
    average_ttft_ms: v.ttft_samples > 0 ? v.ttft_ms / v.ttft_samples : null,
  };
}

export function tokenLabel(n) { return n == null ? '—' : n >= 1e6 ? `${(n / 1e6).toFixed(n < 1e7 ? 1 : 0)}M` : n >= 10000 ? `${Math.round(n / 1000)}K` : n.toLocaleString(); }
export function estimateTokenLabel(n) {
  if (!number(n)) return '未提供';
  if (n >= 1e6) return `${(n / 1e6).toFixed(n < 1e7 ? 1 : 0).replace(/\.0$/, '')}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(n < 100000 ? 1 : 0).replace(/\.0$/, '')}K`;
  return String(n);
}
function estimated(n) { return number(n) ? `~${estimateTokenLabel(n)}` : '未提供'; }
export function contextPercent(context) { return context && number(context.tokens) && number(context.capacity) && context.capacity > 0 ? Math.min(100, Math.max(0, Math.round(context.tokens / context.capacity * 100))) : null; }
export function cachePercent(input, read, write) {
  if (![input, read, write].every(number) || input === 0 || read + write > input) return null;
  return read / input * 100;
}
export function cachePercentLabel(percent) { return percent == null ? null : percent === 100 ? '100%' : percent > 99 ? '>99%' : `${Math.round(percent)}%`; }
export function speedLabel(tokens, milliseconds) {
  if (!number(tokens) || !number(milliseconds) || !tokens || !milliseconds) return null;
  const speed = tokens * 1000 / milliseconds;
  return `${speed < 10 ? speed.toFixed(1).replace(/\.0$/, '') : Math.round(speed)} tok/s`;
}
export function durationLabel(milliseconds) {
  if (!number(milliseconds)) return '未提供';
  if (milliseconds < 1000) return `${milliseconds}毫秒`;
  const seconds = milliseconds / 1000;
  if (seconds < 60) return `${seconds.toFixed(1).replace(/\.0$/, '')}秒`;
  const minutes = Math.floor(seconds / 60), rest = seconds - minutes * 60;
  return `${minutes}分${rest.toFixed(rest % 1 ? 1 : 0).replace(/\.0$/, '')}秒`;
}

function icon(kind) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); svg.setAttribute('viewBox', '0 0 24 24'); svg.setAttribute('class', 'line-icon'); svg.setAttribute('aria-hidden', 'true');
  const path = document.createElementNS(svg.namespaceURI, 'path');
  path.setAttribute('d', kind === 'performance'
    ? 'M4.8 18.5a9 9 0 1 1 14.4 0M12 12l5-5M12 21h.01'
    : 'M4 6c0-4 16-4 16 0s-16 4-16 0v12c0 4 16 4 16 0V6M4 12c0 4 16 4 16 0');
  svg.append(path); return svg;
}

export class ConversationMetrics {
  constructor(root, api) {
    this.events = new AbortController(); const on = (target, type, fn, options = {}) => target.addEventListener(type, fn, { ...options, signal: this.events.signal });
    this.root = root; this.api = api; this.generation = 0; this.session = null; this.value = null; this.pending = false; this.live = null; this.local = new Map();
    this.readings = el('div', 'conversation-stat-readings'); this.pills = [];
    for (const [key, label] of [['performance', '会话统计'], ['tokens', 'Token 用量'], ['context', '上下文占用']]) {
      const b = button('', () => this.show(this.open === key ? null : key), 'conversation-stat'); b.dataset.metric = key; b.setAttribute('aria-label', label); b.setAttribute('aria-haspopup', 'dialog'); b.setAttribute('aria-expanded', 'false');
      let symbol;
      if (key === 'context') {
        symbol = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); symbol.setAttribute('viewBox', '0 0 24 24'); symbol.setAttribute('class', 'context-ring'); symbol.setAttribute('aria-hidden', 'true');
        for (let i = 0; i < 2; i++) { const circle = document.createElementNS(symbol.namespaceURI, 'circle'); circle.setAttribute('cx', '12'); circle.setAttribute('cy', '12'); circle.setAttribute('r', '9'); circle.setAttribute('pathLength', '100'); symbol.append(circle); }
        this.ring = symbol.lastChild;
      } else symbol = icon(key);
      const text = el('span'), detail = el('span', 'metric-secondary'); b.append(symbol, text, detail); this.pills.push({ key, b, text, detail, label }); this.readings.append(b);
    }
    this.panel = el('section', 'conversation-stat-popover'); this.panel.id = 'conversation-stat-details'; this.panel.hidden = true; this.panel.setAttribute('role', 'dialog'); this.panel.setAttribute('aria-labelledby', 'conversation-stat-title');
    this.title = el('strong'); this.title.id = 'conversation-stat-title'; this.valueText = el('strong', 'conversation-stat-value'); this.figures = el('strong', 'conversation-stat-figures');
    this.details = el('dl'); this.note = el('p', 'muted-note');
    this.contextBar = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); this.contextBar.setAttribute('viewBox', '0 0 100 6'); this.contextBar.setAttribute('preserveAspectRatio', 'none'); this.contextBar.setAttribute('class', 'conversation-context-bar'); this.contextBar.setAttribute('role', 'progressbar'); this.contextBar.setAttribute('aria-label', '上下文占用'); this.contextBar.setAttribute('aria-valuemin', '0'); this.contextBar.setAttribute('aria-valuemax', '100');
    const track = document.createElementNS(this.contextBar.namespaceURI, 'rect'); track.setAttribute('class', 'context-track'); track.setAttribute('x', '0'); track.setAttribute('y', '0'); track.setAttribute('width', '100'); track.setAttribute('height', '6'); track.setAttribute('rx', '3'); this.contextBar.append(track);
    this.contextSegments = new Map();
    for (const [key, color] of [['system', 'system'], ['tool', 'tool'], ['message', 'message']]) { const segment = document.createElementNS(this.contextBar.namespaceURI, 'rect'); segment.setAttribute('class', `context-segment is-${color}`); segment.dataset.contextPart = key; segment.setAttribute('y', '0'); segment.setAttribute('height', '6'); segment.setAttribute('rx', '1'); this.contextSegments.set(key, segment); this.contextBar.append(segment); }
    const head = el('header'); head.append(this.title, this.valueText, this.figures);
    this.rule = el('div', 'conversation-stat-rule'); this.panel.append(head, this.rule, this.contextBar, this.details, this.note);
    root.append(this.readings, this.panel); this.root.hidden = true;
    this.popover = new PaneMenu(this.panel, { role: 'dialog', gap: 8, edge: 12, side: 'top', align: 'start', onClose: () => { this.open = null; for (const pill of this.pills) pill.b.setAttribute('aria-expanded', 'false'); } });
    for (const { b } of this.pills) b.setAttribute('aria-controls', this.panel.id);
    on(document, 'keydown', event => { if (event.key === 'Escape' && this.open) { event.preventDefault(); event.stopPropagation(); this.show(null); } });
    on(document, 'visibilitychange', () => { if (document.hidden) clearTimeout(this.timer); else this.schedule(0); });
  }
  dispose() { this.events.abort(); this.clear(); this.popover.dispose(); }
  clear() { this.generation++; this.abort?.abort(); clearTimeout(this.timer); this.pending = false; this.session = null; this.value = null; this.live = null; this.local.clear(); this.error = ''; this.lastRevision = undefined; this.lastLive = ''; this.lastEnabled = undefined; this.enabled = false; this.show(null, false); this.root.hidden = true; }
  select(session, enabled) {
    const changed = this.session?.id !== session?.id;
    if (changed) { this.clear(); this.session = session; }
    this.enabled = Boolean(enabled); this.session = session;
    this.render();
    if (changed || this.lastRevision !== session?.revision || this.enabled !== this.lastEnabled) this.schedule(0);
    this.lastRevision = session?.revision; this.lastEnabled = this.enabled;
  }
  observe(state) {
    if (!state?.run_id) return;
    this.live = state; this.local.set(state.run_id, { steps: state.step || 0, outcome: state.outcome });
    if (this.local.size > 512) this.local.delete(this.local.keys().next().value);
    this.render();
    const signature = `${state.run_id}:${state.step}:${Boolean(state.outcome)}`;
    if (signature !== this.lastLive) { this.lastLive = signature; this.schedule(state.outcome ? 100 : 600); }
  }
  schedule(delay = 1800) { clearTimeout(this.timer); if (this.session && this.enabled && !document.hidden) this.timer = setTimeout(() => { void this.refresh(); }, delay); }
  async refresh() {
    if (this.pending || !this.session || !this.enabled || document.hidden) return;
    const generation = this.generation, id = this.session.id; this.abort = new AbortController(); this.pending = true;
    try { const value = validateStatistics(await this.api.statistics(id, this.abort.signal), id); if (generation !== this.generation) return; this.value = value; this.error = ''; }
    catch (error) { if (generation === this.generation && error.name !== 'AbortError') { this.error = error.message; if ([404, 501].includes(error.status)) this.enabled = false; } }
    finally { if (generation === this.generation) { this.pending = false; this.render(); if (this.session?.status === 'running' || (this.live && !this.live.outcome)) this.schedule(); } }
  }
  current() {
    if (this.value) {
      const v = { ...this.value }; v.turns = Math.max(v.turns, this.session?.turn_count || 0);
      if (this.live && this.live.run_id === v.latest_run_id) v.steps += Math.max(0, (this.live.step || 0) - v.latest_step);
      else if (this.live && !this.live.outcome) v.context = null;
      v.usage_complete = v.usage_complete && !v.active;
      return v;
    }
    if (this.session) return { turns: this.session.turn_count, steps: null, reported_tokens: null, model_calls: null, usage_complete: false };
    const rows = [...this.local.values()]; if (!rows.length) return null;
    return { turns: rows.length, steps: rows.reduce((n, r) => n + r.steps, 0), reported_tokens: rows.reduce((n, r) => n + (r.outcome?.task_usage?.reported_tokens || 0), 0), usage_complete: rows.every(r => r.outcome?.task_usage?.usage_complete === true), local: true };
  }
  render() {
    const v = this.current(), p = contextPercent(v?.context), hasTokens = number(v?.reported_tokens) && v.reported_tokens > 0;
    const hasPerformance = Boolean(v && ((number(v.turns) && v.turns > 0) || (number(v.steps) && v.steps > 0) || v.tokens_per_second != null || v.model_time_ms > 0 || v.tool_time_ms > 0));
    for (const { key, b } of this.pills) b.hidden = key === 'tokens' ? !hasTokens : key === 'context' ? p == null : !hasPerformance;
    this.root.hidden = !hasTokens && p == null && !hasPerformance;
    if (this.root.hidden) { if (this.open) this.show(null, false); return; }
    if (this.open && this.pills.find(pill => pill.key === this.open).b.hidden) this.show(null, false);
    const cachePct = cachePercentLabel(v.cache_hit_percent ?? cachePercent(v.input_tokens, v.cache_read_tokens, v.cache_write_tokens));
    const speed = v.tokens_per_second == null ? speedLabel(v.decode_tokens, v.decode_ms) : `${v.tokens_per_second < 10 ? v.tokens_per_second.toFixed(1).replace(/\.0$/, '') : Math.round(v.tokens_per_second)} tok/s`;
    const values = {
      performance: `${number(v.turns) ? `${v.turns} 轮` : '—'} ${number(v.steps) ? `${v.steps} 步` : ''}${speed ? ` · ${speed}` : ''}`.trim(),
      tokens: `${!v.usage_complete ? '≥ ' : ''}${tokenLabel(v.reported_tokens)} tok${cachePct == null ? '' : ` · 缓存命中 ${cachePct}`}`,
      context: `${p}%`,
    };
    for (const { key, text, detail, b, label } of this.pills) {
      const [primary, secondary] = values[key].split(' · ');
      if (text.textContent !== primary) text.textContent = primary;
      detail.textContent = secondary ? ` · ${secondary}` : ''; detail.hidden = !secondary;
      b.setAttribute('aria-label', `${label}：${values[key]}`);
    }
    this.ring.setAttribute('stroke-dasharray', `${p ?? 0} 100`); this.ring.closest('svg').classList.toggle('is-unavailable', p == null);
    this.root.classList.toggle('is-stale', Boolean(this.error)); if (this.open) this.renderDetails(v);
  }
  show(key, focus = true) {
    if (!key) { this.popover.close(focus); this.open = null; return; }
    this.popover.close(); this.open = key; this.renderDetails(this.current() || {});
    this.popover.open(this.pills.find(p => p.key === key)?.b, focus);
    this.open = key; for (const pill of this.pills) pill.b.setAttribute('aria-expanded', String(pill.key === key));
  }
  renderDetails(v) {
    const isContext = this.open === 'context', isPerformance = this.open === 'performance';
    this.panel.classList.toggle('is-context', isContext); this.panel.classList.toggle('is-performance', isPerformance);
    const triggerIcon = this.pills.find(pill => pill.key === this.open)?.b.querySelector('svg');
    this.title.replaceChildren(...(isContext ? [] : [triggerIcon.cloneNode(true)]), document.createTextNode(isContext ? '上下文占用' : isPerformance ? '会话统计' : 'Token 用量'));
    this.details.replaceChildren();
    const add = (label, value, color) => {
      const dt = el('dt'), dd = el('dd', '', value);
      if (color) { const swatch = el('span', `context-swatch is-${color}`); swatch.setAttribute('aria-hidden', 'true'); dt.append(swatch); }
      dt.append(document.createTextNode(label)); this.details.append(dt, dd);
    };
    if (this.open === 'tokens') {
      this.valueText.textContent = v.reported_tokens == null ? '—' : `${v.reported_tokens.toLocaleString()} tok`;
      const cachePct = cachePercentLabel(v.cache_hit_percent ?? cachePercent(v.input_tokens, v.cache_read_tokens, v.cache_write_tokens));
      if (cachePct != null) add('缓存命中', cachePct);
      add('未缓存输入', v.cache_complete ? `${v.uncached_input_tokens.toLocaleString()} tok` : '未提供');
      add('缓存读取', number(v.cache_read_tokens) ? `${v.cache_read_tokens.toLocaleString()} tok` : '未提供');
      if (number(v.cache_write_tokens) && v.cache_write_tokens > 0) add('缓存写入', `${v.cache_write_tokens.toLocaleString()} tok`);
      add('输出', number(v.output_tokens) ? `${v.output_tokens.toLocaleString()} tok` : '未提供');
    } else if (isPerformance) {
      this.valueText.textContent = '';
      add('模型用时', durationLabel(v.model_time_ms)); add('工具调用用时', durationLabel(v.tool_time_ms));
      add('首 token 平均（TTFT）', v.average_ttft_ms == null ? '未提供' : durationLabel(Math.round(v.average_ttft_ms)));
      add('输出速度（TPS）', speedLabel(v.decode_tokens, v.decode_ms) || '未提供');
    } else {
      const context = v.context, p = contextPercent(context);
      this.valueText.textContent = p == null ? '—' : `${p}%`;
      this.figures.textContent = `${estimated(context?.tokens)} / ${number(context?.capacity) ? estimateTokenLabel(context.capacity) : '未提供'}`;
      this.contextBar.setAttribute('aria-valuenow', String(p ?? 0));
      const parts = [['system', context?.system_tokens], ['tool', context?.tool_tokens], ['message', context?.message_tokens]];
      const total = parts.every(([, count]) => number(count)) ? parts.reduce((sum, [, count]) => sum + count, 0) : 0;
      let offset = 0;
      for (const [key, count] of parts) {
        const width = total > 0 && p != null ? Math.min(100 - offset, p * count / total) : 0;
        const segment = this.contextSegments.get(key); segment.setAttribute('x', String(offset)); segment.setAttribute('width', String(width)); offset += width;
      }
      add('系统提示词', estimated(context?.system_tokens), 'system'); add('工具定义', estimated(context?.tool_tokens), 'tool'); add('对话消息', estimated(context?.message_tokens), 'message');
    }
    this.valueText.hidden = isPerformance; this.figures.hidden = !isContext; this.rule.hidden = isContext; this.contextBar.toggleAttribute('hidden', !isContext); this.details.hidden = false;
    this.note.textContent = this.error ? `读取失败：${this.error}。当前保留最近一次数据。` : '';
    this.note.hidden = !this.error;
  }
}
