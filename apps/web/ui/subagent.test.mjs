import test from 'node:test';
import assert from 'node:assert/strict';
import { validateChild, validateChildren, validateChildPage, validateSettings, roleFromFields } from './subagent-view.mjs';
const role = () => ({description:'Role',instructions:'Stay in scope',model:null,tools:null});
const config = () => ({max_parallel:4,max_children:64,max_depth:1,max_active_parents:128,roles:{default:role()}});
const settings = () => ({version:1,revision:3,enabled:true,locked:false,restart_required:false,config:config(),active:config()});
const child = () => ({id:'s-child',root:'s-root',parent:'s-root',role:'worker',depth:1,title:'Task',status:'completed',revision:5,turns:1,steps:2,run_id:'run-child',usage:{model_calls:2,reported_tokens:30,usage_complete:true},output:'Done',output_truncated:false,error_code:null,model:{provider_id:'local',model_id:'test-model'}});
test('subagent roles distinguish inherited tools from an explicitly empty ceiling',()=>{
 const input={description:'Worker',instructions:'Verify results',provider:'',model:'',tools:'',inheritTools:true};
 assert.equal(roleFromFields(input).tools,null);
 assert.deepEqual(roleFromFields({...input,inheritTools:false}).tools,[]);
 assert.deepEqual(roleFromFields({...input,inheritTools:false,tools:'read, read, grep'}).tools,['read','grep']);
 assert.throws(()=>roleFromFields({...input,provider:'local'}),/同时/);
 assert.throws(()=>roleFromFields({...input,instructions:'文'.repeat(3000)}),/容量/);
});
test('settings validate both active and saved capacities instead of guessing missing fields',()=>{
 assert.equal(validateSettings(settings()).active.max_parallel,4);
 for(const change of [v=>v.active.max_parallel=0,v=>v.config.max_depth=5,v=>delete v.config.roles.default,v=>v.config.roles.default.tools='read',v=>v.config.roles.default.model={provider_id:'local',model_id:''}]){
  const value=settings();change(value);assert.throws(()=>validateSettings(value));
 }
 const empty=settings();empty.config.roles.default.tools=[];assert.deepEqual(validateSettings(empty).config.roles.default.tools,[]);
});
test('child lists preserve actual status, role and model and reject cross-root or duplicate records',()=>{
 const a=child();assert.equal(validateChild(a,'s-root').status,'completed');
 assert.throws(()=>validateChild(a,'another-root'));
 const value={version:1,root:'s-root',parent_run:null,enabled:false,agents:[a]};
 assert.equal(validateChildren(value,'s-root').enabled,false);
 assert.throws(()=>validateChildren({...value,agents:[a,a]},'s-root'));
 for(const patch of [{status:'success'}, {depth:5},{usage:{model_calls:-1,reported_tokens:0,usage_complete:true}},{output:'x'.repeat(8193)}])assert.throws(()=>validateChild({...a,...patch},'s-root'));
});
test('child history cannot mix another run or unlisted turn and remains page bounded',()=>{
 const turn={id:'turn-one',run_id:'run-child',prompt:'Task',status:'completed'};
 const value={version:1,root:'s-root',agent:child(),page:{turns:[turn],older_turns:false,history:{turn,snapshot:{run_id:'run-child',items:[]},next_before:null}}};
 assert.equal(validateChildPage(value,'s-root','s-child').page.history.turn.id,'turn-one');
 for(const change of [v=>v.page.history.snapshot.run_id='another-run',v=>v.page.history.turn={...turn,id:'other-turn'},v=>v.page.history.snapshot.items=Array(31).fill({}),v=>v.page.history.next_before=-1]){
  const bad=structuredClone(value);change(bad);assert.throws(()=>validateChildPage(bad,'s-root','s-child'));
 }
});
