#!/usr/bin/env python3
"""Two bounded OTLP requests; deterministic shape, fresh timestamps, synthetic only."""
import json,time,urllib.request,pathlib
end=int(time.time())-10;start=end-600
points=[];logs=[]
for i in range(61):
 t=start+i*10;spike=35<=i<=45;latency=900 if spike else 80+(i%5)*5
 points.append({'timeUnixNano':str(t*10**9),'asDouble':latency})
 logs.append({'timeUnixNano':str(t*10**9),'severityNumber':17 if spike else 9,'severityText':'ERROR' if spike else 'INFO','body':{'stringValue':'synthetic checkout payment timeout' if spike else 'synthetic checkout completed'},'attributes':[{'key':'demo.phase','value':{'stringValue':'anomaly' if spike else 'baseline'}}]})
resource={'attributes':[{'key':'service.name','value':{'stringValue':'checkout'}},{'key':'deployment.environment.name','value':{'stringValue':'synthetic-demo'}}]}
payloads={'metrics':{'resourceMetrics':[{'resource':resource,'scopeMetrics':[{'scope':{'name':'minimal-seed'},'metrics':[{'name':'demo_checkout_latency_ms','description':'Synthetic checkout latency with known bounded spike','gauge':{'dataPoints':points}}]}]}]},'logs':{'resourceLogs':[{'resource':resource,'scopeLogs':[{'scope':{'name':'minimal-seed'},'logRecords':logs}]}]}}
for kind,payload in payloads.items():
 req=urllib.request.Request('http://127.0.0.1:39418/v1/'+kind,json.dumps(payload).encode(),headers={'Content-Type':'application/json'})
 with urllib.request.urlopen(req,timeout=20) as r:print(kind,r.status,r.read().decode())
pathlib.Path('.evidence').mkdir(exist_ok=True)
pathlib.Path('.evidence/seed.json').write_text(json.dumps({'start':start,'end':end,'points':61,'anomaly_points':11,'synthetic':True},indent=2))
print('Seeded 61 correlated samples; anomaly indices 35..45. No dashboard created.')
