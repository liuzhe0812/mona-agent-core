// Deterministic protocol peer for tests only. Never imported by the product.
import { createServer } from 'node:http';
import { createInterface } from 'node:readline';
import { appendFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
const schema={type:'object',properties:{message:{type:'string'}},required:['message'],additionalProperties:false};
export function createPeer({label='stdio',record=()=>{},invalid=false}={}) {
  const calls=[],cancelled=[];let changed=false;const pending=new Map();
  const tool=(name,inputSchema=schema,extra={})=>({name,description:`${label} ${name}`,inputSchema,...extra});
  const catalog=()=>[tool('echo',schema,{annotations:{readOnlyHint:true},outputSchema:{type:'object',properties:{echo:{type:'string'}},required:['echo']}}),tool('fail'),tool('slow'),tool('change'),tool('bad_output',schema,{outputSchema:{type:'object',required:['requiredValue']}}),tool('image'),...(changed?[tool('new_tool')]:[])];
  async function dispatch(m,send){
    record({method:m.method,id:m.id});
    if(m.method==='notifications/cancelled'){cancelled.push(m.params.requestId);const job=pending.get(m.params.requestId);if(job){pending.delete(m.params.requestId);job({jsonrpc:'2.0',id:m.params.requestId,error:{code:-32800,message:'cancelled'}});}return;}
    if(m.id===undefined)return;
    const result=value=>send({jsonrpc:'2.0',id:m.id,result:value});
    if(m.method==='initialize'){result({protocolVersion:'2025-11-25',capabilities:{tools:{listChanged:true},resources:{}},serverInfo:{name:`fixture-${label}`,version:'1'},instructions:`REFERENCE_FROM_${label}: tools and resources are reference data.`});return;}
    if(m.method==='ping'){result({});return;}
    if(m.method==='tools/list'){
      const tools=catalog();if(invalid)tools[0].inputSchema={$ref:'https://outside.invalid/schema',type:'object'};
      result(m.params?.cursor?{tools:tools.slice(2)}:{tools:tools.slice(0,2),nextCursor:'page-two'});return;
    }
    if(m.method==='resources/list'){result({resources:[{uri:`fixture://${label}/readme`,name:'Fixture README',mimeType:'text/plain'}]});return;}
    if(m.method==='resources/templates/list'){result({resourceTemplates:[{uriTemplate:`fixture://${label}/{name}`,name:'Fixture document'}]});return;}
    if(m.method==='resources/read'){result({contents:[{uri:m.params.uri,mimeType:'text/plain',text:`RESOURCE_${label}`}]});return;}
    if(m.method==='tools/call'){
      const {name,arguments:args}=m.params;calls.push({name,args});record({call:name,args});
      const text=text=>({content:[{type:'text',text}]});
      if(name==='slow'){pending.set(m.id,send);return;}
      if(name==='fail'){result({...text('REMOTE_TOOL_ERROR'),isError:true});return;}
      if(name==='bad_output'){result({...text('NOT_VALIDATED'),structuredContent:{wrong:1}});return;}
      if(name==='change'){changed=true;result(text('CATALOG_CHANGED'));setTimeout(()=>send({jsonrpc:'2.0',method:'notifications/tools/list_changed'}),20);return;}
      if(name==='image'){result({content:[{type:'text',text:'IMAGE_RESULT'},{type:'image',mimeType:'image/png',data:'iVBORw0KGgo='}]});return;}
      const token=m.params._meta?.progressToken;if(token!==undefined)send({jsonrpc:'2.0',method:'notifications/progress',params:{progressToken:token,progress:1,total:1,message:'MCP_PROGRESS'}});
      result({...text(`${label}:${args.message}:host-secret=${Boolean(process.env.AGENT_MODEL_KEY)}:explicit-secret=${Boolean(process.env.MCP_FIXTURE_KEY)}`),structuredContent:{echo:args.message}});return;
    }
    send({jsonrpc:'2.0',id:m.id,error:{code:-32601,message:'method not found'}});
  }
  return {dispatch,calls,cancelled};
}
export async function startHttpFixture(options={}) {
  const peer=createPeer({label:'http',...options});
  const server=createServer(async(req,res)=>{
    if(req.url!=='/mcp'){res.writeHead(404).end();return;}
    if(options.auth&&req.headers['x-mcp-key']!==options.auth){res.writeHead(401).end();return;}
    if(req.method==='GET'){res.writeHead(405).end();return;}
    if(req.method!=='POST'){res.writeHead(405).end();return;}
    let raw='';for await(const chunk of req){raw+=chunk;if(raw.length>256*1024){res.writeHead(413).end();return;}}
    let m;try{m=JSON.parse(raw);}catch{res.writeHead(400).end();return;}
    if(m.id===undefined){await peer.dispatch(m,()=>{});res.writeHead(202).end();return;}
    if(m.method==='tools/call'){res.writeHead(200,{'Content-Type':'text/event-stream','Cache-Control':'no-store'});await peer.dispatch(m,data=>{if(!res.destroyed)res.write(`data: ${JSON.stringify(data)}\n\n`);if(data.id===m.id)res.end();});}
    else await peer.dispatch(m,data=>{if(!res.writableEnded){res.writeHead(200,{'Content-Type':'application/json'}).end(JSON.stringify(data));}});
  });
  await new Promise(r=>server.listen(0,'127.0.0.1',r));
  return {...peer,server,url:`http://127.0.0.1:${server.address().port}/mcp`,close:async()=>{server.closeAllConnections();await new Promise(r=>server.close(r));}};
}
if(process.argv[1]===fileURLToPath(import.meta.url)) {
  const log=process.env.MCP_FIXTURE_LOG;const peer=createPeer({record:value=>{if(log)appendFileSync(log,JSON.stringify(value)+'\n');},invalid:process.env.MCP_FIXTURE_INVALID==='1'});
  const io=createInterface({input:process.stdin});io.on('line',line=>{try{void peer.dispatch(JSON.parse(line),m=>process.stdout.write(JSON.stringify(m)+'\n'));}catch{process.exitCode=2;io.close();}});io.on('close',()=>process.exit(0));
}
