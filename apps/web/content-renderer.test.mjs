import test from 'node:test';
import assert from 'node:assert/strict';
import { parseMarkdown, markdown } from './vendor/content/parser.mjs';
import { messageFileTarget } from './presentation-path.mjs';
import { csvText, safeWebUrl } from './content-dom.mjs';
import { WorkspaceClient } from './workspace.mjs';
import { literalMatches } from './conversation-find.mjs';
import { PaneState } from './pane-state.mjs';
import { parseDiff, pairDiffRows } from './diff-view.mjs';

test('standard parser preserves identifiers, single-column tables, nested emphasis and escaped pipes', () => {
  const inline = markdown.parseInline('foo_bar_baz **外层 *内层* 结束**', {})[0].children;
  assert.equal(inline[0].content, 'foo_bar_baz ');
  assert.ok(inline.some(token => token.type === 'em_open'));
  const table = parseMarkdown('| Name |\n| --- |\n| A\\|B |');
  assert.equal(table[0].type, 'table_open');
  assert.ok(table.some(token => token.type === 'inline' && token.content === 'A|B'));
});
test('four-backtick fences preserve triple fences, links with parentheses and references parse correctly', () => {
  const code = parseMarkdown('````md\n```rust\nfn main() {}\n```\n````');
  assert.equal(code.length, 1); assert.equal(code[0].info, 'md'); assert.ok(code[0].content.includes('```rust'));
  const link = parseMarkdown('[read](docs/a(b).md)\n\n[ref][id]\n\n[id]: docs/info.md');
  assert.ok(link.flatMap(t => t.children || []).some(t => t.attrs?.some(([k,v])=>k==='href'&&v==='docs/a(b).md')));
});
test('math has explicit tokens and currency is not silently interpreted as an equation', () => {
  const tokens = markdown.parseInline('$x_i^2$ and \\(\\frac{a}{b}\\) costs $10 and $20', {})[0].children;
  assert.equal(tokens.filter(t => t.type === 'mona_math_inline').length, 2);
  assert.ok(tokens.some(t=>t.type==='text' && t.content.includes('$10 and $20')));
  assert.equal(parseMarkdown('$$\nx_i^2\n$$')[0].type, 'mona_math_block');
  assert.ok(parseMarkdown('$$x_i^2$$ trailing explanation').some(token => token.type === 'inline' && token.content === 'trailing explanation'));
  assert.ok(parseMarkdown('$$\nx_i^2\n$$ after formula').some(token => token.type === 'inline' && token.content === 'after formula'));
});
test('HTML stays text and document nesting and size are bounded', () => {
  const parsed = parseMarkdown('<script>alert(1)</script>');
  assert.ok(!parsed.some(t => t.type === 'html_block'));
  assert.throws(()=>parseMarkdown('x'.repeat(1024*1024+1)));
});
test('file references resolve within immutable session roots with no traversal or authority confusion', () => {
  assert.deepEqual(messageFileTarget('./docs/info.md#L12', 'D:\\work'), {path:'docs/info.md',line:12});
  assert.equal(messageFileTarget('D:\\work\\src\\main.rs','D:\\work').path, 'src/main.rs');
  assert.equal(messageFileTarget('file:///D:/work/src/main.rs','D:\\work').path, 'src/main.rs');
  for (const value of ['../secret','%2e%2e/secret','a/%00.txt','D:/other/file','//other/path','https://other/file']) assert.throws(()=>messageFileTarget(value,'D:/work'),value);
  assert.equal(messageFileTarget('image.png','/work','docs').path,'docs/image.png');
  assert.throws(()=>messageFileTarget('/work-else/file','/work'));
});
test('relative document links may navigate within the same root but never escape it', () => {
  assert.equal(messageFileTarget('../images/plot.png', '/work', 'docs').path, 'images/plot.png');
  assert.equal(messageFileTarget('./a/../b.md', '/work').path, 'b.md');
  assert.throws(() => messageFileTarget('../../private', '/work', 'docs'));
  assert.throws(() => messageFileTarget('file:///work/a%00.txt', '/work'));
  assert.equal(messageFileTarget('file:///work/percent%2520name.md', '/work').path, 'percent%20name.md');
});
test('literal conversation search preserves source offsets and does not interpret regex input', () => {
  assert.deepEqual(literalMatches('İ hello HELLO', 'hello'), [{ start: 2, end: 7 }, { start: 8, end: 13 }]);
  assert.deepEqual(literalMatches('a+b [x] .*', '[x]'), [{ start: 4, end: 7 }]);
  assert.deepEqual(literalMatches('a+b [x] .*', '.*'), [{ start: 8, end: 10 }]);
  assert.deepEqual(literalMatches('😀中文😀', '中文'), [{ start: 2, end: 4 }]);
  assert.equal(literalMatches('aaa', 'a', 2).length, 2);
});
test('file tab state is bounded, endpoint scoped, and persists no document body or credentials', () => {
  const values = new Map(), storage = { getItem: key => values.get(key) || null, setItem: (key,value) => values.set(key,value) };
  const first = new PaneState(() => storage); first.configure('http://127.0.0.1:8787');
  assert.equal(first.save('session:s-one', [{ path:'docs/a.md', mode:'source', wrap:true, text:'secret' }], 'docs/a.md'), true);
  const second = new PaneState(() => storage); second.configure('http://127.0.0.1:8787');
  assert.deepEqual(second.get('session:s-one').files, [{ path:'docs/a.md', mode:'source', wrap:true }]);
  assert.ok(![...values.values()][0].includes('secret'));
  second.configure('http://127.0.0.1:9999'); assert.equal(second.get('session:s-one'), undefined);
  const denied = new PaneState(() => { throw Error('denied'); }); denied.configure('http://127.0.0.1:8787'); assert.equal(denied.save('session:s-one', [], null), false);
});
test('exports preserve Chinese and neutralize spreadsheet formulas; web URLs reject credential and script schemes', () => {
  assert.equal(csvText([['标题','值'],['=cmd','中文']]), '\uFEFF标题,值\r\n\'=cmd,中文');
  assert.equal(safeWebUrl('javascript:alert(1)'), null); assert.equal(safeWebUrl('https://key:secret@host.test'),null);
  assert.equal(safeWebUrl('https://example.com/a'), 'https://example.com/a');
});
test('unified patch presentation keeps old/new line numbers and pairs replacements correctly', () => {
  const patch = '--- a/f\n+++ b/f\n@@ -8,3 +8,4 @@\n before\n-old\n+new\n+extra\n after';
  const parsed = parseDiff(patch); assert.equal(parsed.added, 2); assert.equal(parsed.removed, 1);
  const paired = pairDiffRows(parsed.rows); const replacement = paired.find(pair => pair.left?.kind === 'delete');
  assert.equal(replacement.left.old, 9); assert.equal(replacement.right.new, 9);
  assert.ok(paired.some(pair => !pair.left && pair.right?.text === 'extra'));
  assert.equal(paired.at(-1).left.old, 10); assert.equal(paired.at(-1).right.new, 11);
  assert.equal(parseDiff(patch, 4).truncated, true);
});
test('owned resource connections never send close to a subsequently configured host', async () => {
  const previous = globalThis.fetch, requests=[];
  globalThis.fetch = async (url, options) => { requests.push([url,options.headers.Authorization]); return new Response(null,{status:204}); };
  try {
    const shared = new WorkspaceClient(); shared.configure('https://one.test','one'); const owned = shared.fork();
    shared.configure('https://two.test','two');
    await owned.terminalClose({kind:'session',id:'s-first'},'t-first'); owned.clear();
    assert.equal(requests[0][0],'https://one.test/api/workbench/session/s-first/terminals/t-first'); assert.equal(requests[0][1],'Bearer one');
  } finally { globalThis.fetch=previous; }
});
