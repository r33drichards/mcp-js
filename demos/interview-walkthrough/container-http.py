"""Executed only inside an ownership-checked demo runtime container."""
import json,sys,urllib.request,urllib.error
spec=json.loads(sys.argv[1]);data=spec.get('data')
r=urllib.request.Request(spec['url'],None if data is None else json.dumps(data).encode(),headers={'Content-Type':'application/json','Host':spec.get('host','127.0.0.1')})
try:
 with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(r,timeout=spec['timeout']) as response:status,body=response.status,response.read().decode()
except urllib.error.HTTPError as e:status,body=e.code,e.read().decode()
print(json.dumps({'status':status,'body':body}))
