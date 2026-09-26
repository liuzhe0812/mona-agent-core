#!/usr/bin/env node
import { cp, copyFile, mkdir, readFile, readdir, rm, stat } from 'node:fs/promises';
import { dirname, extname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const webRoot = resolve(root, 'apps/web');
const clientRoot = resolve(root, 'packages/client/src');
const desktopRoot = resolve(root, 'apps/desktop');
const distRoot = resolve(desktopRoot, 'dist');

if (distRoot === desktopRoot || !distRoot.startsWith(`${desktopRoot}${sep}`)) {
  throw new Error('Refusing to clear a path outside apps/desktop.');
}

function includeWebAsset(source) {
  const path = relative(webRoot, source).replaceAll('\\', '/');
  if (!path) return true;
  const parts = path.split('/');
  const name = parts.at(-1).toLowerCase();
  return !parts.includes('test')
    && path !== 'preview.html'
    && path !== 'src'
    && !path.startsWith('src/')
    && !/^readme(?:\.|$)/i.test(name)
    && !/\.(?:test|spec)\.[^.]+$/i.test(name);
}

async function* walk(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) yield* walk(path);
    else if (entry.isFile()) yield path;
  }
}

function localReferenceTarget(reference, owner) {
  const value = reference.split(/[?#]/, 1)[0];
  if (!value || value.includes('${') || value.startsWith('//') || /^[a-z][a-z\d+.-]*:/i.test(value)) return null;

  if (value.startsWith('/')) {
    if (!['/apps/web', '/packages/client/src'].some(prefix => value === prefix || value.startsWith(`${prefix}/`))) return null;
    return resolve(distRoot, value.slice(1));
  }
  if (value.startsWith('./') || value.startsWith('../')) return resolve(dirname(owner), value);
  return null;
}

async function verifyReferences() {
  const missing = new Set();
  const officeVendor = resolve(distRoot, 'apps/web/vendor/office');
  const check = async (reference, owner) => {
    const target = localReferenceTarget(reference, owner);
    if (!target) return;
    if (target !== distRoot && !target.startsWith(`${distRoot}${sep}`)) {
      missing.add(`${relative(distRoot, owner)} -> ${reference} (outside dist)`);
      return;
    }
    try {
      await stat(target);
    } catch {
      missing.add(`${relative(distRoot, owner)} -> ${reference}`);
    }
  };

  for await (const file of walk(distRoot)) {
    const extension = extname(file).toLowerCase();
    if (!['.html', '.js', '.mjs', '.css'].includes(extension)) continue;
    // Keep the validator focused on app-owned imports, not third-party bundle internals.
    if ((extension === '.js' || extension === '.mjs') && file.startsWith(`${officeVendor}${sep}`)) continue;
    const source = await readFile(file, 'utf8');

    for (const match of source.matchAll(/\b(?:src|href|poster|data-src)\s*=\s*(["'])(.*?)\1/gi)) {
      await check(match[2], file);
    }
    for (const match of source.matchAll(/url\(\s*(?:(["'])(.*?)\1|([^\s)]+))\s*\)/gi)) {
      await check(match[2] ?? match[3], file);
    }
    for (const match of source.matchAll(/@import\s*(["'])(.*?)\1/gi)) {
      await check(match[2], file);
    }

    if (extension === '.js' || extension === '.mjs') {
      for (const match of source.matchAll(/\b(?:import|export)\s+(?:[\w*\s{},]*?\s+from\s+)?(["'])([^"']+)\1/g)) {
        await check(match[2], file);
      }
      for (const match of source.matchAll(/\bimport\s*\(\s*(["'])([^"']+)\1\s*\)/g)) {
        await check(match[2], file);
      }
    }

    // Also catch first-party asset URLs assembled outside markup, such as fetch() and Worker().
    for (const match of source.matchAll(/["'`]((?:\/apps\/web|\/packages\/client\/src)(?:\/[^"'`?#\s)]*)?)(?:[?#][^"'`]*)?["'`]/g)) {
      await check(match[1], file);
    }
  }

  if (missing.size) {
    throw new Error(`Missing local desktop asset references:\n${[...missing].sort().map(value => `- ${value}`).join('\n')}`);
  }
}

await stat(resolve(webRoot, 'index.html'));
await stat(clientRoot);
await rm(distRoot, { recursive: true, force: true });
await mkdir(distRoot, { recursive: true });
await cp(webRoot, resolve(distRoot, 'apps/web'), { recursive: true, filter: includeWebAsset });
await cp(clientRoot, resolve(distRoot, 'packages/client/src'), { recursive: true });
await copyFile(resolve(webRoot, 'index.html'), resolve(distRoot, 'index.html'));
await verifyReferences();

console.log('Desktop Web assets copied; first-party local references verified in apps/desktop/dist.');
