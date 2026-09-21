export const PROTOCOL_VERSION: 2;
export const UI_TEXT_BYTES: number;
export const UI_RETAINED_ITEMS: number;
export class BridgeError extends Error {
  code: string; status?: number; cursor?: number;
  constructor(message: string, details?: {code?:string;status?:number;cursor?:number});
}
export type Json = null | boolean | number | string | Json[] | {[key:string]:Json};
export interface AgentError {code:string;message:string}
export interface TaskUsage {model_calls:number;reported_tokens:number;usage_complete:boolean}
export type RunStatus = 'completed'|'failed'|'cancelled'|'timed_out'|'limited';
export type ItemState = 'pending'|'running'|'completed'|'failed'|'cancelled'|'denied'|'skipped'|'unknown';
export interface ToolResult {call_id:string;status:'success'|'error'|'denied'|'skipped'|'unknown';content:string;truncated:boolean;original_bytes:number;artifact:{uri:string;bytes:number}|null}
export type ItemContent = {kind:'agent_message';text:string;truncated:boolean} | {
  kind:'tool_call';call_id:string|null;name:string;arguments_text:string;arguments:Json;
  arguments_truncated:boolean;output:string;output_truncated:boolean;result:ToolResult|null;details:Record<string,Json>;
};
export interface WorkItem {id:string;step:number;state:ItemState;content:ItemContent}
export interface RunOutcome {status:RunStatus;output:string|null;error:AgentError|null;task_usage:TaskUsage;steps:number}
export interface RunSnapshot {protocol_version:2;run_id:string;seq:number;started:boolean;step:number;items:WorkItem[];pruned_items:number;outcome:RunOutcome|null}
export type RunEvent = {type:'run/started'|'input/applied'} |
  {type:'step/started'|'step/completed';step:number} |
  {type:'item/started'|'item/updated'|'item/completed';item:WorkItem} |
  {type:'item/agentMessage/delta'|'item/toolCall/outputDelta';item_id:string;text:string} |
  {type:'item/toolCall/argumentsDelta';item_id:string;call_id:string|null;name:string|null;delta:string} |
  {type:'run/completed';outcome:RunOutcome};
export interface EventEnvelope {protocol_version:2;run_id:string;seq:number;event:RunEvent}
export type StreamFrame = {kind:'event';envelope:EventEnvelope} |
  {kind:'snapshot';reason:'initial'|'cursor_expired'|'source_resync';snapshot:RunSnapshot} |
  {kind:'fault';error:AgentError};
export interface StartRequest {request_id:string;prompt:string}
export interface StartResponse {run_id:string;reused:boolean}
export interface InputRequest {request_id:string;text:string}
export interface InputReceipt {request_id:string;applied:boolean}
export interface CancelReceipt {run_id:string;signalled:boolean}
export interface ResultResponse {run_id:string;outcome:RunOutcome|null}
export interface StreamOptions {after?:number;onFrame:(frame:StreamFrame)=>void|Promise<void>;signal?:AbortSignal}
export interface Subscription {closed:Promise<void>;close:()=>void}
export interface AgentClient {
  start(request:StartRequest):Promise<StartResponse>;
  cancel(runId:string):Promise<CancelReceipt>;
  input(runId:string,request:InputRequest):Promise<InputReceipt>;
  snapshot(runId:string):Promise<RunSnapshot>;
  result(runId:string):Promise<ResultResponse>;
  forget(runId:string):Promise<void>;
  subscribe(runId:string,options:StreamOptions):Subscription;
}
export function requestId():string;
export function frameSequence(frame:StreamFrame):number|undefined;
export function isTerminal(frame:StreamFrame):boolean;
export class RunView {state:RunSnapshot;constructor(runId:string);apply(frame:StreamFrame):boolean}
