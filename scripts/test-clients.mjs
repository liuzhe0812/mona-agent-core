import {readdirSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import {spawnSync} from 'node:child_process';
import {join} from 'node:path';
const dir=fileURLToPath(new URL('../packages/client/test/',import.meta.url));
const tests=readdirSync(dir).filter(f=>f.endsWith('.test.mjs')).sort().map(f=>join(dir,f));
const result=spawnSync(process.execPath,['--test',...tests],{stdio:'inherit'});
if(result.error){console.error(result.error);process.exit(1);}process.exit(result.status ?? 1);
