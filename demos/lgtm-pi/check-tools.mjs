import {Client} from '@modelcontextprotocol/sdk/client/index.js';
import {StreamableHTTPClientTransport} from '@modelcontextprotocol/sdk/client/streamableHttp.js';
for(const port of [39301,39302]) {
 const c=new Client({name:'minimal-readiness',version:'1.0'});
 await c.connect(new StreamableHTTPClientTransport(new URL(`http://127.0.0.1:${port}/mcp`)));
 const r=await c.listTools();console.log(port,r.tools.map(t=>t.name));await c.close();
}
