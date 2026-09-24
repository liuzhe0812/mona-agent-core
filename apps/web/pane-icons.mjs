// Local icon vocabulary for Mona application panels.
const paths = {
  file: 'M14 2H6a2 2 0 0 0-2 2v16h16V8ZM14 2v6h6M8 13h8M8 17h5',
  folder: 'M3 7V5h6l2 2h10v12H3Z', terminal: 'm4 6 6 6-6 6M13 18h7',
  git: 'M8 3v12a4 4 0 0 0 8 0V9M5 3h6M13 6h6M5 19h6',
  side: 'M21 11a8 8 0 0 1-8 8H6l-4 3V11a8 8 0 0 1 8-8h3a8 8 0 0 1 8 8ZM7 9h10M7 13h7',
  overview: 'M3 4h7v7H3ZM14 4h7v7h-7ZM3 15h7v5H3ZM14 15h7v5h-7Z',
  close: 'm6 6 12 12M18 6 6 18', copy: 'M9 9h11v12H9ZM5 15H3V3h12v2',
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
