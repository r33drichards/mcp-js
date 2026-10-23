#!/usr/bin/env python3
"""Generate a locally scoped Compose project from pinned upstream sources."""
import argparse,json,os,shutil,subprocess,sys
from pathlib import Path
import yaml
import demo as d
ASTRO_VERSION='2.1.3'
ASTRO='91356a39d5a0dc601317b55339b7abfafd91b481'
BROWSER='63bab526d2a999af969657f70a342dfebe1cbb63'
def verify_source(root,commit):
 actual=subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],text=True).strip()
 if actual!=commit:raise RuntimeError('Source pin mismatch: '+str(root))
 tracked=subprocess.check_output(['git','-C',str(root),'ls-tree','-rz','--name-only',commit]).decode().split('\0')
 missing=[name for name in tracked if name and not os.path.lexists(root/name)]
 if missing:raise RuntimeError('Source checkout is not materialized: '+str(root)+'; missing '+missing[0])
 for args in [('diff','--quiet'),('diff','--cached','--quiet')]:
  if subprocess.run(['git','-C',str(root),*args]).returncode:raise RuntimeError('Source has tracked modifications; preserve and inspect: '+str(root))
def checkout(name,url,commit):
 p=d.STATE/name
 fresh=not p.exists()
 if fresh:subprocess.run(['git','clone','--no-checkout',url,str(p)],check=True)
 empty=not any(x.name!='.git' for x in p.iterdir())
 actual=subprocess.check_output(['git','-C',str(p),'rev-parse','HEAD'],text=True).strip()
 if actual!=commit:
  if not empty:
   for args in [('diff','--quiet'),('diff','--cached','--quiet')]:
    if subprocess.run(['git','-C',str(p),*args]).returncode:raise RuntimeError('Refusing to replace modified source '+str(p))
  subprocess.run(['git','-C',str(p),'fetch','--depth','1','origin',commit],check=True)
 # Matching HEAD is not proof that clone --no-checkout populated the worktree.
 if fresh or empty or actual!=commit:subprocess.run(['git','-C',str(p),'checkout','--detach',commit],check=True)
 verify_source(p,commit)
 return p

def render(path,env=None):
 return json.loads(subprocess.check_output(['docker','compose','-f',str(path),'--profile','browser','--profile','agent','config','--format','json'],env=env,text=True))
def prepare(binary,astro_source=None,browser_source=None,skip_build=False):
 c=d.config(True);ident=c['id'];out=d.STATE
 astro=Path(astro_source).resolve() if astro_source else checkout('astronomy','https://github.com/open-telemetry/opentelemetry-demo.git',ASTRO)
 browser=Path(browser_source).resolve() if browser_source else checkout('computer-use','https://github.com/r33drichards/computer-use.git',BROWSER)
 for root,pin in [(astro,ASTRO),(browser,BROWSER)]:
  verify_source(root,pin)
 env=dict(os.environ,DEMO_ID=ident,RUNTIME_IMAGE=ident+':local',BROWSER_CONTEXT=str(browser/'images/browser'),MCP_CONFIG_FILE=str(d.HERE/'mcp-full.json'))
 base=render(d.HERE/'compose.yml',env);upstream=render(astro/'docker-compose.yml',dict(os.environ,DEMO_VERSION=ASTRO_VERSION))
 collector=yaml.safe_load((astro/'src/otel-collector/otelcol-config.yml').read_text())
 for key in ['docker_stats','hostmetrics']:collector['receivers'].pop(key,None)
 collector['processors']['resourcedetection']['detectors']=['env']
 collector['service']['pipelines']['metrics']['receivers']=[x for x in collector['service']['pipelines']['metrics']['receivers'] if x not in ['docker_stats','hostmetrics']]
 collector['exporters']['otlphttp/loki']={'endpoint':'http://loki:3100/otlp'}
 collector['service']['pipelines']['logs']['exporters'].append('otlphttp/loki')
 (out/'collector.yml').write_text(yaml.safe_dump(collector))
 # Keep upstream application instrumentation, replace its duplicate Grafana only.
 services=base['services'];services.pop('app');services.pop('prometheus')
 for name,s in upstream['services'].items():
  if name=='grafana':continue
  build=s.pop('build',None)
  if not s.get('image'):
   if not build:raise RuntimeError('No image/build for '+name)
   s['image']=ident+'-'+name+':'+ASTRO[:8];s['build']=build
  s.pop('ports',None);s.pop('develop',None)
  s['container_name']=ident+'-'+name;s['networks']={'demo':None};s['labels']={'mcp-js-demo-owner':ident}
  s['restart']='no'
  if 'depends_on' in s:s['depends_on'].pop('grafana',None)
  for mount in s.get('volumes',[]):
   if mount.get('type')=='bind' and (mount.get('source')=='/' or 'docker.sock' in mount.get('source','')):continue
  s['volumes']=[m for m in s.get('volumes',[]) if m.get('target') not in ['/hostfs','/var/run/docker.sock']]
  if name=='otel-collector':
   for m in s['volumes']:
    if m['target']=='/etc/otelcol-config.yml':m['source']=str(out/'collector.yml')
  if name=='frontend-proxy':s['ports']=[{'target':8080,'published':'38880','host_ip':'127.0.0.1','protocol':'tcp'}]
  if name=='flagd':
   for m in s.get('volumes',[]):
    if m.get('target')=='/etc/flagd':
     target=out/'flagd';shutil.copytree(m['source'],target,dirs_exist_ok=True);m['source']=str(target)
  services[name]=s
 services['grafana']['depends_on']={'prometheus':{'condition':'service_started'}}
 # Existing Grafana receives stock metrics; Loki is an explicit added log export.
 ds=out/'datasources.yml';ds.write_text(yaml.safe_dump({'apiVersion':1,'datasources':[{'name':'Demo Prometheus','uid':'demo-prometheus','type':'prometheus','url':'http://prometheus:9090','access':'proxy','isDefault':True},{'name':'Demo logs','uid':'demo-logs','type':'loki','url':'http://loki:3100','access':'proxy'}]}))
 # Avoid duplicate UID in the original provisioning directory.
 prov=out/'provisioning';shutil.copytree(d.HERE/'provisioning',prov,dirs_exist_ok=True);(prov/'datasources/demo.yml').unlink(missing_ok=True)
 shutil.copy2(ds,prov/'datasources/astronomy.yml')
 for name in ['node1','node2','node3','query']:services[name]['user']='1000:1000'
 for m in services['grafana']['volumes']:
  if m['target']=='/etc/grafana/provisioning':m['source']=str(prov)
 services['grafana-mcp']['command']=[x.replace('prometheus,datasource','prometheus,datasource,loki') for x in services['grafana-mcp']['command']]
 services['loki']={'image':'grafana/loki:3.5.7','container_name':ident+'-loki','networks':{'demo':None},'labels':{'mcp-js-demo-owner':ident},'command':['-config.file=/etc/loki/local-config.yaml'],'volumes':[{'type':'volume','source':'loki','target':'/loki'}]}
 base['volumes']['loki']={'labels':{'mcp-js-demo-owner':ident}}
 for name,value in upstream.get('volumes',{}).items():base['volumes'][name]=dict(value or {},labels={'mcp-js-demo-owner':ident})
 # No global upstream container/network names and no upstream host-port exposure.
 for key,vol in base['volumes'].items():vol['name']=ident+'-'+key
 base['name']=ident;base['networks']={'demo':base['networks']['demo']}
 for service in services.values():service['platform']='linux/amd64'
 services['browser']['healthcheck']={'test':['CMD','/bin/bash','-c','echo > /dev/tcp/127.0.0.1/8081'],'interval':'3s','timeout':'3s','retries':40}
 services['query']['depends_on']['browser']={'condition':'service_healthy'}
 services['pi']['environment']['MCP_JS_DEMO_TOKEN']=c['token']
 d.save(out/'compose.json',base)
 (out/'compose.json').chmod(0o600)
 d.save(out/'sources.json',{'mcp_js':d.COMMIT,'astronomy':ASTRO,'browser':BROWSER,'adaptation':'No host metrics/socket; OTLP logs additionally exported to Loki; Grafana replaced with local Viewer; runtime images must be locked after pull.'})
 if not skip_build:subprocess.run([sys.executable,str(d.HERE/'build-image.py'),'--binary',str(Path(binary).resolve()),'--context',str(out/'image'),'--tag',ident+':local'],check=True)
 print(out/'compose.json')
if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('--binary',default=None);p.add_argument('--astronomy-source');p.add_argument('--browser-source');p.add_argument('--skip-build',action='store_true');a=p.parse_args()
 if not a.binary and not a.skip_build:
  a.binary=next((str(p) for p in [d.HERE.parents[1]/'target/release/server',d.HERE.parents[1]/'target/debug/server'] if p.is_file()),None)
  if not a.binary:p.error('No built server found. Run ./launch.sh prepare --skip-build then ./build-portable.sh, or supply --binary PATH.')
 prepare(a.binary,a.astronomy_source,a.browser_source,a.skip_build)
