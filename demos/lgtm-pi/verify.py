import json,time,pathlib,urllib.request,base64
from client import execute,request
out=pathlib.Path('.evidence');out.mkdir(exist_ok=True)
def run(label,code,**kwargs):
 r=execute(code,**kwargs);(out/(label+'.json')).write_text(json.dumps(r))
 if r['status']!='completed':raise RuntimeError(label+': '+str(r.get('error')))
 return r
def tool(label,name,args):
 r=run(label,'(async()=>{console.log(JSON.stringify(await mcp.callTool("grafana",'+json.dumps(name)+','+json.dumps(args)+')));})()')
 t=json.loads(r['console']['data']);assert not t.get('isError'),t
 print(label,'success');return t
seed=json.loads((out/'seed.json').read_text())
metrics=tool('metrics','query_prometheus',{'datasourceUid':'prometheus','expr':'demo_checkout_latency_ms','startTime':'now-30m','endTime':'now','stepSeconds':10,'queryType':'range'})
assert '900' in json.dumps(metrics),'No seeded anomaly in metric response'
logs=tool('logs','query_loki_logs',{'datasourceUid':'loki','logql':'{service_name="checkout"}','startRfc3339':'now-30m','endRfc3339':'now','limit':100})
assert 'payment timeout' in json.dumps(logs),'No correlated anomaly logs'
dashboard={'uid':'minimal-checkout','title':'Synthetic checkout latency','schemaVersion':41,'version':1,'time':{'from':'now-30m','to':'now'},'panels':[{'id':1,'title':'Checkout latency — seeded anomaly','type':'timeseries','datasource':{'type':'prometheus','uid':'prometheus'},'gridPos':{'x':0,'y':0,'w':24,'h':12},'targets':[{'refId':'A','expr':'demo_checkout_latency_ms'}],'fieldConfig':{'defaults':{'unit':'ms'},'overrides':[]}}]}
run('direct-write','(async()=>{await fs.writeFile("/dashboards/checkout.json",'+json.dumps(json.dumps(dashboard))+');console.log(await fs.readFile("/dashboards/checkout.json","utf8"));})()')
assert json.loads(pathlib.Path('dashboards/checkout.json').read_text())==dashboard
for i in range(30):
 try:
  loaded=request('/api/dashboards/uid/minimal-checkout',base='http://127.0.0.1:39300')
  if loaded['dashboard']['panels']==dashboard['panels']:break
 except Exception:pass
 time.sleep(1)
else:raise RuntimeError('Grafana did not load shared file')
print('Direct mcp-js fs.writeFile -> shared file -> Grafana provisioning verified')
r=execute('(async()=>{await fs.writeFile("/tmp/forbidden.json","no");})()');assert r['status']=='failed';(out/'denied-write.json').write_text(json.dumps(r))
args={'tab':'minimal-checkout','operations':[{'type':'navigate','params':{'url':'http://lgtm:3000/d/minimal-checkout'}},{'type':'wait','params':{'ms':3500}},{'type':'evaluate','params':{'script':'JSON.stringify({title:document.title,text:document.body.innerText,canvasCount:document.querySelectorAll("canvas").length})'}},{'type':'screenshot','params':{'fullPage':False}}]}
r=run('browser','(async()=>{console.log(JSON.stringify(await mcp.callTool("browser","browser_execute",'+json.dumps(args)+')));})()');t=json.loads(r['console']['data']);assert not t.get('isError'),str(t)[:500]
for c in t.get('content',[]):
 if c.get('type')=='image':(out/'dashboard.png').write_bytes(base64.b64decode(c['data']))
print('Browser result and screenshot saved; visual review required')
