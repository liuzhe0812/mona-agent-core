import test from 'node:test';
import assert from 'node:assert/strict';
import { WorkspaceClient, relativePath } from './workspace.mjs';

test('file pages cannot mix another path, version, offset or byte count', async () => {
  const original = globalThis.fetch; const client = new WorkspaceClient(); client.configure('http://localhost:1', 'token');
  const valid = {path:'note.txt',name:'note.txt',kind:'text',text:'中',revision:'v1',offset:0,next_offset:3,bytes:6,eof:false};
  let value = valid, calls = 0;
  globalThis.fetch = async () => { calls++; return Response.json(value); };
  try {
    assert.equal((await client.read('s-one','note.txt',{revision:'v1'})).text, '中');
    for (const change of [{path:'other.txt'}, {revision:'v2'}, {offset:1}, {next_offset:7}, {text:'x'}, {eof:true}]) {
      value = {...valid,...change}; await assert.rejects(() => client.read('s-one','note.txt',{revision:'v1'}));
    }
    const before = calls;
    for (const options of [{offset:-1},{offset:1.5},{limit:3},{limit:65537}]) await assert.rejects(() => client.read('s-one','note.txt',options));
    assert.equal(calls,before);
  } finally { client.clear(); globalThis.fetch = original; }
});

test('workspace transport uses opaque session identity, relative paths and header authorization', async () => {
  const original = globalThis.fetch; let call;
  globalThis.fetch = async (url, options) => { call = { url, options }; return Response.json({ path: new URL(url).searchParams.get('path'), entries: [], revision: 'v1', next_offset: null }); };
  try {
    const client = new WorkspaceClient(); client.configure('http://127.0.0.1:8787/', 'secret');
    await client.list('s-one', 'docs/材料', { offset: 0 });
    assert.equal(new URL(call.url).pathname, '/api/sessions/s-one/files');
    assert.equal(new URL(call.url).searchParams.get('path'), 'docs/材料');
    assert.equal(call.options.headers.Authorization, 'Bearer secret');
    assert.equal(call.options.credentials, 'omit'); assert.equal(call.options.redirect, 'error'); assert.ok(!call.url.includes('secret'));
    const before = call;
    for (const value of ['../secret', '/etc/passwd', 'C:/secret', 'a\\b', 'a:stream']) await assert.rejects(() => client.read('s-one', value));
    assert.equal(call, before);
  } finally { globalThis.fetch = original; }
});
test('changing authority discards a late response and malformed pages never reach the view', async () => {
  const original = globalThis.fetch; let release;
  globalThis.fetch = () => new Promise(resolve => { release = resolve; });
  const client = new WorkspaceClient(); client.configure('http://localhost:1', 'first');
  try {
    const request = client.settings(); client.configure('http://localhost:2', 'second');
    release(Response.json({ revision: 0, default_root: '/old' })); await assert.rejects(request, e => e.name === 'AbortError');
    globalThis.fetch = async () => Response.json({ entries: [{name:'bad',path:'../bad',kind:'file'}], revision:'v1', next_offset:null });
    await assert.rejects(() => client.list('s-one'));
    globalThis.fetch = async () => Response.json({ kind:'text',text:'x',revision:'v1',bytes:10,next_offset:0,eof:false });
    await assert.rejects(() => client.read('s-one','file.txt'));
  } finally { client.clear(); globalThis.fetch = original; }
});
test('project writes carry revisions and files retain explicit version conflicts', async () => {
  const original = globalThis.fetch; const calls=[];
  const client = new WorkspaceClient(); client.configure('http://localhost:1','secret');
  globalThis.fetch = async (url, options) => {calls.push({url,options});return Response.json({ revision:2,projects:[] });};
  try {
    await client.addProject({ request_id:'first', revision:1, name:'Project', path:'/work' });
    assert.deepEqual(JSON.parse(calls[0].options.body),{request_id:'first',revision:1,name:'Project',path:'/work'});
    await client.createProject('named',2,'新项目');
    assert.equal(new URL(calls[1].url).pathname,'/api/projects/create');
    assert.deepEqual(JSON.parse(calls[1].options.body),{request_id:'named',revision:2,name:'新项目'});
    await client.removeProject('p-one',2);assert.equal(calls[2].options.method,'POST');
    globalThis.fetch = async () => Response.json({code:'conflict',message:'changed'}, {status:409});
    await assert.rejects(() => client.read('s-one','a',{revision:'old'}),e=>e.status===409&&e.code==='conflict');
  } finally {client.clear();globalThis.fetch=original;}
});
test('oversized responses and requests cancelled by their view are rejected', async () => {
  const original = globalThis.fetch; const client=new WorkspaceClient();client.configure('http://localhost:1','token');
  try {
    globalThis.fetch=async()=>new Response('{}',{headers:{'content-length':String(7*1024*1024)}});
    await assert.rejects(()=>client.settings(),/限制/);
    const abort=new AbortController();abort.abort();
    globalThis.fetch=async()=>Response.json({});
    await assert.rejects(()=>client.info('s-one',abort.signal),e=>e.name==='AbortError');
    assert.equal(relativePath('docs/readme.md'),'docs/readme.md');
  } finally{client.clear();globalThis.fetch=original;}
});
