#!/usr/bin/env python3
"""Explicit staged profile; no runtime start, no measured-fit claim."""
import argparse,copy,json,shutil,sys,time
from pathlib import Path
import yaml
import demo as d
SHOP={'ad','cart','checkout','currency','email','flagd','frontend','image-provider','payment','product-catalog','quote','recommendation','shipping','valkey-cart','otel-collector','prometheus'}
SUPPORT={'browser','grafana','grafana-mcp','node1','objects','pi','publisher','query-network','loki'}
NEW_CAPS={'grafana':512,'grafana-mcp':256,'objects':128,'pi':512,'publisher':64,'query-network':32,'loki':512}
def limit(s):return int(s.get('mem_limit',s.get('deploy',{}).get('resources',{}).get('limits',{}).get('memory',0)))
def prepare():
 owner=d.config();full=json.loads((d.STATE/'compose.json').read_text());small=copy.deepcopy(full);small['services']={n:s for n,s in small['services'].items() if n in SHOP|SUPPORT};services=small['services']
 for n,s in services.items():
  if n in NEW_CAPS:s['mem_limit']=NEW_CAPS[n]*1024**2
  if 'depends_on' in s:s['depends_on']={k:v for k,v in s['depends_on'].items() if k in services}
 # Official minimal deployment omits Kafka; checkout source guards empty KAFKA_ADDR.
 services['checkout']['environment'].pop('KAFKA_ADDR',None)
 # One engine owns both metadata and execution in this phase. Fresh solo volume,
 # never reuse a three-voter Raft database with a different membership.
 solo=services['node1'];query=full['services']['query'];solo.pop('networks',None);solo['network_mode']='service:query-network'
 args=solo['command'];args[args.index('--peers')+1]=''
 args.extend(['--mcp-config','/mcp.json']);args[args.index('--policies-json')+1]='/analysis-policies.json'
 solo['volumes']=[{'type':'volume','source':'solo','target':'/data'}]+[v for v in query['volumes'] if v['type']=='bind']
 solo['depends_on']=copy.deepcopy(query['depends_on']);services['query-network']['networks']['demo']={'aliases':['node1']}
 small['volumes']['solo']={'name':owner['id']+'-solo','labels':{'mcp-js-demo-owner':owner['id']}}
 # Direct frontend port avoids keeping the proxy's optional UI/backend graph.
 services['frontend']['ports']=[{'target':8080,'published':'38880','host_ip':'127.0.0.1','protocol':'tcp'}]
 # Actual OTLP app logs -> Loki; actual spans -> spanmetrics -> Prometheus.
 # No trace archive/OpenSearch/host receiver in the small phase.
 collector=yaml.safe_load((d.STATE/'collector.yml').read_text())
 collector['receivers']={k:v for k,v in collector['receivers'].items() if k=='otlp'}
 collector['exporters']={k:v for k,v in collector['exporters'].items() if k in ['otlphttp/prometheus','otlphttp/loki']}
 pipes=collector['service']['pipelines'];pipes['traces']['receivers']=['otlp'];pipes['traces']['exporters']=['spanmetrics'];pipes['metrics']['receivers']=['otlp','spanmetrics'];pipes['metrics']['exporters']=['otlphttp/prometheus'];pipes['logs']['exporters']=['otlphttp/loki']
 path=d.STATE/'collector.investigation.yml';path.write_text(yaml.safe_dump(collector))
 for v in services['otel-collector']['volumes']:
  if v['target']=='/etc/otelcol-config.yml':v['source']=str(path)
 services['otel-collector']['depends_on']={'loki':{'condition':'service_started'},'prometheus':{'condition':'service_started'}}
 services['traffic']={'image':owner['id']+':local','platform':'linux/amd64','container_name':owner['id']+'-traffic','entrypoint':['/usr/bin/python3','/traffic.py'],'networks':{'demo':None},'labels':{'mcp-js-demo-owner':owner['id']},'mem_limit':64*1024**2,'volumes':[{'type':'bind','source':str(d.HERE/'memory-profile-traffic.py'),'target':'/traffic.py','read_only':True}],'depends_on':{'frontend':{'condition':'service_started'}},'restart':'no'}
 # Phase two is only the already-tested original three-voter topology + blobs.
 cluster=copy.deepcopy(full);cluster['services']={n:s for n,s in cluster['services'].items() if n in ['node1','node2','node3','objects']};cluster['services']['objects']['mem_limit']=128*1024**2
 for name,c in [('investigation',small),('cluster',cluster)]:
  d.save(d.STATE/('compose.'+name+'.json'),c);(d.STATE/('compose.'+name+'.json')).chmod(0o600)
  d.docker('compose','-f',str(d.STATE/('compose.'+name+'.json')),'--profile','browser','--profile','agent','config','--quiet')
 caps=sum(limit(s) for n,s in services.items() if n!='pi')
 report={'scope':'STATIC CONFIGURATION VALIDATED; runtime memory not measured','investigation_default_services':len(services)-1,'default_cap_MiB':caps/1024**2,'optional_pi_cap_MiB':512,'stock_service_caps':'unchanged','new_caps':'bounded safety ceilings for previously uncapped support processes, not measured requirements','cluster_cap_MiB':sum(limit(s) for s in cluster['services'].values())/1024**2,'removed_services':sorted(set(full['services'])-set(services)),'source':'Astronomy 2.1.3 docker-compose.minimal.yml and src/checkout/main.go optional KAFKA_ADDR'}
 d.save(d.STATE/'memory-profile-report.json',report);print(json.dumps(report,indent=2))
def activate(phase):
 owner=d.config();running=d.docker('ps','-q','--filter','label=mcp-js-demo-owner='+owner['id'])
 if running:raise RuntimeError('Stop the current owned phase first with launch.sh down; volumes must be preserved')
 current=owner.get('demo_phase','full');files=['demo.json','pairs.json','alert.json','query.json','handoff.json']
 archive=d.STATE/'phase-state'/current;archive.mkdir(parents=True,exist_ok=True)
 for name in files:
  f=d.STATE/name
  if f.exists():shutil.copy2(f,archive/name)
 target=d.STATE/'phase-state'/phase
 for name in files:
  f=d.STATE/name
  if (target/name).exists():shutil.copy2(target/name,f)
  elif f.exists():f.unlink() # safely archived above; never remove blob/label volumes
 owner.update(demo_phase=phase,node_ids=[1] if phase=='investigation' else [1,2,3],query_node=1 if phase=='investigation' else 4)
 d.save(d.STATE/'ownership.json',owner);print('Selected '+phase+'; no containers started. Use compose.'+phase+'.json explicitly.')
def pull(phase):
 path=d.STATE/('compose.'+phase+'.json');c=json.loads(path.read_text())
 for name,service in c['services'].items():
  image=service.get('image')
  if not image or 'build' in service or image==d.config()['id']+':local':continue
  if '@sha256:' not in image:
   d.docker('pull',image);info=json.loads(d.docker('image','inspect',image))[0]
   digests=info.get('RepoDigests') or []
   if not digests:raise RuntimeError('No immutable image digest for '+image)
   service['image']=digests[0]
 d.save(path,c);path.chmod(0o600)
def check():
 report=json.loads((d.STATE/'memory-profile-report.json').read_text())
 total=int(d.docker('info','--format','{{.MemTotal}}'))
 required=int((report['default_cap_MiB']+512)*1024**2)
 print(json.dumps({'docker_memory_MiB':total/1024**2,'default_caps_MiB':report['default_cap_MiB'],'reserved_daemon_headroom_MiB':512,'required_MiB':required/1024**2,'fits_static_ceiling':total>=required,'warning':'Not a runtime performance or OOM validation; Pi should run on host, not agent profile on this memory budget.'},indent=2))
 if total<required:raise SystemExit('Docker memory below static ceiling plus headroom; do not launch')
def measure():
 owner=d.config();ids=d.docker('ps','-q','--filter','label=mcp-js-demo-owner='+owner['id']).split()
 if not ids:raise RuntimeError('No running owned phase to measure')
 rows=[]
 for cid in ids:
  info=d.owned('container',cid);rows.append({'container':info['Name'],'oom_killed':info['State']['OOMKilled'],'restart_count':info['RestartCount']})
 stats=d.docker('stats','--no-stream','--format','{{json .}}',*ids)
 report={'at':time.time(),'phase':owner.get('demo_phase'),'containers':rows,'stats':[json.loads(line) for line in stats.splitlines()]};d.save(d.STATE/'memory-measurement.json',report);print(json.dumps(report,indent=2))
if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('action',choices=['prepare','activate','measure','pull','check']);p.add_argument('phase',nargs='?',choices=['investigation','cluster']);a=p.parse_args()
 if a.action=='prepare':prepare()
 elif a.action=='check':check()
 elif a.action=='pull':
  if not a.phase:raise SystemExit('Specify investigation or cluster')
  pull(a.phase)
 elif a.action=='activate':
  if not a.phase:raise SystemExit('Specify investigation or cluster')
  activate(a.phase)
 else:measure()
