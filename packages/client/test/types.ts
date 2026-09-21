import {RunView, type AgentClient, requestId} from 'client';
import {HttpAgentClient} from 'client/http';
import {TauriAgentClient} from 'client/tauri';
const http: AgentClient = new HttpAgentClient({baseUrl:'https://example.test',token:'not-a-real-token'.repeat(3)});
declare const invoke: (cmd:string,args?:Record<string,unknown>)=>Promise<unknown>;
declare const Channel: new()=>{onmessage:(message:unknown)=>void};
const tauri: AgentClient = new TauriAgentClient({invoke,Channel,streamIdleTimeoutMs:120000});
async function consume(client:AgentClient) {const started=await client.start({request_id:requestId(),prompt:'hello'});const view=new RunView(started.run_id);const sub=client.subscribe(started.run_id,{after:0,onFrame(frame){view.apply(frame);}});await sub.closed;return client.result(started.run_id);}
void consume;void http;void tauri;
