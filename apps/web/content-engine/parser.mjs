import MarkdownIt from 'markdown-it';
import footnote from 'markdown-it-footnote';

export const MAX_MARKDOWN_CHARS = 1024 * 1024;
export const markdown = new MarkdownIt({ html: false, linkify: true, typographer: false, breaks: false, maxNesting: 64 }).use(footnote);
markdown.linkify.set({ fuzzyEmail: false, fuzzyLink: false });
// Preserve opaque/local paths as tokens. The DOM renderer, never this parser, grants clickable access.
markdown.validateLink = () => true;
function escaped(source, at) { let count = 0; while (at > 0 && source[--at] === '\\') count++; return count % 2 === 1; }
function formulaInline(state, silent) {
  const start = state.pos, parens = state.src.startsWith('\\(', start), dollar = state.src[start] === '$' && state.src[start + 1] !== '$';
  if (!parens && !dollar) return false;
  const openLength = parens ? 2 : 1, marker = parens ? '\\)' : '$';
  if (dollar && /\s/.test(state.src[start + 1] || ' ')) return false;
  let end = state.src.indexOf(marker, start + openLength);
  while (end >= 0 && escaped(state.src, end)) end = state.src.indexOf(marker, end + marker.length);
  if (end < 0 || end === start + openLength || end - start > 8192) return false;
  const source = state.src.slice(start + openLength, end);
  if (source.includes('\n') || (dollar && (/\s$/.test(source) || /^\d/.test(state.src[end + 1] || '')))) return false;
  // Do not turn "$10 and $20" into an equation; single identifiers and real math remain supported.
  if (dollar && /^\d[\d,.]*\s+[\p{L}\s]+$/u.test(source)) return false;
  if (!silent) { const token = state.push('mona_math_inline', 'math', 0); token.content = source; }
  state.pos = end + marker.length; return true;
}
function formulaBlock(state, start, end, silent) {
  const begin = state.bMarks[start] + state.tShift[start], stop = state.eMarks[start];
  const line = state.src.slice(begin, stop), match = /^(\$\$|\\\[)/.exec(line);
  if (!match || state.sCount[start] - state.blkIndent >= 4) return false;
  if (silent) return true;
  const marker = match[1] === '$$' ? '$$' : '\\]';
  let current = start, body = line.slice(match[1].length), close = body.indexOf(marker);
  const pieces = []; let trailing = '';
  if (close >= 0) { pieces.push(body.slice(0, close)); trailing = body.slice(close + marker.length); }
  else {
    pieces.push(body);
    for (current = start + 1; current < end; current++) {
      body = state.src.slice(state.bMarks[current] + state.tShift[current], state.eMarks[current]); close = body.indexOf(marker);
      if (close >= 0) { pieces.push(body.slice(0, close)); trailing = body.slice(close + marker.length); break; }
      pieces.push(body);
    }
  }
  const token = state.push('mona_math_block', 'math', 0); token.block = true; token.content = pieces.join('\n').trim(); token.map = [start, Math.min(end, current + 1)];
  token.meta = { complete: close >= 0 }; state.line = Math.min(end, current + 1);
  if (trailing.trim()) { state.push('paragraph_open', 'p', 1); const text = state.push('inline', '', 0); text.content = trailing.trim(); text.children = []; state.push('paragraph_close', 'p', -1); }
  return true;
}
markdown.inline.ruler.before('escape', 'mona_math', formulaInline);
markdown.block.ruler.before('fence', 'mona_math', formulaBlock, { alt: ['paragraph', 'reference', 'blockquote', 'list'] });
export function parseMarkdown(source) {
  if (typeof source !== 'string' || source.length > MAX_MARKDOWN_CHARS) throw new Error('正文超过 1 MiB 排版上限，请复制原文或分段查看。');
  return markdown.parse(source, {});
}
