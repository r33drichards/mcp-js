import assert from 'node:assert/strict';
import {loadExtensions,createExtensionRuntime} from './node_modules/@earendil-works/pi-coding-agent/dist/core/extensions/loader.js';
import {Client} from '@modelcontextprotocol/sdk/client/index.js';
import {StreamableHTTPClientTransport} from '@modelcontextprotocol/sdk/client/streamableHttp.js';
const runtime=createExtensionRuntime();
const result=await loadExtensions([new URL('./agent.ts',import.meta.url).pathname],process.cwd(),undefined,runtime);
assert.equal(result.errors.length,0,JSON.stringify(result.errors));assert.equal(result.extensions.length,1);
for(const handler of result.extensions[0].handlers.get('session_start'))await handler({type:'session_start'},{sessionManager:{getSessionId:()=> 'minimal-extension-readiness'},ui:{setStatus:()=>{}}});
const servers=runtime.mcpServers.list();assert.deepEqual(servers.map(s=>s.name).sort(),['demo']);
for(const s of servers){const c=new Client({name:'pi-extension-check',version:'1'});await c.connect(new StreamableHTTPClientTransport(new URL(s.config.url),{requestInit:{headers:s.config.headers||{}}}));assert((await c.listTools()).tools.some(t=>t.name==='run_js'));await c.close();}
console.log('Actual Pi TypeScript extension loaded; session_start registered the demo namespace; its MCP connection discovered run_js; no model calls.');
