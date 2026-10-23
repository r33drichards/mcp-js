/** No model calls. Real Pi SessionManager forks and MCP StreamableHTTP transport. */
import fs from 'node:fs';import path from 'node:path';import assert from 'node:assert/strict';import {pathToFileURL} from 'node:url';
const root=process.env.PI_MODULE_ROOT||path.resolve(import.meta.dirname,'node_modules');
const {SessionManager}=await import(pathToFileURL(path.join(root,'@earendil-works/pi-coding-agent/dist/core/session-manager.js')));
const mcpRoot=process.env.MCP_MODULE_ROOT||root;
const {Client}=await import(pathToFileURL(path.join(mcpRoot,'@modelcontextprotocol/sdk/dist/esm/client/index.js')));
const {StreamableHTTPClientTransport}=await import(pathToFileURL(path.join(mcpRoot,'@modelcontextprotocol/sdk/dist/esm/client/streamableHttp.js')));
const state=process.env.MCP_JS_DEMO_STATE;if(!state)throw Error('Set disposable state path');
const config=JSON.parse(fs.readFileSync(path.join(state,'ownership.json'),'utf8'));const url='http://127.0.0.1:38080';
async function post(action,args){const r=await fetch(url+'/api/pair/'+action,{method:'POST',headers:{'Content-Type':'application/json','X-Demo-Token':config.token},body:JSON.stringify(args)});const b=await r.json();assert.equal(r.status,200,JSON.stringify(b));return b;}
const dir=path.join(state,'pi-test-sessions');fs.mkdirSync(dir,{recursive:true});const parent=SessionManager.create(state,dir);parent.appendMessage({role:'user',content:'Synthetic Astronomy Shop investigation; no model invocation.',timestamp:Date.now()});
// Pi persists complete session files after an assistant message; use an explicit test-only entry.
parent.appendMessage({role:'assistant',content:[{type:'text',text:'TEST FIXTURE: investigate checkout'}],api:'openai-responses',provider:'test',model:'no-model',usage:{input:0,output:0,cacheRead:0,cacheWrite:0,totalTokens:0,cost:{input:0,output:0,cacheRead:0,cacheWrite:0,total:0}},stopReason:'stop',timestamp:Date.now()});
await post('init',{session:parent.getSessionId()});const reservation=await post('prepare',{session:parent.getSessionId()});const child=SessionManager.forkFrom(parent.getSessionFile(),state,dir);assert.notEqual(child.getSessionId(),parent.getSessionId());await post('commit',{reservation:reservation.reservation,session:child.getSessionId()});
async function connect(sid){const c=new Client({name:'pairing-verifier',version:'1'});await c.connect(new StreamableHTTPClientTransport(new URL(url+'/mcp/'+sid),{requestInit:{headers:{'X-Demo-Token':config.token}}}));return c;}
const pc=await connect(parent.getSessionId()),cc=await connect(child.getSessionId());assert((await cc.listTools()).tools.some(t=>t.name==='run_js'));
const before=(await pc.callTool({name:'branch_info',arguments:{}})).structuredContent;
const write=await cc.callTool({name:'run_js',arguments:{code:'(async()=>{await fs.writeFile("/work/scratch/child.txt","independent finding");})()'}});assert.equal(write.isError,false,JSON.stringify(write));
assert.equal((await pc.callTool({name:'branch_info',arguments:{}})).structuredContent.root,before.root);
assert.notEqual((await cc.callTool({name:'branch_info',arguments:{}})).structuredContent.root,before.root);
assert.equal((await cc.callTool({name:'run_js',arguments:{code:'console.log("no")',fs:'parent'}})).isError,true);
const cancel=await post('prepare',{session:parent.getSessionId()});assert.equal((await post('abort',{reservation:cancel.reservation})).status,'aborted');
const merged=await post('merge',{target:parent.getSessionId(),other:child.getSessionId()});assert(merged.fs);assert.equal(merged.conversation_histories,'unchanged');
await pc.close();await cc.close();console.log(JSON.stringify({passed:true,checks:['Pi 1.4 SessionManager fork','native MCP handshake/list/call','child write isolates parent','root override rejected','cancel journal','explicit FS merge preserves conversations'],limitation:'No model or interactive extension event dispatch was exercised.'},null,2));
