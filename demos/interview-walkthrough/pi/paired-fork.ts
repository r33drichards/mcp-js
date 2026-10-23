/** Custom demo integration for Pi 1.4.0, not a built-in filesystem fork. */
import type { ExtensionAPI } from '@earendil-works/pi-coding-agent';
const base = process.env.MCP_JS_DEMO_URL || 'http://127.0.0.1:38080';
const token = process.env.MCP_JS_DEMO_TOKEN;
export default function (pi: ExtensionAPI) {
  if (!token) throw new Error('Set MCP_JS_DEMO_TOKEN from the disposable controller ownership.json');
  let pairedFork = false;
  async function post(action: string, args: object) {
    const r = await fetch(`${base}/api/pair/${action}`, {method:'POST',headers:{'Content-Type':'application/json','X-Demo-Token':token!},body:JSON.stringify(args)});
    const body = await r.json(); if (!r.ok) throw new Error(JSON.stringify(body)); return body;
  }
  pi.on('session_start', async (_event, ctx) => {
    const sid = ctx.sessionManager.getSessionId();
    // Registered on each replacement runtime. Unbound sessions fail closed.
    pi.registerMcpServer('interview', {url:`${base}/mcp/${sid}`,headers:{'X-Demo-Token':token!}});
    ctx.ui.setStatus('demo-branch', `paired filesystem: ${sid.slice(0,8)} (check branch_info)`);
  });
  pi.on('session_before_fork', async (_event, ctx) => {
    if (!pairedFork) { ctx.ui.notify('Use /paired-fork to reserve both conversation and filesystem branches.', 'warning'); return {cancel:true}; }
  });
  pi.registerCommand('demo-init', {description:'Bind this synthetic conversation to the prepared mcp-js base',handler:async (_args,ctx)=>{
    await ctx.waitForIdle(); const r=await post('init',{session:ctx.sessionManager.getSessionId()}); ctx.ui.notify(JSON.stringify(r),'info');
  }});
  pi.registerCommand('paired-fork', {description:'Fork Pi at the current leaf and pair a fresh isolated mcp-js filesystem branch',handler:async (_args,ctx)=>{
    await ctx.waitForIdle(); const leaf=ctx.sessionManager.getLeafId();
    if (!leaf) throw new Error('Add a synthetic conversation entry before forking.');
    const reservation=await post('prepare',{session:ctx.sessionManager.getSessionId()});
    pairedFork=true;
    try {
      const outcome=await ctx.fork(leaf,{position:'at',withSession:async fresh=>{
        const bound=await post('commit',{reservation:reservation.reservation,session:fresh.sessionManager.getSessionId()});
        fresh.ui.notify(`Conversation fork + filesystem branch: ${bound.label}`,'info');
      }});
      if (outcome.cancelled) await post('abort',{reservation:reservation.reservation});
    } catch (e) {
      // Prepared reservations abort; committing records remain tracked for reconciliation.
      await post('abort',{reservation:reservation.reservation}).catch(()=>{}); throw e;
    } finally { pairedFork=false; }
  }});
  pi.registerCommand('paired-merge',{description:'Explicitly merge another bound session filesystem into this one; conversations stay separate',handler:async (other,ctx)=>{
    await ctx.waitForIdle(); if(!other.trim())throw new Error('Usage: /paired-merge OTHER_SESSION_ID');
    const r=await post('merge',{target:ctx.sessionManager.getSessionId(),other:other.trim()}); ctx.ui.notify(JSON.stringify(r),'info');
  }});
}
