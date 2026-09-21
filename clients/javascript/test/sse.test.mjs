import test from 'node:test';import assert from 'node:assert/strict';
import {readSse} from '../src/sse.mjs';import {bytesBody} from './helpers.mjs';
async function parse(text,chunk=1,limit=4096){const results=[];for await(const m of readSse(bytesBody(text,chunk),limit))results.push(m);return results;}
test('SSE parses split Chinese UTF-8 and CRLF correctly',async()=>{const events=await parse('id: 7\r\nevent: agent\r\ndata: {"text":"中文🙂"}\r\n\r\n');assert.deepEqual(events,[{id:'7',event:'agent',data:'{"text":"中文🙂"}'}]);});
test('comments, multiline data and id carry-forward follow SSE framing',async()=>{const events=await parse(': heartbeat\n\nevent: agent\nid: 8\ndata: first\ndata: second\n\ndata: third\n\n',3);assert.deepEqual(events,[{id:'8',event:'agent',data:'first\nsecond'},{id:'8',event:'message',data:'third'}]);});
test('bare CR delimiters and final CR are supported',async()=>{assert.equal((await parse('data: x\r\r'))[0].data,'x');});
test('truncated final frame is rejected rather than treated as complete',async()=>{await assert.rejects(()=>parse('data: {"x":1}'),/incomplete/);});
test('invalid UTF-8 fails closed',async()=>{const body=new ReadableStream({start(c){c.enqueue(new Uint8Array([0xff]));c.close();}});await assert.rejects(async()=>{for await(const _ of readSse(body)){};});});
test('oversized single data frame is rejected',async()=>{await assert.rejects(()=>parse('data: '+ 'x'.repeat(500)+'\n\n',3,128),/byte limit/);});
test('multiple small frames in one network chunk do not count as one giant frame',async()=>{assert.equal((await parse('data: x\n\n'.repeat(200),100000,32)).length,200);});
test('reader is cancelled if consumer stops early',async()=>{let cancelled=false;const body=new ReadableStream({start(c){c.enqueue(new TextEncoder().encode('data: one\n\n'));},cancel(){cancelled=true;}});for await(const _ of readSse(body)){break;}assert.equal(cancelled,true);});
