"""Actual Grafana MCP query and explicit COW -> Grafana file publication bridge."""
import base64,json,time,urllib.request,urllib.error,subprocess,hashlib,os,sys
from pathlib import Path
PROM='prom/prometheus@sha256:23031bfe0e74a13004252caaa74eccd0d62b6c6e7a04711d5b8bf5b7e113adc7'
GRAFANA='grafana/grafana@sha256:70d9599b186ce287be0d2c5ba9a78acb2e86c1a68c9c41449454d0fc3eeb84e8'
MCP='grafana/mcp-grafana@sha256:71a3f4e6f412d253611314233df709ec2571fb50e39742d23fa2940f670646dc'
class Observability:
 def __init__(self,d):self.d=d
 def url(self,service,port,path):
  info=self.d.owned('container',self.d.name(service));ip=next(iter(info['NetworkSettings']['Networks'].values()))['IPAddress'];return f'http://{ip}:{port}{path}'
 def http(self,service,port,path,data=None):
  if os.environ.get('MCP_JS_DEMO_TRANSPORT')=='exec' or sys.platform=='darwin':
   r=self.d.container_http(1,f'http://{service}:{port}{path}',data,15,host=f'{service}:{port}')
   if r['status']>=400:raise RuntimeError('Service HTTP error '+str(r))
   return r['body'].encode()
  req=urllib.request.Request(self.url(service,port,path),data=None if data is None else json.dumps(data).encode(),headers={'Content-Type':'application/json'})
  with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(req,timeout=15) as r:return r.read()
 def create(self,service,image,args=(),env=None,extra=()):
  d=self.d;c=d.config();e={'NO_PROXY':'*','no_proxy':'*','HTTP_PROXY':'','HTTPS_PROXY':'','http_proxy':'','https_proxy':''};e.update(env or {})
  flags=sum((['-e',k+'='+v] for k,v in e.items()),[])
  return d.docker('create','--network',c['id'],'--name',d.name(service),'--network-alias',str(service),'--label','mcp-js-demo-owner='+c['id'],'--cap-drop','ALL','--security-opt','no-new-privileges','--memory','768m',*flags,*extra,image,*args)
 def cp(self,source,service,target):
  Path(source).chmod(0o644)
  self.d.docker('cp',str(source),self.d.name(service)+':'+target)
 def up(self):
  d=self.d;here=d.HERE;image=d.config()['id']+':local'
  for suffix in ['dashboards','grafana','prometheus']:
   d.docker('volume','create','--label','mcp-js-demo-owner='+d.config()['id'],d.name(suffix+'-data'))
  self.create('app',image,['/app.py'],extra=['--entrypoint','/usr/bin/python3']);self.cp(here/'synthetic-app.py','app','/app.py');self.cp(here/'fixtures/ec2_cpu_utilization_825cc2.csv','app','/data.csv');d.docker('start',d.name('app'))
  self.create('prometheus',PROM,['--config.file=/etc/prometheus/demo.yml','--storage.tsdb.path=/prometheus','--storage.tsdb.retention.time=1h'],extra=['--mount','type=volume,src='+d.name('prometheus-data')+',dst=/prometheus']);self.cp(here/'prometheus.yml','prometheus','/etc/prometheus/demo.yml');d.docker('start',d.name('prometheus'))
  self.create('publisher',image,['-c','import time;time.sleep(86400)'],extra=['--entrypoint','/usr/bin/python3','--mount','type=volume,src='+d.name('dashboards-data')+',dst=/dashboards']);d.docker('start',d.name('publisher'))
  env={'GF_AUTH_ANONYMOUS_ENABLED':'true','GF_AUTH_ANONYMOUS_ORG_ROLE':'Viewer','GF_AUTH_DISABLE_LOGIN_FORM':'true','GF_SECURITY_DISABLE_INITIAL_ADMIN_CREATION':'true','GF_ANALYTICS_REPORTING_ENABLED':'false','GF_ANALYTICS_CHECK_FOR_UPDATES':'false','GF_DASHBOARDS_DEFAULT_HOME_DASHBOARD_PATH':'','GF_SERVER_ROOT_URL':'http://127.0.0.1:38300/'}
  self.create('grafana',GRAFANA,env=env,extra=['-p','127.0.0.1:38300:3000','--mount','type=volume,src='+d.name('grafana-data')+',dst=/var/lib/grafana','--mount','type=volume,src='+d.name('dashboards-data')+',dst=/var/lib/grafana/dashboards,readonly'])
  self.cp(here/'provisioning/datasources/demo.yml','grafana','/etc/grafana/provisioning/datasources/demo.yml');self.cp(here/'provisioning/dashboards/demo.yml','grafana','/etc/grafana/provisioning/dashboards/demo.yml');d.docker('start',d.name('grafana'))
  for _ in range(80):
   try:
    if json.loads(self.http('grafana',3000,'/api/health')).get('database')=='ok':break
   except Exception:pass
   time.sleep(.25)
  else:raise RuntimeError('Grafana readiness timeout')
  self.create('grafana-mcp',MCP,['--transport','streamable-http','--address','0.0.0.0:8000','--allowed-hosts','grafana-mcp:8000','--enabled-tools','prometheus,datasource','--disable-write','--usage-stats','disabled'],env={'GRAFANA_URL':'http://grafana:3000','DO_NOT_TRACK':'1'});d.docker('start',d.name('grafana-mcp'));time.sleep(.5)
  policies={'filesystem':{'policies':[{'url':'file:///filesystem.rego','rule':'data.mcp.filesystem.allow'}]},'mcp_tools':{'policies':[{'url':'file:///mcp-tools.rego','rule':'data.mcp.tools.allow'}]}}
  p=d.STATE/'analysis-policies.json';d.save(p,policies)
  env={'AWS_ENDPOINT_URL':'http://objects:9000','AWS_S3_FORCE_PATH_STYLE':'true','AWS_ACCESS_KEY_ID':'demo-only','AWS_SECRET_ACCESS_KEY':'demo-only','AWS_REGION':'us-east-1','AWS_EC2_METADATA_DISABLED':'true','AWS_REQUEST_CHECKSUM_CALCULATION':'WHEN_REQUIRED','AWS_RESPONSE_CHECKSUM_VALIDATION':'WHEN_REQUIRED'}
  self.create(4,image,['--http-port','3000','--bind-host','0.0.0.0','--heap-store','none','--fs-store','s3','--s3-bucket','demo','--cache-dir','/tmp/cache','--session-db-path','/tmp/sessions','--policies-json','/analysis-policies.json','--mcp-config','/mcp.json','--heap-memory-max','64'],env=env)
  for source,target in [(p,'/analysis-policies.json'),(here/'mcp-tools.rego','/mcp-tools.rego'),(here/'filesystem.rego','/filesystem.rego'),(here/'mcp-grafana.json','/mcp.json')]:self.cp(source,4,target)
  d.docker('start',d.name(4));time.sleep(1)
  return d.record('observability_up',{'grafana':'anonymous Viewer, no service account token','images':{'prometheus':PROM,'grafana':GRAFANA,'grafana_mcp':MCP},'browser':'not verified; separate source build'})
 def query_expr(self,expr,instant=False):
  args={'datasourceUid':'demo-prometheus','expr':expr,'startTime':'now-5m','endTime':'now','stepSeconds':1,'queryType':'instant' if instant else 'range'}
  code='(async()=>{const r=await mcp.callTool("grafana","query_prometheus",'+json.dumps(args)+');console.log(JSON.stringify(r));await fs.writeFile("/work/scratch/prometheus.json",JSON.stringify(r));})()'
  s=self.d.state_data() or self.d.seed();result=self.d.ok_run(code,s.get('investigation_root',s.get('M',s['B'])),4)
  tool=json.loads(result['console']['data'])
  if tool.get('isError'):raise RuntimeError('Grafana MCP tool failed: '+str(tool))
  parsed=json.loads(next(c['text'] for c in tool['content'] if c['type']=='text'))
  if 'data' not in parsed:raise RuntimeError('No Grafana MCP data')
  evidence={'args':args,'result':result,'parsed':parsed,'route':'mcp-js -> official Grafana MCP -> Grafana datasource -> Prometheus'}
  s['investigation_root']=result['fs']; self.d.save(self.d.STATE/'demo.json',s); self.d.save(self.d.STATE/'query.json',evidence);return self.d.record('grafana_mcp_query',evidence)
 def query(self):return self.query_expr('sum by (service_name,status_code) (rate(traces_span_metrics_calls_total{service_name="checkout"}[2m]))')
 def logs(self):
  d=self.d;s=d.state_data();root=s.get('investigation_root',s.get('B'))
  args={'datasourceUid':'demo-logs','logql':'{service_name=~"checkout|payment"}','startRfc3339':'now-15m','endRfc3339':'now','limit':50}
  code='(async()=>{const r=await mcp.callTool("grafana","query_loki_logs",'+json.dumps(args)+');console.log(JSON.stringify(r));await fs.writeFile("/work/scratch/logs.json",JSON.stringify(r));})()'
  r=d.ok_run(code,root,4);tool=json.loads(r['console']['data'])
  if tool.get('isError'):raise RuntimeError('Loki MCP query failed: '+str(tool))
  s['investigation_root']=r['fs'];d.save(d.STATE/'demo.json',s)
  return d.record('grafana_mcp_logs',{'args':args,'result':r,'warning':'Check returned lines; success with no lines is not evidence of fault log coverage.'})
 def dashboard(self):
  # Stage JSON in COW filesystem. Publication is a separate explicit operation.
  d=self.d;s=d.state_data();root=s.get('investigation_root',s.get('M',s.get('B')))
  if not root:raise RuntimeError('Seed/merge first')
  dashboard={'id':None,'uid':'mcp-anomaly','title':'Astronomy Shop checkout investigation','schemaVersion':41,'version':1,'tags':['synthetic','mcp-js-demo'],'timezone':'browser','time':{'from':'now-15m','to':'now'},'refresh':'5s','panels':[{'id':1,'type':'timeseries','title':'Checkout request rate by status','gridPos':{'h':10,'w':16,'x':0,'y':0},'datasource':{'type':'prometheus','uid':'demo-prometheus'},'targets':[{'refId':'A','expr':'sum by (service_name,status_code) (rate(traces_span_metrics_calls_total{service_name="checkout"}[2m]))','legendFormat':'{{status_code}}'}],'fieldConfig':{'defaults':{'unit':'reqps'},'overrides':[]}},{'id':2,'type':'timeseries','title':'Checkout span latency','gridPos':{'h':10,'w':8,'x':16,'y':0},'datasource':{'type':'prometheus','uid':'demo-prometheus'},'targets':[{'refId':'A','expr':'histogram_quantile(0.95, sum by (le) (rate(traces_span_metrics_duration_milliseconds_bucket{service_name="checkout"}[2m])))','legendFormat':'p95 ms'}]}]}
  blob=json.dumps(dashboard,indent=2);r=d.ok_run(d.write(blob,'/work/dashboards/anomaly.json'),root,4)
  s['dashboard_root']=r['fs'];d.save(d.STATE/'demo.json',s)
  return d.record('dashboard_staged',{'root':r['fs'],'virtual_path':'/work/dashboards/anomaly.json','published':False})
 def publish(self):
  d=self.d;s=d.state_data();root=s.get('dashboard_root')
  if not root:raise RuntimeError('Stage a dashboard first')
  r=d.ok_run('(async()=>{console.log(await fs.readFile("/work/dashboards/anomaly.json","utf8"));})()',root,4)
  page=r['console'];text=page.get('output',page.get('data',''))
  dashboard=json.loads(text)
  if dashboard.get('uid')!='mcp-anomaly' or not dashboard.get('panels'):raise RuntimeError('Unexpected dashboard identity/schema')
  # Reject cross-datasource/HTML/URL content in this fixed demo handoff.
  for panel in dashboard['panels']:
   if panel.get('type')!='timeseries' or panel.get('datasource',{}).get('uid')!='demo-prometheus':raise RuntimeError('Only local Prometheus timeseries panels permitted')
  raw=json.dumps(dashboard,indent=2).encode();encoded=base64.b64encode(raw).decode()
  d.owned('container',d.name('publisher'))
  script='import base64,os;from pathlib import Path;p=Path("/dashboards/anomaly.json");t=p.with_suffix(".tmp");f=t.open("wb");f.write(base64.b64decode('+repr(encoded)+'));f.flush();os.fsync(f.fileno());f.close();os.replace(t,p)'
  d.docker('exec',d.name('publisher'),'/usr/bin/python3','-c',script)
  for _ in range(40):
   try:
    result=json.loads(self.http('grafana',3000,'/api/dashboards/uid/mcp-anomaly'))
    if result.get('dashboard',{}).get('title')==dashboard['title']:break
   except Exception:pass
   time.sleep(.25)
  else:raise RuntimeError('Grafana did not provision dashboard within deadline')
  handoff={'root':root,'sha256':hashlib.sha256(raw).hexdigest(),'path':'/work/dashboards/anomaly.json','provisioned':True,'dashboard_url':'http://127.0.0.1:38300/d/mcp-anomaly','browser_render_verified':False,'note':'API provisioning verified; browser rendering is a separate required check.'}
  d.save(d.STATE/'handoff.json',handoff);return d.record('dashboard_published',handoff)
