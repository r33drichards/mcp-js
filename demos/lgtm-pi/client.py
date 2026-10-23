import json,time,urllib.request
BASE='http://127.0.0.1:39301'
def request(path,data=None,base=BASE):
 req=urllib.request.Request(base+path,None if data is None else json.dumps(data).encode(),headers={'Content-Type':'application/json'})
 with urllib.request.urlopen(req,timeout=40) as r:return json.load(r)
def execute(code,base=BASE,**kwargs):
 r=request('/api/exec',{'code':code,'execution_timeout_secs':30,**kwargs},base);eid=r['execution_id']
 for _ in range(160):
  r=request('/api/executions/'+eid,base=base)
  if r['status']!='running':
   r['console']=request('/api/executions/'+eid+'/output',base=base)
   return r
  time.sleep(.25)
 raise RuntimeError('Execution outcome uncertain: '+eid)
