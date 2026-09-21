import test from 'node:test';import assert from 'node:assert/strict';
import {TauriAgentClient} from '../src/tauri.mjs';import {RunView} from '../src/index.mjs';import {frames,snapshot,sleep} from './helpers.mjs';
function fixture({items=frames(),idle=false,completed=false}={}){
 const calls=[];let channel,at=0;const id='subscription-7';
 class Channel{onmessage=()=>{};}
 function next(){if(at<items.length){const packet={subscription_id:id,delivery_id:at+1,frame:items[at++]};queueMicrotask(()=>channel.onmessage(packet));}}
 const invoke=async(command,args)=>{const name=command.split('|')[1];calls.push({command,name,args});switch(name){
 case 'subscribe_events':channel=args.onEvent;if(!idle)next();return {subscription_id:id};
 case 'ack_event':next();return;
 case 'get_snapshot':return snapshot(completed?5:0,completed);
 case 'unsubscribe_events':return;
 case 'start_task':return {run_id:'r',reused:false};
 case 'cancel_task':return {run_id:'r',signalled:true};
 default:return {};
 }};
 return {client:new TauriAgentClient({invoke,Channel,streamIdleTimeoutMs:1000}),calls,invoke,Channel};
}
test('Tauri accepts packets arriving before subscribe response and awaits ACK',async()=>{const f=fixture();const v=new RunView('r');await f.client.subscribe('r',{after:0,onFrame:f=>v.apply(f)}).closed;assert.equal(v.state.seq,5);assert.equal(f.calls.filter(c=>c.name==='ack_event').length,5);assert.ok(f.calls.some(c=>c.name==='unsubscribe_events'));});
test('Tauri callback finishes before ACK is sent',async()=>{const f=fixture({items:frames().slice(-1)});let ready=false;await f.client.subscribe('r',{onFrame:async()=>{assert.equal(f.calls.some(c=>c.name==='ack_event'),false);await sleep(5);ready=true;}}).closed;assert.equal(ready,true);assert.equal(f.calls.filter(c=>c.name==='ack_event').length,1);});
test('native detach never translates to cancel_task',async()=>{const f=fixture({idle:true});const sub=f.client.subscribe('r',{onFrame:()=>{}});sub.close();await sub.closed;await sleep(1);assert.equal(f.calls.some(c=>c.name==='cancel_task'),false);assert.equal(f.calls.some(c=>c.name==='unsubscribe_events'),true);});
test('native lost channel eventually rejects with cursor instead of hanging forever',async()=>{const f=fixture({idle:true});const c=new TauriAgentClient({invoke:f.invoke,Channel:f.Channel,streamIdleTimeoutMs:15});await assert.rejects(c.subscribe('r',{after:3,onFrame:()=>{}}).closed,e=>e.code==='disconnected'&&e.cursor===3);});
test('completed cursor with no remaining packets terminates without waiting forever',async()=>{const f=fixture({idle:true,completed:true});await f.client.subscribe('r',{after:5,onFrame:()=>{throw new Error('not expected');}}).closed;});
test('UI callback failure is exposed and detaches the channel',async()=>{const f=fixture();await assert.rejects(f.client.subscribe('r',{onFrame:()=>{throw new Error('UI failed');}}).closed,/UI failed/);assert.ok(f.calls.some(c=>c.name==='unsubscribe_events'));});
test('both transport clients use the same application request shape',async()=>{const f=fixture();await f.client.start({request_id:'key',prompt:'hello'});assert.deepEqual(f.calls[0],{command:'plugin:bridge|start_task',name:'start_task',args:{request:{request_id:'key',prompt:'hello'}}});});
