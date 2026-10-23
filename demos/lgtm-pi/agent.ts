import type { ExtensionAPI } from '@earendil-works/pi-coding-agent';
export default function(pi: ExtensionAPI) {
 pi.on('session_start',async (_event,ctx)=>{
  pi.registerMcpServer('demo',{url:'http://127.0.0.1:39301/mcp',exposure:'direct'});
  ctx.ui.setStatus('minimal','One demo.run_js: shared dashboards, explicit COW scratch, Grafana and browser');
 });
}
