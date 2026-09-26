// Compact query navigation for long conversations. It reads rendered turns only;
// session ownership and persistence remain outside this presentation component.
const PREVIEW_OPEN_DELAY_MS = 120;
const PREVIEW_CLOSE_DELAY_MS = 80;
const PREVIEW_VIEWPORT_GUTTER_PX = 16;
const NAVIGATOR_MIN_WIDTH_PX = 864;
const MAX_PREVIEW_CHARS = 220;
const MAX_PREVIEW_PARAGRAPHS = 2;

export function railVisualState(itemIndex, focusIndex) {
  if (focusIndex == null) return { tone: 'idle', opacity: .58, scaleX: 1 };
  const distance = Math.abs(itemIndex - focusIndex);
  if (distance === 0) return { tone: 'peak', opacity: 1, scaleX: 2.6 };
  if (distance === 1) return { tone: 'near', opacity: .86, scaleX: 1.7 };
  if (distance === 2) return { tone: 'mid', opacity: .72, scaleX: 1.25 };
  return { tone: 'idle', opacity: .58, scaleX: 1 };
}

export function previewText(value, fallback) {
  const paragraphs = String(value || '').trim().split(/\n\s*\n/u)
    .map(part => part.replace(/\s+/gu, ' ').trim()).filter(Boolean)
    .slice(0, MAX_PREVIEW_PARAGRAPHS);
  const valueText = paragraphs.join('\n') || fallback;
  return valueText.length <= MAX_PREVIEW_CHARS
    ? valueText
    : `${valueText.slice(0, MAX_PREVIEW_CHARS - 3).trimEnd()}...`;
}

export function mountConversationRail(ctx) {
  const main = document.querySelector('#main');
  const timeline = document.querySelector('#timeline');
  const rail = document.querySelector('#conversation-rail');
  const workspace = rail?.parentElement;
  const marksRoot = rail?.querySelector('.conversation-rail-marks');
  const preview = rail?.querySelector('.conversation-rail-preview');
  const previewTitle = rail?.querySelector('.conversation-rail-preview-title');
  const previewTextNode = rail?.querySelector('.conversation-rail-preview-text');
  if (!main || !timeline || !rail || !workspace || !marksRoot || !preview || !previewTitle || !previewTextNode) {
    throw new Error('会话轮次导航模板不完整。');
  }
  const marks = new Map();
  let frame = 0;
  let interactionIndex;
  let orderedItems = [];
  let previewTarget;
  let openTimer;
  let closeTimer;

  const turns = () => [...timeline.children].filter(node => node.classList.contains('turn'));
  const queries = () => turns().flatMap(turn =>
    [...turn.querySelectorAll('.user-message')].map(query => ({ turn, query })));

  function promptOf(query) {
    return previewText(query.textContent, '用户输入');
  }

  function answerOf(turn) {
    const answer = turn.querySelector('.assistant-body .message-text');
    if (answer?.textContent?.trim()) return { text: previewText(answer.textContent, '暂无助手正文'), muted: false };
    if (turn.querySelector('.execution-process.is-running')) return { text: '助手仍在工作', muted: true };
    return { text: '暂无助手正文', muted: true };
  }

  function clearPreviewTimers() {
    clearTimeout(openTimer);
    clearTimeout(closeTimer);
  }

  function hidePreview() {
    clearPreviewTimers();
    preview.hidden = true;
    previewTarget = undefined;
  }

  function showPreview(item) {
    if (!item.query.isConnected) return;
    const answer = answerOf(item.turn);
    previewTitle.textContent = promptOf(item.query);
    previewTextNode.textContent = answer.text;
    previewTextNode.classList.toggle('is-muted', answer.muted);
    rail.append(preview);
    preview.hidden = false;
    const railBox = rail.getBoundingClientRect();
    const markBox = item.mark.getBoundingClientRect();
    const desiredTop = markBox.top - railBox.top;
    const maxTop = Math.max(PREVIEW_VIEWPORT_GUTTER_PX,
      rail.clientHeight - preview.offsetHeight - PREVIEW_VIEWPORT_GUTTER_PX);
    const top = Math.min(Math.max(PREVIEW_VIEWPORT_GUTTER_PX, desiredTop), maxTop);
    preview.style.top = `${top}px`;
    previewTarget = item.query;
  }

  function schedulePreview(item) {
    clearPreviewTimers();
    openTimer = setTimeout(() => showPreview(item), PREVIEW_OPEN_DELAY_MS);
  }

  function scheduleHidePreview() {
    clearTimeout(openTimer);
    clearTimeout(closeTimer);
    closeTimer = setTimeout(hidePreview, PREVIEW_CLOSE_DELAY_MS);
  }

  function applyVisualState() {
    rail.classList.toggle('is-interacting', interactionIndex != null);
    orderedItems.forEach((item, index) => {
      const state = railVisualState(index, interactionIndex);
      item.mark.classList.remove('is-idle', 'is-mid', 'is-near', 'is-peak');
      item.mark.classList.add(`is-${state.tone}`);
      item.mark.dataset.visualTone = state.tone;
      item.mark.dataset.visualOpacity = String(state.opacity);
      item.mark.dataset.visualScale = String(state.scaleX);
    });
  }

  function setInteraction(index) {
    interactionIndex = index;
    applyVisualState();
  }

  function queryTop(query) {
    const viewport = main.getBoundingClientRect();
    const box = query.getBoundingClientRect();
    return main.scrollTop + box.top - viewport.top;
  }

  function currentQuery(list) {
    if (!list.length) return undefined;
    const start = main.scrollTop;
    const end = start + Math.max(1, main.clientHeight);
    const positions = list.map(item => {
      const top = queryTop(item.query);
      return { item, top, bottom: top + Math.max(1, item.query.getBoundingClientRect().height) };
    });
    const visible = positions.filter(item => item.bottom >= start && item.top <= end);
    if (visible.length) return visible.reduce((nearest, candidate) =>
      Math.abs(candidate.top - start) < Math.abs(nearest.top - start) ? candidate : nearest).item;
    return positions.findLast(item => item.top <= start)?.item
      || positions.find(item => item.top > start)?.item
      || list[0];
  }

  function keepActiveMarkVisible(mark) {
    const slot = mark.parentElement;
    if (!slot) return;
    const top = slot.offsetTop;
    const bottom = top + slot.offsetHeight;
    if (top < marksRoot.scrollTop) marksRoot.scrollTop = top;
    else if (bottom > marksRoot.scrollTop + marksRoot.clientHeight) {
      marksRoot.scrollTop = bottom - marksRoot.clientHeight;
    }
  }

  function syncActive(list = orderedItems) {
    if (!list.length || rail.hidden) return;
    const current = currentQuery(list);
    for (const item of list) {
      const active = item === current;
      item.mark.classList.toggle('is-active', active);
      item.mark.dataset.active = String(active);
      if (active) item.mark.setAttribute('aria-current', 'location');
      else item.mark.removeAttribute('aria-current');
    }
    if (current) keepActiveMarkVisible(current.mark);
  }

  function jumpTo(item) {
    hidePreview();
    const top = queryTop(item.query);
    if (matchMedia('(prefers-reduced-motion: reduce)').matches) main.scrollTop = top;
    else main.scrollTo({ top, behavior: 'smooth' });
  }

  function createMark(item) {
    const slot = document.createElement('div');
    slot.className = 'conversation-rail-slot';
    const mark = document.createElement('button');
    const controller = new AbortController();
    item.controller = controller;
    mark.type = 'button';
    mark.className = 'conversation-rail-mark is-idle';
    mark.addEventListener('pointerenter', () => {
      setInteraction(orderedItems.indexOf(item));
      schedulePreview(item);
    }, { signal: controller.signal });
    mark.addEventListener('pointerleave', () => {
      setInteraction(undefined);
      scheduleHidePreview();
    }, { signal: controller.signal });
    mark.addEventListener('focus', () => {
      setInteraction(orderedItems.indexOf(item));
      showPreview(item);
    }, { signal: controller.signal });
    mark.addEventListener('blur', () => {
      setInteraction(undefined);
      scheduleHidePreview();
    }, { signal: controller.signal });
    mark.addEventListener('click', () => jumpTo(item), { signal: controller.signal });
    slot.append(mark);
    item.slot = slot;
    item.mark = mark;
    return slot;
  }

  function sync() {
    const liveQueries = queries();
    const live = new Set(liveQueries.map(item => item.query));
    for (const [query, item] of marks) {
      if (!live.has(query)) {
        item.controller.abort();
        item.slot.remove();
        marks.delete(query);
        if (previewTarget === query) hidePreview();
      }
    }

    rail.hidden = liveQueries.length < 2;
    if (rail.hidden) {
      hidePreview();
      return;
    }

    liveQueries.forEach(({ turn, query }, index) => {
      let item = marks.get(query);
      if (!item) {
        item = { turn, query };
        marks.set(query, item);
        createMark(item);
      } else item.turn = turn;
      const running = Boolean(turn.querySelector('.execution-process.is-running'))
        && query === [...turn.querySelectorAll('.user-message')].at(-1);
      item.mark.setAttribute('aria-label', `跳转到第 ${index + 1} 条问题`);
      item.mark.setAttribute('aria-posinset', String(index + 1));
      item.mark.setAttribute('aria-setsize', String(liveQueries.length));
      item.mark.dataset.itemIndex = String(index);
      item.mark.dataset.running = String(running);
      item.mark.classList.toggle('is-running', running);
      if (marksRoot.children[index] !== item.slot) {
        marksRoot.insertBefore(item.slot, marksRoot.children[index] || null);
      }
    });
    orderedItems = liveQueries.map(item => marks.get(item.query));
    rail.dataset.itemCount = String(orderedItems.length);
    applyVisualState();
    syncActive();
    if (previewTarget && !preview.hidden) {
      const item = marks.get(previewTarget);
      if (item) showPreview(item);
    }
  }

  function schedule() {
    if (frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      sync();
    });
  }

  function syncWidth() {
    rail.classList.toggle('is-wide', workspace.clientWidth >= NAVIGATOR_MIN_WIDTH_PX);
  }

  const mutations = new MutationObserver(schedule);
  mutations.observe(timeline, { childList: true, subtree: true, characterData: true });
  const resizeObserver = new ResizeObserver(() => {
    syncWidth();
    schedule();
  });
  resizeObserver.observe(main);
  resizeObserver.observe(workspace);
  ctx.listen(marksRoot, 'pointerleave', () => setInteraction(undefined));
  ctx.listen(marksRoot, 'scroll', () => {
    setInteraction(undefined);
    hidePreview();
  }, { passive: true });
  ctx.listen(main, 'scroll', () => {
    setInteraction(undefined);
    hidePreview();
    syncActive();
  }, { passive: true });
  ctx.listen(window, 'resize', schedule);
  syncWidth();
  schedule();
  return () => {
    if (frame) cancelAnimationFrame(frame);
    clearPreviewTimers();
    mutations.disconnect();
    resizeObserver.disconnect();
    for (const item of marks.values()) item.controller.abort();
    marks.clear();
    orderedItems = [];
    marksRoot.replaceChildren();
    preview.hidden = true;
    previewTitle.textContent = '';
    previewTextNode.textContent = '';
    rail.hidden = true;
    rail.classList.remove('is-wide', 'is-interacting');
    delete rail.dataset.itemCount;
  };
}
