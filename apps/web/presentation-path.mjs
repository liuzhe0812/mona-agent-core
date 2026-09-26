// Display-path normalization only. The host's Directory authorization remains authoritative.
export function messageFileTarget(value, workspace, base = '') {
  if (typeof value !== 'string' || value.length > 4096 || /[\u0000-\u001f\u007f]/.test(value)) throw new Error('文件引用无效。');
  let path = value, line;
  const fragment = /#L(\d+)(?:-L?\d+)?$/.exec(path); if (fragment) { line = Number(fragment[1]); path = path.slice(0, fragment.index); }
  if (/^file:\/\//i.test(path)) {
    const uri = new URL(path);
    if ((uri.hostname && uri.hostname !== 'localhost') || uri.username || uri.password || uri.search) throw new Error('不能打开其他主机或带查询参数的文件。');
    path = uri.pathname.replace(/^\/([a-z]:\/)/i, '$1');
  }
  try { path = decodeURIComponent(path); } catch { throw new Error('文件路径编码无效。'); }
  if (/[\u0000-\u001f\u007f]/.test(path)) throw new Error('文件引用含有无效字符。');
  path = path.replace(/\\/g, '/').replace(/^\/\/\?\//, ''); const root = String(workspace || '').replace(/\\/g, '/').replace(/^\/\/\?\//, '').replace(/\/$/, '');
  if (path.startsWith('//') || (/^[a-z][a-z0-9+.-]*:/i.test(path) && !/^[a-z]:\//i.test(path))) throw new Error('此引用不是工作区文件。');
  if (path.startsWith('/') || /^[a-z]:\//i.test(path)) {
    const comparable = /^[a-z]:\//i.test(root) ? path.toLowerCase() : path, expected = /^[a-z]:\//i.test(root) ? root.toLowerCase() : root;
    if (!expected || !comparable.startsWith(expected + '/')) throw new Error('文件引用超出此会话的工作区。'); path = path.slice(root.length + 1);
  } else if (base) path = base.replace(/\/$/, '') + '/' + path;
  const segments = [];
  for (const part of path.split('/')) {
    if (!part || part === '.') continue;
    if (part.includes(':')) throw new Error('文件引用格式无效。');
    if (part === '..') { if (!segments.length) throw new Error('文件引用不能跳出工作区。'); segments.pop(); }
    else segments.push(part);
  }
  if (!segments.length) throw new Error('请选择工作区内的文件。');
  if (line !== undefined && (!Number.isSafeInteger(line) || line < 1 || line > 1000000)) throw new Error('文件行号无效。');
  return { path: segments.join('/'), line };
}
