import { element as el } from './content-dom.mjs';

// A read-only projection of an existing patch, never a patch application or a guessed file version.
export function parseDiff(source, limit = 6000) {
  const lines = String(source || '').split('\n'), rows = []; let oldLine = null, newLine = null, added = 0, removed = 0;
  for (const text of lines.slice(0, limit)) {
    const header = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(text);
    if (header) { oldLine = Number(header[1]); newLine = Number(header[2]); rows.push({ kind: 'hunk', text }); continue; }
    if (oldLine == null || newLine == null || /^diff --git |^index |^--- |^\+\+\+ /.test(text)) { rows.push({ kind: 'meta', text }); if (/^diff --git /.test(text)) oldLine = newLine = null; continue; }
    if (text.startsWith('-')) { removed++; rows.push({ kind: 'delete', text: text.slice(1), old: oldLine++ }); }
    else if (text.startsWith('+')) { added++; rows.push({ kind: 'add', text: text.slice(1), new: newLine++ }); }
    else if (text.startsWith(' ')) rows.push({ kind: 'context', text: text.slice(1), old: oldLine++, new: newLine++ });
    else rows.push({ kind: 'meta', text });
  }
  return { rows, added, removed, truncated: lines.length > limit };
}
export function pairDiffRows(rows) {
  const result = [];
  for (let i = 0; i < rows.length;) {
    if (!['add', 'delete'].includes(rows[i].kind)) { const row = rows[i++]; result.push(row.kind === 'context' ? { left: row, right: row } : { meta: row }); continue; }
    const removed = [], added = [];
    while (i < rows.length && ['add', 'delete'].includes(rows[i].kind)) { const row = rows[i++]; (row.kind === 'delete' ? removed : added).push(row); }
    for (let line = 0; line < Math.max(removed.length, added.length); line++) result.push({ left: removed[line], right: added[line] });
  }
  return result;
}
function side(row, previous) {
  const cell = el('div', `diff-side ${row ? 'diff-' + row.kind : 'diff-empty'}`), number = previous ? row?.old : row?.new;
  const gutter = el('span', 'diff-line-number', number == null ? '' : String(number)); gutter.setAttribute('aria-hidden', 'true');
  const code = el('code', 'diff-code', row?.text || ' '); cell.append(gutter, code); return cell;
}
export function diffContent(source, { split = false, limit = 6000 } = {}) {
  const parsed = parseDiff(source, limit), root = el('div', split ? 'message-diff is-split' : 'message-diff'); root.setAttribute('aria-label', split ? '左右文件差异' : '合并文件差异');
  if (split) {
    const header = el('div', 'diff-columns'); header.append(el('span', '', '修改前'), el('span', '', '修改后')); root.append(header);
    for (const pair of pairDiffRows(parsed.rows)) {
      if (pair.meta) { root.append(el('pre', 'diff-meta', pair.meta.text)); continue; }
      const row = el('div', 'diff-columns'); row.append(side(pair.left, true), side(pair.right, false)); root.append(row);
    }
  } else for (const value of parsed.rows) {
    if (value.kind === 'meta' || value.kind === 'hunk') { root.append(el('pre', 'diff-meta', value.text)); continue; }
    const row = el('div', `diff-unified diff-${value.kind}`);
    const before = el('span', 'diff-line-number', value.old == null ? '' : String(value.old)), after = el('span', 'diff-line-number', value.new == null ? '' : String(value.new));
    before.setAttribute('aria-hidden', 'true'); after.setAttribute('aria-hidden', 'true');
    row.append(before, after, el('code', 'diff-code', (value.kind === 'add' ? '+' : value.kind === 'delete' ? '-' : ' ') + value.text)); root.append(row);
  }
  if (parsed.truncated) root.append(el('p', 'muted-note', `差异预览达到 ${limit} 行，复制可获取本次接口返回的完整差异。`));
  return root;
}
