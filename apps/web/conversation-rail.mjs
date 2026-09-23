// Conversation rail: compact turn navigation for long conversations.
const main = document.querySelector('#main');
const timeline = document.querySelector('#timeline');
const rail = document.querySelector('#conversation-rail');
const marksRoot = rail?.querySelector('.conversation-rail-marks');
const preview = rail?.querySelector('.conversation-rail-preview');
const previewTitle = rail?.querySelector('.conversation-rail-preview-title');
const previewText = rail?.querySelector('.conversation-rail-preview-text');

const compact = value => String(value || '').replace(/\s+/g, ' ').trim();
const turns = () => [...(timeline?.children || [])].filter(node => node.classList.contains('turn'));

if (main && timeline && rail && marksRoot && preview && previewTitle && previewText) {
  const marks = new Map();
  let frame = 0;

  function promptOf(turn) {
    return compact(turn.querySelector('.user-message')?.textContent) || '未命名轮次';
  }

  function answerOf(turn) {
    const visible = turn.querySelector('.assistant-body .message-text, .assistant-body, .message-text');
    return compact(visible?.textContent) || '本轮暂无可见回复';
  }

  function hidePreview() {
    preview.hidden = true;
  }

  function showPreview(turn) {
    previewTitle.textContent = promptOf(turn);
    previewText.textContent = answerOf(turn);
    preview.hidden = false;
  }

  function turnTop(turn) {
    const viewport = main.getBoundingClientRect();
    const box = turn.getBoundingClientRect();
    return main.scrollTop + box.top - viewport.top;
  }

  function syncActive(list = turns()) {
    if (!list.length || rail.hidden) return;
    const readingLine = main.scrollTop + main.clientHeight * 0.42;
    let current = list[0];
    for (const turn of list) {
      if (turnTop(turn) <= readingLine) current = turn;
      else break;
    }
    for (const [turn, mark] of marks) {
      const active = turn === current;
      mark.classList.toggle('is-active', active);
      if (active) mark.setAttribute('aria-current', 'step');
      else mark.removeAttribute('aria-current');
    }
  }

  function createMark(turn) {
    const mark = document.createElement('button');
    mark.type = 'button';
    mark.className = 'conversation-rail-mark';
    mark.addEventListener('pointerenter', () => showPreview(turn));
    mark.addEventListener('pointerleave', hidePreview);
    mark.addEventListener('focus', () => showPreview(turn));
    mark.addEventListener('blur', hidePreview);
    mark.addEventListener('click', () => {
      hidePreview();
      turn.scrollIntoView({
        behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth',
        block: 'start',
      });
    });
    marks.set(turn, mark);
    return mark;
  }

  function sync() {
    const list = turns();
    const live = new Set(list);
    for (const [turn, mark] of marks) {
      if (!live.has(turn)) {
        mark.remove();
        marks.delete(turn);
      }
    }

    rail.hidden = list.length < 2;
    rail.classList.toggle('is-short', list.length <= 6);
    if (rail.hidden) {
      hidePreview();
      return;
    }

    list.forEach((turn, index) => {
      const mark = marks.get(turn) || createMark(turn);
      mark.setAttribute('aria-label', `第 ${index + 1} 轮：${promptOf(turn).slice(0, 60)}`);
      if (marksRoot.children[index] !== mark) {
        marksRoot.insertBefore(mark, marksRoot.children[index] || null);
      }
    });
    syncActive(list);
  }

  function schedule() {
    if (frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      sync();
    });
  }

  new MutationObserver(schedule).observe(timeline, {
    childList: true,
    subtree: true,
    characterData: true,
  });
  new ResizeObserver(schedule).observe(main);
  main.addEventListener('scroll', () => {
    hidePreview();
    syncActive();
  }, { passive: true });
  window.addEventListener('resize', schedule);
  schedule();
}
