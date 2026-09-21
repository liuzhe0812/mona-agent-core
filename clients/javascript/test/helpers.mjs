export const usage = {model_calls:1,reported_tokens:3,usage_complete:true};
export const outcome = {status:'completed',output:'hello',error:null,task_usage:usage,steps:1};
export const message = (text = '', state = 'running') => ({id:'step-1-message',step:1,state,content:{kind:'agent_message',text,truncated:false}});
export const tool = () => ({id:'step-1-tool-0',step:1,state:'pending',content:{kind:'tool_call',call_id:null,name:'',arguments_text:'',arguments:null,arguments_truncated:false,output:'',output_truncated:false,result:null,details:{}}});
export const envelope = (seq,event,run='r') => ({protocol_version:2,run_id:run,seq,event});
export const frame = (seq,event,run='r') => ({kind:'event',envelope:envelope(seq,event,run)});
export const snapshot = (seq=0,done=false) => ({protocol_version:2,run_id:'r',seq,started:seq>0,step:seq>0?1:0,items:done?[message('hello','completed')]:[],pruned_items:0,outcome:done?outcome:null});
export const frames = () => [frame(1,{type:'run/started'}),frame(2,{type:'item/started',item:message()}),frame(3,{type:'item/agentMessage/delta',item_id:'step-1-message',text:'hel'}),frame(4,{type:'item/completed',item:message('hello','completed')}),frame(5,{type:'run/completed',outcome})];
export const sse = data => data.map(f=>`id: ${f.envelope?.seq ?? f.snapshot?.seq}\nevent: agent\ndata: ${JSON.stringify(f)}\n\n`).join('');
export function bytesBody(text,chunk=1) { const bytes = new TextEncoder().encode(text); let offset=0;return new ReadableStream({pull(c){if(offset===bytes.length){c.close();return;}c.enqueue(bytes.subarray(offset,offset+chunk));offset=Math.min(bytes.length,offset+chunk);}}); }
export const sleep = ms => new Promise(r=>setTimeout(r,ms));
