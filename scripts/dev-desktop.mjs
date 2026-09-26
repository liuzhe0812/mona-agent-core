import { spawn } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const desktop = resolve(dirname(fileURLToPath(import.meta.url)), '../apps/desktop');
const child = spawn(process.env.CARGO || 'cargo', ['tauri', 'dev', '--no-dev-server', '--', '--locked'], {
  cwd: desktop, stdio: 'inherit', env: process.env,
});

child.on('error', error => { console.error(`无法启动 Tauri 桌面版：${error.message}`); process.exitCode = 1; });
child.on('exit', (code, signal) => { process.exitCode = code ?? (signal ? 1 : 0); });
