// Reproducible self-hosted content assets. npm ci --prefix scripts/content-assets --ignore-scripts
import { createRequire } from 'node:module';
import { mkdir, cp, readdir, readFile, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
const root = resolve('scripts/content-assets'), require = createRequire(join(root, 'package.json'));
const { build } = require('esbuild'); const target = 'apps/web/vendor/content'; await mkdir(target, { recursive: true });
for (const name of ['parser', 'core', 'highlight', 'diagram']) await build({
  entryPoints: [`apps/web/content-engine/${name}.mjs`], nodePaths: [join(root, 'node_modules')], bundle: true,
  format: name === 'diagram' ? 'iife' : 'esm', platform: 'browser', target: 'es2022', minify: true, legalComments: 'eof',
  outfile: `${target}/${name}.${name === 'diagram' ? 'js' : 'mjs'}`, define: { 'process.env.NODE_ENV': '"production"' },
});
await cp(join(root, 'node_modules/katex/dist/katex.min.css'), `${target}/katex.css`);
await cp(join(root, 'node_modules/katex/dist/fonts'), `${target}/fonts`, { recursive: true });
const notices = [];
async function walk(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (!entry.isDirectory() || entry.name.startsWith('.')) continue;
    const path = join(directory, entry.name); if (entry.name.startsWith('@')) { await walk(path); continue; }
    let pkg; try { pkg = JSON.parse(await readFile(join(path, 'package.json'), 'utf8')); } catch { continue; }
    if (pkg.name.includes('esbuild')) continue;
    let text = `${pkg.name}@${pkg.version} (${pkg.license || 'see package'})\n`;
    for (const file of await readdir(path)) if (/^(?:licen[cs]e|copying|notice)(?:\.|$)/i.test(file)) try { text += await readFile(join(path, file), 'utf8'); text += '\n'; } catch {}
    notices.push(text); try { await walk(join(path, 'node_modules')); } catch {}
  }
}
await walk(join(root, 'node_modules')); await writeFile(`${target}/THIRD-PARTY-NOTICES.txt`, notices.join('\n--------------------\n'));
console.log('Built fixed self-hosted Markdown, math, highlighter and isolated Mermaid engines.');
