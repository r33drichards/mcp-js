import json,uuid,pathlib,shutil
from client import execute
label='unified-check-'+uuid.uuid4().hex
code='''(async()=>{
const unpack=r=>{if(r.isError)throw Error(JSON.stringify(r));return r.structuredContent??JSON.parse(r.content.find(c=>c.type==='text').text)};
async function scratch(code,fs){
 const start=unpack(await mcp.callTool('scratch','run_js',{code,fs,execution_timeout_secs:10}));
 for(let i=0;i<100;i++){
  const r=unpack(await mcp.callTool('scratch','get_execution',{execution_id:start.execution_id}));
  if(r.status!=='running')return r;
 }
 throw Error('Scratch outcome uncertain; inspect execution '+start.execution_id);
}
const a=await scratch('(async()=>{await fs.writeFile("/work/note.txt","base");})()',LABEL);
if(a.status!=='completed')throw Error(JSON.stringify(a));
const b=await scratch('(async()=>{await fs.writeFile("/work/note.txt","child");})()',a.fs);
if(b.status!=='completed'||b.fs===a.fs)throw Error('Child root not isolated');
const check=await scratch('(async()=>{if(await fs.readFile("/work/note.txt","utf8")!=="base")throw Error("base changed");})()',a.fs);
if(check.status!=='completed')throw Error('COW base changed');
const denied=await scratch('(async()=>{await fs.writeFile("/dashboards/forbidden.json","{}");})()',b.fs);
if(denied.status!=='failed')throw Error('Scratch accessed shared dashboards');
let blocked=false;try{await fs.writeFile('/tmp/forbidden.json','no')}catch(e){blocked=true}if(!blocked)throw Error('Host policy bypass');
await fs.writeFile('/dashboards/unified-readiness.json',JSON.stringify({uid:'unified-readiness',title:'Temporary unified verification',schemaVersion:41,panels:[]}));
if(!await fs.exists('/dashboards/unified-readiness.json'))throw Error('Shared write missing');
const metrics=await mcp.callTool('grafana','query_prometheus',{datasourceUid:'prometheus',expr:'demo_checkout_latency_ms',startTime:'now-2h',endTime:'now',queryType:'range',stepSeconds:10});
if(metrics.isError||!JSON.stringify(metrics).includes('900'))throw Error('Seeded metric unavailable');
const browser=await mcp.callTool('browser','browser_execute',{tab:'unified-readiness',operations:[{type:'navigate',params:{url:'http://lgtm:3000'}},{type:'evaluate',params:{script:'document.title'}}]});
if(browser.isError)throw Error('Browser failed');
console.log(JSON.stringify({passed:true,base:a.fs,child:b.fs,sharedWrite:true,cowIsolation:true,scratchDashboardDenied:true,hostOutsideDenied:true,grafana:true,browser:true}));
})()'''.replace('LABEL',json.dumps(label))
r=execute(code);out=pathlib.Path('.evidence');out.mkdir(exist_ok=True);(out/'unified-verification.json').write_text(json.dumps(r,indent=2))
if r['status']!='completed':raise RuntimeError(r.get('error'))
assert pathlib.Path('dashboards/unified-readiness.json').is_file()
shutil.move('dashboards/unified-readiness.json',out/'unified-readiness-dashboard.json')
print(r['console']['data'])
