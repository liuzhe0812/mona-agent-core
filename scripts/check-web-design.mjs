#!/usr/bin/env node
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { dirname, extname, join, relative, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '..');
const FEATURE_CSS = ['apps/web/styles.css', 'apps/web/sessions.css', 'apps/web/appearance.css', 'apps/web/workspace.css', 'apps/web/memory.css', 'apps/web/conversation.css', 'apps/web/ui/extensions.css'];
const OWNERSHIP_ROOTS = ['apps/web', 'docs/ui'];
const OWNERSHIP_TEXT_EXTENSIONS = new Set(['.css', '.html', '.js', '.json', '.md', '.mjs', '.toml', '.ts', '.txt', '.yaml', '.yml']);
const LEGACY_EXTERNAL_NAME = ['cin', 'dy'].join('');
const RUNTIME_JS = [
  'apps/web/layout-geometry.mjs', 'apps/web/shell-layout.mjs', 'apps/web/overlay-scope.mjs',
  'apps/web/pane-layout.mjs', 'apps/web/pane-controls.mjs',
  'apps/web/process-scroll.mjs',
  'apps/web/conversation-policy.mjs', 'apps/web/conversation-metrics.mjs', 'apps/web/workspace-files.mjs',
  'apps/web/conversation-find.mjs', 'apps/web/diff-view.mjs',
  'apps/web/pane-state.mjs',
  'apps/web/file-reference-cards.mjs',
  'apps/web/content-dom.mjs', 'apps/web/diagram-view.mjs', 'apps/web/conversation-controls.mjs', 'apps/web/presentation-path.mjs',
  'apps/web/pane-icons.mjs', 'apps/web/file-preview.mjs', 'apps/web/workbench-ui.mjs',
  'apps/web/app.mjs', 'apps/web/appearance.mjs', 'apps/web/theme.mjs',
  'apps/web/run-view.mjs', 'apps/web/content-renderer.mjs', 'apps/web/sessions-ui.mjs', 'apps/web/tooltip.mjs',
  'apps/web/conversation-rail.mjs', 'apps/web/workspace-ui.mjs', 'apps/web/right-pane.mjs', 'apps/web/side-conversation.mjs',
  'apps/web/memory-ui.mjs',
  ...['registry','host','client','shell','commands','command-menu','dom','catalog','plan-view','subagent-view','mcp-view'].map(id => `apps/web/ui/${id}.mjs`),
  ...['models','capabilities','appearance','memory','workspace','workbench','side','metrics','details','planner','sandbox','subagent','mcp'].map(id => `apps/web/ui/modules/${id}.mjs`),
];
const REQUIRED_DECLARATIONS = Object.freeze({
  '--ui-control-sm': '32px', '--ui-control-md': '36px', '--ui-control-lg': '40px',
  '--ui-radius-inner': '8px', '--ui-settings-sidebar-width': '232px',
  '--ui-settings-content-max': '1040px', '--ui-topbar-height': '48px',
  '--ui-activity-row-height': '28px', '--ui-settings-row-height': '72px',
  '--ui-list-row-height': '52px', '--ui-provider-sidebar-width': '240px',
});
const RUNTIME_STYLE_ALLOW = Object.freeze({
  'apps/web/ui/command-menu.mjs': [
    /^this\.root\.style\.setProperty\('--composer-menu-width', `\$\{width\}px`\);$/,
    /^this\.root\.style\.setProperty\('--composer-menu-height', `\$\{Math\.max\(0, height\)\}px`\);$/,
    /^this\.root\.style\.setProperty\('--pane-menu-[xy]', `\$\{Math\.round\([xy]\)\}px`\);$/,
  ],
  'apps/web/pane-controls.mjs': [
    /^this\.root\.style\.setProperty\('--pane-menu-[xy]', `\$\{Math\.round\([xy]\)\}px`\);$/,
  ],  'apps/web/conversation-controls.mjs': [
    /^main\.parentElement\.style\.setProperty\('--conversation-dock-height', `\$\{height\}px`\);$/,
  ],
  'apps/web/shell-layout.mjs': [
    /^if \(this\.shell\.style\.getPropertyValue\('--sidebar-width'\) !== width\) this\.shell\.style\.setProperty\('--sidebar-width', width\);$/,
    /^if \(this\.workspace\.style\.getPropertyValue\('--conversation-scrollbar-width'\) !== scrollbar\) this\.workspace\.style\.setProperty\('--conversation-scrollbar-width', scrollbar\);$/,
    /^if \(this\.composer\.style\.getPropertyValue\('--composer-draft-height'\) !== desired\) this\.composer\.style\.setProperty\('--composer-draft-height', desired\);$/,
  ],
  'apps/web/theme.mjs': [
    /^for \(const key of PALETTE_KEYS\) element\.style\.setProperty\(`--\$\{key\}`, palette\[key\]\);$/,
    /^element\.style\.setProperty\('--chat-font-size', `\$\{preference\.fontSize\}px`\);$/,
    /^for \(const \[key, value\] of Object\.entries\(\{ composer, bubble, panel, detail \}\)\) element\.style\.setProperty\(`--skin-\$\{key\}-radius`, `\$\{value\}px`\);$/,
    /^element\.style\.setProperty\('--switch-thumb', palette\.inverse\);$/,
    /^this\.root\.style\.colorScheme = this\.mode;$/,
  ],
  'apps/web/tooltip.mjs': [
    /^tip\.style\.(?:left|top) = /,
  ],
  'apps/web/sessions-ui.mjs': [
    /^menu\.style\.(?:left|top) = /,
  ],
  'apps/web/workspace-ui.mjs': [
    /^this\.projectMenu\.style\.(?:left|top) = /,
  ],
  'apps/web/right-pane.mjs': [
    /^this\.pane\.style\.setProperty\('--right-split-first', `\$\{ratio\}fr`\);$/,
    /^this\.pane\.style\.setProperty\('--right-split-last', `\$\{100 - ratio\}fr`\);$/,
    /^if \(this\.shell\.style\.getPropertyValue\('--right-pane-width'\) !== width\) this\.shell\.style\.setProperty\('--right-pane-width', width\);$/,
  ],
  'apps/web/conversation-rail.mjs': [
    /^preview\.style\.top = `\$\{top\}px`;$/,
  ],
});
const RUNTIME_LAYOUT_MARKERS = Object.freeze({
  'apps/web/ui/command-menu.mjs': [
    'const COMMAND_MENU_GAP = 4;', 'const COMMAND_MENU_EDGE = 12;', 'const COMMAND_MENU_MAX_HEIGHT = 400;',
  ],
  'apps/web/layout-geometry.mjs': [
    'const SIDEBAR_MIN_WIDTH = 208;', 'const SIDEBAR_MAX_WIDTH = 460;',
    'const SIDEBAR_DEFAULT_WIDTH = 264;', 'const SIDEBAR_KEYBOARD_STEP = 16;', 'const CENTER_MIN_WIDTH = 400;',
  ],
  'apps/web/sessions-ui.mjs': [
    'const SESSION_MENU_VIEWPORT_GUTTER = 8;', 'const SESSION_MENU_ANCHOR_GAP = 4;',
  ],
  'apps/web/conversation-rail.mjs': [
    'const NAVIGATOR_MIN_WIDTH_PX = 864;', 'const PREVIEW_VIEWPORT_GUTTER_PX = 16;',
  ],
});
const REQUIRED_TOKENS = [
  '--ui-font', '--ui-font-code', '--ui-text-13', '--ui-text-14', '--ui-space-2', '--ui-space-4',
  '--ui-control-sm', '--ui-control-md', '--ui-control-lg', '--ui-radius-inner', '--ui-radius-container',
  '--ui-radius-pill', '--ui-settings-sidebar-width', '--ui-settings-content-max', '--surface',
  '--surface-elevated', '--border-default', '--text-primary', '--text-secondary', '--focus-ring',
  '--motion-hover', '--motion-panel',
];

function withoutComments(source) {
  return source.replace(/\/\*[\s\S]*?\*\//g, '');
}

function lineOf(source, offset) {
  return source.slice(0, offset).split('\n').length;
}

function violation(file, source, match, message) {
  return { file, line: lineOf(source, match.index ?? 0), message, value: String(match[0] ?? '').trim() };
}

export function auditFeatureCss(source, file = '<css>') {
  const clean = withoutComments(source);
  const violations = [];
  const scans = [
    {
      pattern: /#[0-9a-f]{3,8}\b|\b(?:rgb|rgba|hsl|hsla)\s*\(/gi,
      message: 'Feature CSS must consume semantic color tokens; raw colors belong in design-system.css or theme data.',
    },
    {
      pattern: /font-size\s*:\s*([^;}]*)/gi,
      invalid: value => !/(?:var\(--(?:ui-text-[^)]+|chat-font-size)\)|\b1em\b)/.test(value),
      message: 'Font sizes must use the Mona typography tokens (or the registered 1em preview inheritance).',
    },
    {
      pattern: /font-weight\s*:\s*([^;}]*)/gi,
      invalid: value => !/var\(--ui-weight-(?:regular|medium|semibold)\)/.test(value),
      message: 'UI font weights must use the Mona weight ladder.',
    },
    {
      pattern: /border-radius\s*:\s*([^;}]*)/gi,
      invalid: value => !/var\(--(?:ui-radius-[^)]+|skin-[^)]+)\)/.test(value),
      message: 'Visible layers must use the registered radius tiers or bounded skin radius roles.',
    },
    {
      pattern: /(?:transition|animation)(?:-[a-z-]+)?\s*:\s*([^;}]*)/gi,
      invalid: value => /\b\d+(?:\.\d+)?m?s\b/i.test(value),
      message: 'Motion duration must use the shared motion tokens.',
    },
  ];
  for (const scan of scans) {
    for (const match of clean.matchAll(scan.pattern)) {
      if (!scan.invalid || scan.invalid(match[1] ?? match[0])) violations.push(violation(file, clean, match, scan.message));
    }
  }
  for (const match of clean.matchAll(/box-shadow\s*:\s*([^;}]*)/gi)) {
    const value = match[1];
    if (!/var\(--(?:focus-ring-soft|ui-shadow-hover-card|ui-shadow-composer)\)/.test(value) && !/^\s*none\s*$/i.test(value)) {
      violations.push(violation(file, clean, match, 'Ad-hoc elevation shadows are forbidden; only the shared focus indicator is allowed in formal feature CSS.'));
    }
  }
  return violations;
}

export function auditRuntimeJs(source, file = '<js>') {
  const violations = [];
  const allowed = RUNTIME_STYLE_ALLOW[file] || [];
  source.split(/\r?\n/).forEach((line, index) => {
    const trimmed = line.trim();
    if (!/(?:\.style\.|setProperty\(|setAttribute\(['"]style)/.test(trimmed)) return;
    if (allowed.some(pattern => pattern.test(trimmed))) return;
    violations.push({
      file, line: index + 1,
      message: 'Runtime UI code must not bypass the design system with unregistered inline styles.',
      value: trimmed,
    });
  });
  return violations;
}

export function auditHtml(source, file = 'apps/web/index.html') {
  const violations = [];
  for (const id of ['provider-dialog', 'model-dialog', 'model-picker-dialog', 'connection-dialog']) {
    const start = source.indexOf(`<dialog id="${id}"`);
    const end = start < 0 ? -1 : source.indexOf('</dialog>', start);
    if (start < 0 || end < 0) {
      violations.push({ file, line: 1, message: `Missing formal dialog ${id}.`, value: id });
      continue;
    }
    const fragment = source.slice(start, end);
    if (/dialog-head[\s\S]*?aria-label="关闭"/.test(fragment) && /dialog-actions[\s\S]*?value="cancel"/.test(fragment)) {
      violations.push({ file, line: lineOf(source, start), message: 'Dismissible form dialogs must not combine a top-right close icon with a footer Cancel action.', value: id });
    }
  }
  if (/style\s*=/.test(source)) violations.push({ file, line: 1, message: 'Formal HTML must not carry inline style attributes; use the design-system stylesheets.', value: 'style=' });
  return violations;
}

function ownershipFiles(base) {
  const files = [];
  const walk = directory => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      if (entry.name === '.git' || entry.name === '.tmp-verify' || entry.name === 'node_modules' || entry.name === 'target') continue;
      const absolute = join(directory, entry.name);
      if (entry.isDirectory()) walk(absolute);
      else if (OWNERSHIP_TEXT_EXTENSIONS.has(extname(entry.name).toLowerCase())) {
        files.push(relative(base, absolute).replaceAll('\\', '/'));
      }
    }
  };
  for (const rootPath of OWNERSHIP_ROOTS) {
    const absolute = resolve(base, rootPath);
    if (existsSync(absolute)) walk(absolute);
  }
  for (const file of ['AGENTS.md', 'apps/README.md']) {
    if (existsSync(resolve(base, file))) files.push(file);
  }
  return [...new Set(files)].sort();
}

export function auditMonaOwnership(base = root) {
  const violations = [];
  for (const file of ownershipFiles(base)) {
    const lowerPath = file.toLowerCase();
    if (lowerPath.includes(LEGACY_EXTERNAL_NAME)) {
      violations.push({ file, line: 1, message: 'Mona design assets must not retain an external product namespace in their paths.', value: file });
      continue;
    }
    const source = readFileSync(resolve(base, file), 'utf8');
    const offset = source.toLowerCase().indexOf(LEGACY_EXTERNAL_NAME);
    if (offset >= 0) violations.push({
      file, line: lineOf(source, offset),
      message: 'Mona design assets must remain self-contained and must not name a legacy external design system.',
      value: 'legacy external design reference',
    });
  }
  return violations;
}

export function auditDesignSystem(base = root) {
  const violations = [];
  const read = path => readFileSync(resolve(base, path), 'utf8');
  const design = read('apps/web/design-system.css');
  violations.push(...auditMonaOwnership(base));
  for (const token of REQUIRED_TOKENS) {
    if (!design.includes(`${token}:`)) violations.push({ file: 'apps/web/design-system.css', line: 1, message: `Required design token is missing: ${token}`, value: token });
  }
  for (const [token, value] of Object.entries(REQUIRED_DECLARATIONS)) {
    if (!design.includes(`${token}: ${value};`)) violations.push({
      file: 'apps/web/design-system.css', line: 1,
      message: `The governed component value changed without updating the design contract: ${token}`,
      value: `${token}: ${value}`,
    });
  }
  for (const file of FEATURE_CSS) violations.push(...auditFeatureCss(read(file), file));
  for (const file of RUNTIME_JS) violations.push(...auditRuntimeJs(read(file), file));
  for (const [file, markers] of Object.entries(RUNTIME_LAYOUT_MARKERS)) {
    const source = read(file);
    for (const marker of markers) {
      if (!source.includes(marker)) violations.push({
        file, line: 1, message: 'Runtime layout constants have drifted from the governed CSS contract.', value: marker,
      });
    }
  }
  const styles = read('apps/web/styles.css');
  if (!/^@import '\.\/design-system\.css';/m.test(styles)) {
    violations.push({ file: 'apps/web/styles.css', line: 1, message: 'The formal UI stylesheet must import design-system.css first.', value: '@import' });
  }
  for (const legacy of ['56px 54px', 'min-height: 116px', 'font-size: 25px', 'font-size: 26px']) {
    for (const file of FEATURE_CSS) {
      const source = read(file), offset = source.indexOf(legacy);
      if (offset >= 0) violations.push({ file, line: lineOf(source, offset), message: 'Legacy oversized settings geometry has returned.', value: legacy });
    }
  }
  violations.push(...auditHtml(read('apps/web/index.html')));
  return {
    violations,
    files: ['apps/web/design-system.css', ...FEATURE_CSS, ...RUNTIME_JS, 'apps/web/index.html'],
    requiredTokens: REQUIRED_TOKENS.length,
  };
}

function main() {
  const result = auditDesignSystem();
  if (result.violations.length) {
    for (const item of result.violations) console.error(`${item.file}:${item.line} ${item.message}\n  ${item.value}`);
    console.error(`Web design check failed with ${result.violations.length} violation(s).`);
    process.exitCode = 1;
    return;
  }
  console.log(`Web design check passed: ${result.files.length} formal files, ${result.requiredTokens} required tokens.`);
}

if (process.argv[1] && pathToFileURL(process.argv[1]).href === import.meta.url) main();
