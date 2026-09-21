import type {AgentClient,StartRequest,StartResponse,CancelReceipt,InputRequest,InputReceipt,RunSnapshot,ResultResponse,StreamOptions,Subscription} from './index.js';
export class TauriAgentClient implements AgentClient {
  constructor(options:{invoke:(command:string,args?:Record<string,unknown>)=>Promise<unknown>;streamIdleTimeoutMs?:number;Channel:new()=>{onmessage:(packet:any)=>void}});
  start(request:StartRequest):Promise<StartResponse>;
  cancel(runId:string):Promise<CancelReceipt>;
  input(runId:string,request:InputRequest):Promise<InputReceipt>;
  snapshot(runId:string):Promise<RunSnapshot>;
  result(runId:string):Promise<ResultResponse>;
  forget(runId:string):Promise<void>;
  subscribe(runId:string,options:StreamOptions):Subscription;
}
