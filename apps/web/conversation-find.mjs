// Literal, Unicode-safe search over presentation text. Formatting nodes are not word boundaries.
const EXCLUDED = 'button,summary,script,style,.message-actions,.message-code-head,.message-table-actions,.katex-mathml,.file-line-numbers,[data-search-exclude]';
const BLOCKS = 'p,pre,li,td,th,h1,h2,h3,h4,h5,h6,.user-message,.activity-panel-code';
export function literalMatches(text, query, limit = 5000) {
  if (!query || limit <= 0) return [];
  // Matching against the original string preserves UTF-16 offsets (e.g. Turkish capital I).
  const escaped = query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const expression = new RegExp(escaped, 'giu'); const matches = [];
  for (const match of String(text).matchAll(expression)) { matches.push({ start: match.index, end: match.index + match[0].length }); if (matches.length >= limit) break; }
  return matches;
}
export function indexDomText(root) {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, { acceptNode: node => {
    const parent = node.parentElement;
    return parent && !parent.closest(EXCLUDED) ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT;
  } });
  let text = '', previousBlock; const parts = [];
  while (walker.nextNode()) {
    const node = walker.currentNode; if (!node.data) continue;
    const block = node.parentElement.closest(BLOCKS) || root;
    if (previousBlock && block !== previousBlock) text += '\n';
    parts.push({ node, start: text.length, end: text.length + node.length }); text += node.data; previousBlock = block;
  }
  return { text, parts };
}
export function domMatchRanges(root, query, limit = 5000) {
  const { text, parts } = indexDomText(root); const matches = literalMatches(text, query, limit); const ranges = [];
  let firstIndex = 0;
  for (const match of matches) {
    while (firstIndex < parts.length && parts[firstIndex].end <= match.start) firstIndex++;
    const first = parts[firstIndex]; let lastIndex = firstIndex;
    while (lastIndex < parts.length && parts[lastIndex].end < match.end) lastIndex++;
    const last = parts[lastIndex];
    if (!first || !last || match.start < first.start || match.end > last.end) continue;
    const range = document.createRange(); range.setStart(first.node, match.start - first.start); range.setEnd(last.node, match.end - last.start); ranges.push(range);
  }
  return ranges;
}
