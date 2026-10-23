import type { ExtensionAPI } from '@earendil-works/pi-coding-agent';
export default function(pi: ExtensionAPI) {
 pi.on('session_start',async (_event,ctx)=>{
  pi.registerMcpServer('dashboards',{url:'http://127.0.0.1:39301/mcp',exposure:'direct'});
  pi.registerMcpServer('scratch',{url:'http://127.0.0.1:39302/mcp',exposure:'direct',headers:{'X-MCP-Session-Id':ctx.sessionManager.getSessionId()}});
  ctx.ui.setStatus('minimal','Shared dashboards + separate COW scratch; Grafana/browser via dashboards.run_js');
 });
}
