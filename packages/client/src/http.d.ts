import type {AgentClient,StartRequest,StartResponse,CancelReceipt,InputRequest,InputReceipt,RunSnapshot,ResultResponse,StreamOptions,Subscription} from './index.js';
export class HttpAgentClient implements AgentClient {
  constructor(options:{baseUrl:string;token:string;fetch?:typeof fetch;maxEventBytes?:number});
  start(request:StartRequest):Promise<StartResponse>;
  cancel(runId:string):Promise<CancelReceipt>;
  input(runId:string,request:InputRequest):Promise<InputReceipt>;
  snapshot(runId:string):Promise<RunSnapshot>;
  result(runId:string):Promise<ResultResponse>;
  forget(runId:string):Promise<void>;
  subscribe(runId:string,options:StreamOptions):Subscription;
}
