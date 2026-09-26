// Local icon vocabulary for Mona application panels.
const paths = {
  models: 'M20 5c0 2-3.6 3.5-8 3.5S4 7 4 5s3.6-3.5 8-3.5S20 3 20 5ZM4 5v14c0 2 3.6 3.5 8 3.5s8-1.5 8-3.5V5M4 12c0 2 3.6 3.5 8 3.5s8-1.5 8-3.5',
  sandbox: 'M12 3 4 6v6c0 5 8 9 8 9s8-4 8-9V6ZM9 12l2 2 4-4',
  more: 'M5 12h.01M12 12h.01M19 12h.01', search: 'M10.5 4a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13Zm5 12 5 5', wrap: 'M4 6h16M4 11h12a4 4 0 0 1 0 8h-5m3-3-3 3 3 3M4 16h3', download: 'M12 3v12m-5-5 5 5 5-5M4 16v5h16v-5', collapseAll: 'M5 4h14M5 9h14m-11 5 4 4 4-4',
  files: 'M3 7V5h6l2 2h10v12H3Z', expand: 'M8 3H3v5M16 3h5v5M3 16v5h5M21 16v5h-5', split: 'M3 4h18v16H3ZM12 4v16',
  file: 'M14 2H6a2 2 0 0 0-2 2v16h16V8ZM14 2v6h6M8 13h8M8 17h5',
  image: 'M4 4h16v16H4ZM7 16l4-4 3 3 2-2 2 3M8 9h.01',
  imageOff: 'M4 4h16v16H4ZM7 16l4-4 3 3 2-2 2 3M8 9h.01M3 3l18 18',
  previous: 'm15 5-7 7 7 7', next: 'm9 5 7 7-7 7',
  minus: 'M5 12h14',
  horizontalExpand: 'M4 12h16M4 12l4-4M4 12l4 4M20 12l-4-4M20 12l-4 4',
  folder: 'M3 7V5h6l2 2h10v12H3Z', terminal: 'm4 6 6 6-6 6M13 18h7',
  git: 'M8 3v12a4 4 0 0 0 8 0V9M5 3h6M13 6h6M5 19h6',
  side: 'M21 11a8 8 0 0 1-8 8H6l-4 3V11a8 8 0 0 1 8-8h3a8 8 0 0 1 8 8ZM7 9h10M7 13h7',
  overview: 'M3 4h7v7H3ZM14 4h7v7h-7ZM3 15h7v5H3ZM14 15h7v5h-7Z',
  close: 'm6 6 12 12M18 6 6 18', copy: 'M9 9h11v12H9ZM5 15H3V3h12v2', check: 'M5 12l4 4L19 6',
  collapse: 'M3 4h18v16H3ZM15 4v16', add: 'M12 5v14M5 12h14', refresh: 'M20 8a8 8 0 1 0 0 8M20 3v5h-5',
};
export function paneIcon(kind) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('class', 'line-icon'); svg.setAttribute('viewBox', '0 0 24 24'); svg.setAttribute('aria-hidden', 'true');
  const path = document.createElementNS(svg.namespaceURI, 'path'); path.setAttribute('d', paths[kind] || paths.file); svg.append(path); return svg;
}
export function panelButton(label, action, className = 'text-button') {
  const button = document.createElement('button'); button.type = 'button'; button.className = className; button.textContent = label;
  if (action) button.addEventListener('click', action); return button;
}
export async function copyPanelText(text, button) {
  try { await navigator.clipboard.writeText(text); const label = button.textContent; button.textContent = '已复制'; setTimeout(() => { if (button.isConnected) button.textContent = label; }, 1200); }
  catch { button.textContent = '复制失败'; }
}
