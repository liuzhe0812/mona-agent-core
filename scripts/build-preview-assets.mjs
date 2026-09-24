// Builds audited, pinned libraries into self-hosted lazy assets; application startup needs no npm install.
// npm ci --prefix .tmp-verify/preview-build --ignore-scripts --no-audit --no-fund
// node scripts/build-preview-assets.mjs
import { createRequire } from 'node:module';
import { mkdir, copyFile, readdir, readFile, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
const modules = resolve('.tmp-verify/preview-build/node_modules');
const require = createRequire(join(modules, '_loader.cjs'));
const { build } = require('esbuild'); const target = 'apps/web/vendor/office'; await mkdir(target, { recursive: true });
for (const kind of ['docx', 'pptx', 'xlsx']) {
  await build({ entryPoints: [`apps/web/preview-engine/${kind}.mjs`], nodePaths: [modules], bundle: true, format: 'iife', platform: 'browser', minify: true,
    define: { 'process.env.NODE_ENV': '"production"' }, outfile: `${target}/${kind}.js`, legalComments: 'eof', external: ['pdfjs-dist/*'] });
}
await copyFile(join(modules, '@extend-ai/react-xlsx/dist/duke_sheets_wasm_bg.wasm'), `${target}/sheets.wasm`);
const notices = [];
async function scan(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (!entry.isDirectory() || entry.name.startsWith('.')) continue;
    const path = join(directory, entry.name);
    if (entry.name.startsWith('@')) { await scan(path); continue; }
    let p; try { p = JSON.parse(await readFile(join(path, 'package.json'), 'utf8')); } catch { continue; }
    if (/esbuild/.test(p.name)) continue;
    const licenses = (await readdir(path)).filter(name => /^(?:licen[cs]e|copying|notice)(?:\.|$)/i.test(name));
    let text = `${p.name}@${p.version} (${p.license || 'see package'})\n`;
    for (const name of licenses) { try { text += await readFile(join(path, name), 'utf8'); text += '\n'; } catch {} }
    notices.push(text);
  }
}
await scan(modules); await writeFile(`${target}/THIRD-PARTY-NOTICES.txt`, notices.join('\n------------------------------\n'));
await copyFile('.tmp-verify/preview-build/package.json', `${target}/build-package.json`);
await copyFile('.tmp-verify/preview-build/package-lock.json', `${target}/build-package-lock.json`);
console.log('Built self-hosted docx / pptx / xlsx previews and retained license notices.');
