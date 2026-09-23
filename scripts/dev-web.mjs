#!/usr/bin/env node
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { chmod, mkdir, readFile, stat, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { dirname, extname, resolve, sep } from 'node:path';
import { homedir } from 'node:os';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '..');
const webRoot = resolve(root, 'apps', 'web');
const CONFIG_PATH = '/__mona_dev_config__.json';
const HEALTH_PATH = '/__mona_dev_health__';
const STATIC_PREFIXES = ['/apps/web/', '/packages/client/src/'];
const MIME = new Map([
  ['.html', 'text/html; charset=utf-8'],
  ['.mjs', 'text/javascript; charset=utf-8'],
  ['.js', 'text/javascript; charset=utf-8'],
  ['.css', 'text/css; charset=utf-8'],
  ['.json', 'application/json; charset=utf-8'],
  ['.svg', 'image/svg+xml'],
  ['.png', 'image/png'],
]);

export function parseDotEnv(source) {
  const values = {};
  for (const rawLine of source.split(/\r?\n/)) {
    let line = rawLine.trim();
    if (!line || line.startsWith('#')) continue;
    if (line.startsWith('export ')) line = line.slice(7).trim();
    const equals = line.indexOf('=');
    if (equals < 1) continue;
    const key = line.slice(0, equals).trim();
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) continue;
    let value = line.slice(equals + 1).trim();
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'"))) {
      const quote = value[0];
      value = value.slice(1, -1);
      if (quote === '"') {
        value = value
          .replace(/\\n/g, '\n')
          .replace(/\\r/g, '\r')
          .replace(/\\t/g, '\t')
          .replace(/\\"/g, '"')
          .replace(/\\\\/g, '\\');
      }
    } else {
      const comment = value.search(/\s+#/);
      if (comment >= 0) value = value.slice(0, comment).trimEnd();
    }
    values[key] = value;
  }
  return values;
}

export async function loadDotEnv(env = process.env, file = resolve(root, '.env')) {
  if (!existsSync(file)) return false;
  const values = parseDotEnv(await readFile(file, 'utf8'));
  for (const [key, value] of Object.entries(values)) {
    if (env[key] == null || env[key] === '') env[key] = value;
  }
  return true;
}

function parsePositiveInt(value, name, fallback, max = Number.MAX_SAFE_INTEGER) {
  if (value == null || value === '') return fallback;
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 1 || parsed > max) {
    throw new Error(`${name} 必须是 1–${max} 的整数。`);
  }
  return parsed;
}

function loopbackHost(host) {
  return host === '127.0.0.1' || host === 'localhost' || host === '::1' || host === '[::1]';
}

function parseBindAddress(value) {
  const address = value || '127.0.0.1:8787';
  let url;
  try {
    url = new URL(`http://${address}`);
  } catch {
    throw new Error('AGENT_SERVER_ADDR 格式应为 127.0.0.1:8787。');
  }
  if (!loopbackHost(url.hostname)) {
    throw new Error('一键开发启动器只允许 Agent Server 绑定环回地址。远程部署请使用独立服务配置。');
  }
  const port = parsePositiveInt(url.port, 'AGENT_SERVER_ADDR 端口', 80, 65535);
  const browserHost = url.hostname === '::1' || url.hostname === '[::1]' ? '[::1]' : url.hostname;
  return { address, endpoint: `http://${browserHost}:${port}` };
}

function validateToken(token) {
  if (typeof token !== 'string' || token.length < 32 || token.length > 512 || !/^[\x21-\x7e]+$/.test(token)) {
    throw new Error('AGENT_SERVER_TOKEN 必须是 32–512 个可见 ASCII 字符。删除该变量可让开发启动器临时生成。');
  }
  return token;
}

export function resolveDevConfig(env = process.env) {
  const modelManagement = env.AGENT_MODEL_MANAGEMENT !== '0';
  if (!modelManagement) {
    const missing = ['AGENT_MODEL_ENDPOINT', 'AGENT_MODEL_NAME'].filter((key) => !env[key]?.trim());
    if (missing.length) {
      throw new Error(`固定模型模式缺少 ${missing.join('、')}。启用模型管理可直接在 Web UI 中配置。`);
    }
  }

  const webHost = env.MONA_WEB_HOST?.trim() || '127.0.0.1';
  if (!loopbackHost(webHost)) {
    throw new Error('MONA_WEB_HOST 只允许环回地址；该命令是本机开发入口，不是生产部署器。');
  }
  const webPort = parsePositiveInt(env.MONA_WEB_PORT, 'MONA_WEB_PORT', 4173, 65535);
  const server = parseBindAddress(env.AGENT_SERVER_ADDR?.trim());
  const token = validateToken(env.AGENT_SERVER_TOKEN?.trim() || randomBytes(32).toString('base64url'));
  const uiHostForUrl = webHost === '::1' || webHost === '[::1]' ? '[::1]' : webHost;
  const uiOrigin = `http://${uiHostForUrl}:${webPort}`;
  const stateDirectory = resolve(env.MONA_DEV_STATE_DIR?.trim()
    || (process.platform === 'win32' && env.LOCALAPPDATA
      ? resolve(env.LOCALAPPDATA, 'mona-agent-core')
      : resolve(env.XDG_STATE_HOME?.trim() || resolve(homedir(), '.local', 'state'), 'mona-agent-core')));

  return {
    webHost,
    webPort,
    uiOrigin,
    endpoint: server.endpoint,
    serverAddress: server.address,
    token,
    openBrowser: env.MONA_OPEN_BROWSER !== '0',
    startupTimeoutMs: parsePositiveInt(
      env.MONA_STARTUP_TIMEOUT_SECONDS,
      'MONA_STARTUP_TIMEOUT_SECONDS',
      300,
      86400,
    ) * 1000,
    cargoBin: env.CARGO?.trim() || 'cargo',
    modelManagement,
    stateDirectory,
    modelSettingsPath: resolve(env.AGENT_MODEL_SETTINGS_PATH?.trim() || resolve(stateDirectory, 'model-settings.enc')),
    capabilityStatePath: resolve(env.AGENT_CAPABILITY_STATE_PATH?.trim() || resolve(stateDirectory, 'capabilities.json')),
    spillDirectory: resolve(env.AGENT_SPILL_DIR?.trim() || resolve(stateDirectory, 'spill')),
    sessionsDirectory: resolve(env.AGENT_SESSIONS_DIR?.trim() || resolve(stateDirectory, 'sessions')),
  };
}

function validateStoreKey(value) {
  if (typeof value !== 'string' || value.length < 16 || value.length > 512 || !/^[\x21-\x7e]+$/.test(value)) {
    throw new Error('本地模型设置密钥无效；删除开发状态目录中的 model-store.key 后重新启动。');
  }
  return value;
}

/** Create a stable local-development key once; production hosts inject their own key. */
export async function ensureDevStoreKey(directory) {
  await mkdir(directory, { recursive: true });
  const path = resolve(directory, 'model-store.key');
  try {
    return validateStoreKey((await readFile(path, 'utf8')).trim());
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error;
  }
  const generated = randomBytes(32).toString('base64url');
  try {
    await writeFile(path, `${generated}\n`, { encoding: 'utf8', mode: 0o600, flag: 'wx' });
    if (process.platform !== 'win32') await chmod(path, 0o600);
    return generated;
  } catch (error) {
    if (error?.code !== 'EEXIST') throw error;
    return validateStoreKey((await readFile(path, 'utf8')).trim());
  }
}

function writeResponse(response, status, type, body, method = 'GET', cache = 'no-store') {
  const data = Buffer.isBuffer(body)
    ? body
    : Buffer.from(typeof body === 'string' ? body : JSON.stringify(body));
  response.writeHead(status, {
    'Content-Type': type,
    'Content-Length': data.length,
    'Cache-Control': cache,
    'Referrer-Policy': 'no-referrer',
    'X-Content-Type-Options': 'nosniff',
  });
  if (method !== 'HEAD') response.end(data);
  else response.end();
}

function safeStaticPath(pathname) {
  if (pathname === '/' || pathname === '/index.html') return resolve(webRoot, 'index.html');
  if (!STATIC_PREFIXES.some((prefix) => pathname.startsWith(prefix))) return null;

  let decoded;
  try {
    decoded = decodeURIComponent(pathname);
  } catch {
    return null;
  }
  if (decoded.includes('\0') || decoded.split('/').includes('..')) return null;

  const candidate = resolve(root, `.${decoded}`);
  const allowedRoots = [webRoot, resolve(root, 'packages', 'client', 'src')];
  if (!allowedRoots.some((allowed) => candidate === allowed || candidate.startsWith(`${allowed}${sep}`))) {
    return null;
  }
  return candidate;
}

export async function startUiServer({ host, port, endpoint, token }) {
  const server = createServer(async (request, response) => {
    const method = request.method || 'GET';
    if (method !== 'GET' && method !== 'HEAD') {
      writeResponse(response, 405, 'application/json; charset=utf-8', { error: 'method_not_allowed' }, method);
      return;
    }

    const url = new URL(request.url || '/', `http://${host}:${port || 80}`);
    if (url.pathname === CONFIG_PATH) {
      writeResponse(response, 200, 'application/json; charset=utf-8', { endpoint, token }, method);
      return;
    }
    if (url.pathname === HEALTH_PATH) {
      writeResponse(response, 200, 'application/json; charset=utf-8', { ok: true }, method);
      return;
    }
    if (url.pathname === '/favicon.ico') {
      response.writeHead(204, { 'Cache-Control': 'no-store' });
      response.end();
      return;
    }

    const file = safeStaticPath(url.pathname);
    if (!file) {
      writeResponse(response, 404, 'application/json; charset=utf-8', { error: 'not_found' }, method);
      return;
    }

    try {
      const info = await stat(file);
      if (!info.isFile()) throw new Error('not a file');
      const content = await readFile(file);
      writeResponse(
        response,
        200,
        MIME.get(extname(file)) || 'application/octet-stream',
        content,
        method,
        'no-store',
      );
    } catch {
      writeResponse(response, 404, 'application/json; charset=utf-8', { error: 'not_found' }, method);
    }
  });

  await new Promise((resolveListen, rejectListen) => {
    server.once('error', rejectListen);
    server.listen(port, host, () => {
      server.off('error', rejectListen);
      resolveListen();
    });
  });
  return server;
}

async function waitForRuntime(endpoint, token, timeoutMs, child) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (child.exitCode != null) {
      throw new Error(`Rust Runtime 在就绪前退出，退出码 ${child.exitCode}。`);
    }
    try {
      const response = await fetch(`${endpoint}/v1/info`, {
        headers: { Authorization: `Bearer ${token}` },
        cache: 'no-store',
        signal: AbortSignal.timeout(1500),
      });
      if (response.ok) {
        const info = await response.json();
        if (info.protocol_version !== 2 || info.stream !== 'sse') {
          throw new Error('Agent Server 未提供 Stream v2 / SSE。');
        }
        return;
      }
    } catch (error) {
      if (error?.message?.includes('Stream v2')) throw error;
    }
    await new Promise((resolveSleep) => setTimeout(resolveSleep, 300));
  }
  throw new Error(
    `等待 Rust Runtime 超时（${Math.round(timeoutMs / 1000)} 秒）。首次编译较慢时可设置 MONA_STARTUP_TIMEOUT_SECONDS。`,
  );
}

function openBrowser(url) {
  let command;
  let args;
  if (process.platform === 'win32') {
    command = 'cmd';
    args = ['/d', '/s', '/c', 'start', '', url];
  } else if (process.platform === 'darwin') {
    command = 'open';
    args = [url];
  } else {
    command = 'xdg-open';
    args = [url];
  }
  const opener = spawn(command, args, { stdio: 'ignore', detached: true });
  opener.on('error', () => {});
  opener.unref();
}

async function terminate(child) {
  if (!child || child.exitCode != null || !child.pid) return;
  if (process.platform === 'win32') {
    await new Promise((resolveKill) => {
      const killer = spawn('taskkill', ['/pid', String(child.pid), '/t', '/f'], { stdio: 'ignore' });
      killer.once('error', resolveKill);
      killer.once('exit', resolveKill);
    });
    return;
  }

  try {
    process.kill(-child.pid, 'SIGINT');
  } catch {
    try { child.kill('SIGINT'); } catch {}
  }
  await Promise.race([
    new Promise((resolveExit) => child.once('exit', resolveExit)),
    new Promise((resolveDelay) => setTimeout(resolveDelay, 5000)),
  ]);
  if (child.exitCode == null) {
    try {
      process.kill(-child.pid, 'SIGKILL');
    } catch {
      try { child.kill('SIGKILL'); } catch {}
    }
  }
}

export async function main() {
  await loadDotEnv();
  const config = resolveDevConfig();
  if (!existsSync(resolve(webRoot, 'index.html'))) {
    throw new Error('缺少 apps/web/index.html，无法启动标准 Web UI。');
  }

  const childEnv = {
    ...process.env,
    AGENT_SERVER_TOKEN: config.token,
    AGENT_SERVER_ADDR: config.serverAddress,
    AGENT_UI_ORIGIN: config.uiOrigin,
    AGENT_CAPABILITY_STATE_PATH: config.capabilityStatePath,
    AGENT_SPILL_DIR: config.spillDirectory,
    AGENT_SESSIONS_DIR: config.sessionsDirectory,
  };
  if (config.modelManagement) {
    childEnv.AGENT_MODEL_STORE_KEY = process.env.AGENT_MODEL_STORE_KEY?.trim()
      || await ensureDevStoreKey(config.stateDirectory);
    childEnv.AGENT_MODEL_SETTINGS_PATH = config.modelSettingsPath;
  }

  const uiServer = await startUiServer({
    host: config.webHost,
    port: config.webPort,
    endpoint: config.endpoint,
    token: config.token,
  });
  let shuttingDown = false;
  let runtime;

  const shutdown = async (reason, code = 0) => {
    if (shuttingDown) return;
    shuttingDown = true;
    if (reason) console.log(`\n${reason}`);
    await new Promise((resolveClose) => uiServer.close(() => resolveClose()));
    await terminate(runtime);
    process.exitCode = code;
  };

  try {
    console.log('Mona Agent Web 开发环境');
    console.log(`- Web UI:       ${config.uiOrigin}`);
    console.log(`- Agent API:    ${config.endpoint}`);
    console.log('- Bridge token: 已临时生成/读取，不会输出到终端');
    console.log(`- Runtime:      ${config.modelManagement ? '模型管理模式（首次在设置页配置）' : '固定模型模式'}`);
    console.log('\n正在启动并等待 Rust Runtime；首次 cargo 编译可能需要几分钟……\n');

    runtime = spawn(config.cargoBin, ['run', '-p', 'server'], {
      cwd: root,
      env: childEnv,
      stdio: 'inherit',
      detached: process.platform !== 'win32',
    });
    runtime.once('error', (error) => {
      if (!shuttingDown) void shutdown(`无法启动 cargo：${error.message}`, 1);
    });
    runtime.once('exit', (code, signal) => {
      if (!shuttingDown) {
        void shutdown(`Rust Runtime 已退出（${signal ?? code ?? 'unknown'}）。`, code || 1);
      }
    });

    await waitForRuntime(config.endpoint, config.token, config.startupTimeoutMs, runtime);
    console.log('\nRuntime 已就绪，Web UI 会自动使用 HTTP/SSE Bridge 连接。');
    if (config.openBrowser) openBrowser(config.uiOrigin);
    else console.log(`请打开：${config.uiOrigin}`);

    process.once('SIGINT', () => { void shutdown('正在停止 Web UI 与 Runtime……', 130); });
    process.once('SIGTERM', () => { void shutdown('正在停止 Web UI 与 Runtime……', 143); });
    await new Promise(() => {});
  } catch (error) {
    await shutdown(error instanceof Error ? error.message : String(error), 1);
  }
}

const invokedAsScript = process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url;
if (invokedAsScript) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  });
}
