#!/usr/bin/env python3
"""Generate a locally scoped Compose project from pinned upstream sources."""
import argparse,json,os,shutil,subprocess,sys
from pathlib import Path
import yaml
import demo as d
ASTRO='91356a39d5a0dc601317b55339b7abfafd91b481'
BROWSER='63bab526d2a999af969657f70a342dfebe1cbb63'
def checkout(name,url,commit):
 p=d.STATE/name
 if not p.exists():subprocess.run(['git','clone','--no-checkout',url,str(p)],check=True)
 actual=subprocess.check_output(['git','-C',str(p),'rev-parse','HEAD'],text=True).strip()
 if actual!=commit:
  subprocess.run(['git','-C',str(p),'fetch','--depth','1','origin',commit],check=True)
  subprocess.run(['git','-C',str(p),'checkout','--detach',commit],check=True)
 if subprocess.check_output(['git','-C',str(p),'rev-parse','HEAD'],text=True).strip()!=commit:raise RuntimeError('Source pin mismatch')
 return p

def render(path,env=None):
 return json.loads(subprocess.check_output(['docker','compose','-f',str(path),'--profile','browser','--profile','agent','config','--format','json'],env=env,text=True))
def prepare(binary,astro_source=None,browser_source=None,skip_build=False):
 c=d.config(True);ident=c['id'];out=d.STATE
 astro=Path(astro_source).resolve() if astro_source else checkout('astronomy','https://github.com/open-telemetry/opentelemetry-demo.git',ASTRO)
 browser=Path(browser_source).resolve() if browser_source else checkout('computer-use','https://github.com/r33drichards/computer-use.git',BROWSER)
 for root,pin in [(astro,ASTRO),(browser,BROWSER)]:
  if subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],text=True).strip()!=pin:raise RuntimeError('Wrong source commit '+str(root))
 env=dict(os.environ,DEMO_ID=ident,RUNTIME_IMAGE=ident+':local',BROWSER_CONTEXT=str(browser/'images/browser'),MCP_CONFIG_FILE=str(d.HERE/'mcp-full.json'))
 base=render(d.HERE/'compose.yml',env);upstream=render(astro/'docker-compose.yml')
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
 services['grafana']['volumes'].append({'type':'bind','source':str(ds),'target':'/etc/grafana/provisioning/datasources/astronomy.yml','read_only':True})
 # Avoid duplicate UID in the original provisioning directory.
 prov=out/'provisioning';shutil.copytree(d.HERE/'provisioning',prov,dirs_exist_ok=True);(prov/'datasources/demo.yml').unlink(missing_ok=True)
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
 p=argparse.ArgumentParser();p.add_argument('--binary',required=True);p.add_argument('--astronomy-source');p.add_argument('--browser-source');p.add_argument('--skip-build',action='store_true');a=p.parse_args();prepare(a.binary,a.astronomy_source,a.browser_source,a.skip_build)
