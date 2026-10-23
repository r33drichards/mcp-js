#!/usr/bin/env python3
"""Disposable local controller. Only owns labeled resources it created.
No model calls. All writes use synthetic /work/allowed data.
"""
import argparse, concurrent.futures, hashlib, json, os, secrets, socket, subprocess, sys, threading, time, urllib.error, urllib.request, uuid
from pathlib import Path
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
HERE=Path(__file__).resolve().parent
STATE=Path(os.environ.get('MCP_JS_DEMO_STATE',str(HERE/'.state'))).resolve()
COMMIT='fe5b613db0afcc9cf20d7ca1ef659c914a0b5008'
LOCK=threading.RLock()
def save(path,data):
    path.parent.mkdir(parents=True,exist_ok=True)
    tmp=path.with_suffix('.tmp'); tmp.write_text(json.dumps(data,indent=2)+'\n'); tmp.replace(path)
def config(create=False):
    p=STATE/'ownership.json'
    if p.exists(): return json.loads(p.read_text())
    if not create: raise RuntimeError('Demo not initialized; run up first')
    if STATE.exists() and any(STATE.iterdir()): raise RuntimeError('Refusing nonempty unowned state directory')
    c={'id':'mcp-interview-'+uuid.uuid4().hex[:8],'base_commit':COMMIT,'token':secrets.token_hex(24),'created':time.time()}
    save(p,c); return c
def docker(*args,check=True):
    r=subprocess.run(['docker',*args],capture_output=True,text=True)
    if check and r.returncode: raise RuntimeError(r.stderr.strip() or r.stdout.strip())
    return r.stdout.strip()
def name(n): return config()['id']+'-'+str(n)
def owned(kind,target):
    data=json.loads(docker(kind,'inspect',target))[0]
    labels=data.get('Config',{}).get('Labels',{}) if kind=='container' else data.get('Labels',{})
    if (labels or {}).get('mcp-js-demo-owner')!=config()['id']: raise RuntimeError('Refusing unowned resource '+target)
    return data
def container_http(node,url,data=None,timeout=12,host='127.0.0.1'):
    owned('container',name(node))
    spec=json.dumps({'url':url,'data':data,'timeout':timeout,'host':host})
    result=docker('exec',name(node),'/usr/bin/python3','-c',(HERE/'container-http.py').read_text(),spec)
    return json.loads(result)
def request(node,path,data=None,raft=False,timeout=12):
    if os.environ.get('MCP_JS_DEMO_TRANSPORT')=='exec' or sys.platform=='darwin':
        r=container_http(node,f'http://127.0.0.1:{4000 if raft else 3000}{path}',data,timeout)
        return r['status'],json.loads(r['body'])
    port=(38200 if raft else 38100)+node
    info=owned('container',name(node))
    if not info['State']['Running']: raise RuntimeError('demo node stopped')
    # Internal networks may intentionally omit port publication; use their private IP.
    if not info['NetworkSettings']['Networks'] and info['HostConfig']['NetworkMode'].startswith('container:'):
        info=owned('container',info['HostConfig']['NetworkMode'].split(':',1)[1])
    ip=next(iter(info['NetworkSettings']['Networks'].values()))['IPAddress']
    url=f'http://{ip}:{4000 if raft else 3000}{path}'
    req=urllib.request.Request(url,None if data is None else json.dumps(data).encode(),{'Content-Type':'application/json','Host':'127.0.0.1'})
    try:
        with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(req,timeout=timeout) as r: return r.status,json.load(r)
    except urllib.error.HTTPError as e:
        try: body=json.load(e)
        except Exception: body={'error':str(e)}
        return e.code,body

def record(event,data):
    STATE.mkdir(parents=True,exist_ok=True)
    with (STATE/'events.jsonl').open('a') as f: f.write(json.dumps({'at':time.time(),'event':event,'data':data})+'\n')
    return data
def statuses():
    result=[]
    for i in (1,2,3):
        try:
            _,s=request(i,'/raft/status',raft=True,timeout=.8); s['node']=i
        except Exception as e: s={'node':i,'role':'unreachable','error':str(e)}
        result.append(s)
    return result
def leader(timeout=15,exclude=()):
    end=time.monotonic()+timeout
    while time.monotonic()<end:
        for s in statuses():
            if s['node'] not in exclude and s.get('role','').lower()=='leader': return s['node']
        time.sleep(.15)
    raise RuntimeError('No leader observed within deadline')
def exec_js(code,fs,node=None,timeout=5):
    node=node or leader(); start=time.monotonic()
    status,r=request(node,'/api/exec',{'code':code,'fs':fs,'execution_timeout_secs':timeout})
    if status!=202: raise RuntimeError(f'Exec submission: {status} {r}')
    eid=r['execution_id']
    for _ in range(int((timeout+8)*20)):
        _,r=request(node,'/api/executions/'+eid)
        if r['status']!='running':
            _,o=request(node,'/api/executions/'+eid+'/output')
            r['console']=o; r['elapsed_ms']=round((time.monotonic()-start)*1000,2); r['node']=node
            record('execution',r); return r
        time.sleep(.05)
    raise RuntimeError('Execution poll deadline exceeded; outcome uncertain; inspect events/node logs before retry')
def ok_run(code,fs,node=None,timeout=5):
    r=exec_js(code,fs,node,timeout)
    if r['status']!='completed' or not r.get('fs'): raise RuntimeError('Execution failed: '+json.dumps(r))
    return r
PATH='/work/allowed/notes.txt'
def write(text,path=PATH): return '(async()=>{await fs.writeFile('+json.dumps(path)+','+json.dumps(text)+');console.log(await fs.readFile('+json.dumps(path)+',"utf8"));})()'
def read(expected,path=PATH): return '(async()=>{const s=await fs.readFile('+json.dumps(path)+',"utf8");if(s!=='+json.dumps(expected)+')throw Error("readback mismatch");console.log(s);})()'
def state_data():
    p=STATE/'demo.json'; return json.loads(p.read_text()) if p.exists() else {}
def seed():
    label='walkthrough-'+uuid.uuid4().hex[:8]
    r=ok_run(write('alpha\nbeta\ngamma\n'),label)
    s={'label':label,'B':r['fs'],'base_text':'alpha\nbeta\ngamma\n'}
    n=leader(); code,result=request(n,'/api/fs/push',{'label':label,'ca_id':s['B']})
    if code!=200: raise RuntimeError('Publish base failed '+json.dumps(result))
    save(STATE/'demo.json',s); return record('seed',s)
def policy():
    s=state_data() or seed()
    ok_run(read(s['base_text']),s['B'])
    r=exec_js(write('must not be written','/work/denied/secret.txt'),s['B'])
    if r['status']!='failed': raise RuntimeError('STOP: denied write did not fail')
    # Source-derived gate error, not an arbitrary script or startup failure.
    err=str(r.get('error','')).lower()
    if 'den' not in err and 'policy' not in err: raise RuntimeError('Failure was not a policy denial: '+err)
    ok_run(read(s['base_text']),s['B'])
    return record('policy',{'allowed':'read/write /work/allowed/notes.txt','denied':r,'base_unchanged':True})
def branches():
    s=state_data() or seed(); start=time.monotonic()
    with concurrent.futures.ThreadPoolExecutor(2) as pool:
        a=pool.submit(ok_run,write('ALPHA\nbeta\ngamma\n'),s['B'])
        b=pool.submit(ok_run,write('alpha\nbeta\nGAMMA\n'),s['B'])
        s.update(A=a.result()['fs'],T=b.result()['fs'])
    ok_run(read(s['base_text']),s['B']); ok_run(read('ALPHA\nbeta\ngamma\n'),s['A']); ok_run(read('alpha\nbeta\nGAMMA\n'),s['T'])
    s['branch_elapsed_ms']=round((time.monotonic()-start)*1000,2); save(STATE/'demo.json',s)
    return record('branches',s)
def merge():
    s=state_data()
    if 'A' not in s: s=branches()
    n=leader(); start=time.monotonic()
    code,r=request(n,'/api/fs/merge',{'ours':s['A'],'theirs':s['T'],'base':s['B']})
    if code!=200 or 'ca_id' not in r: raise RuntimeError('Merge failed '+json.dumps(r))
    s['M']=r['ca_id']; s['merge_ms']=round((time.monotonic()-start)*1000,2)
    ok_run(read('ALPHA\nbeta\nGAMMA\n'),s['M'])
    code,p=request(n,'/api/fs/push',{'label':s['label'],'ca_id':s['M'],'expected':s['B']})
    if code!=200: raise RuntimeError('CAS publish failed '+json.dumps(p))
    code,reject=request(n,'/api/fs/push',{'label':s['label'],'ca_id':s['A'],'expected':s['B']})
    if code!=409: raise RuntimeError('STOP: stale CAS was not rejected '+json.dumps(reject))
    s['cas_rejection']=reject; save(STATE/'demo.json',s); return record('merge',s)
def conflict():
    s=state_data() or seed()
    a=ok_run(write('alpha\nOURS\ngamma\n'),s['B'])['fs']; b=ok_run(write('alpha\nTHEIRS\ngamma\n'),s['B'])['fs']
    code,r=request(leader(),'/api/fs/merge',{'ours':a,'theirs':b,'base':s['B']})
    if r.get('status')!='conflict': raise RuntimeError('Expected structured conflict '+json.dumps(r))
    return record('conflict',r)
def reset():
    s=state_data(); code,r=request(leader(),'/api/fs/reset',{'label':s['label'],'ca_id':s['B']})
    if code!=200: raise RuntimeError('Reset failed '+json.dumps(r))
    ok_run(read(s['base_text']),s['label']); return record('reset',r)
def kill_node(i):
    if i not in (1,2,3): raise RuntimeError('Only demo nodes 1..3')
    owned('container',name(i)); docker('kill','--signal=KILL',name(i)); return record('node_killed',{'node':i})
def restart_node(i):
    owned('container',name(i)); docker('start',name(i)); return record('node_restarted',{'node':i})
def outage():
    s=state_data()
    if 'B' not in s: s=seed()
    old=leader(); before=statuses(); kill_node(old); start=time.monotonic()
    try:
        new=leader(exclude=(old,)); election_ms=round((time.monotonic()-start)*1000,2)
        code,p=request(new,'/api/fs/push',{'label':'after-outage-'+uuid.uuid4().hex[:8],'ca_id':s['B']})
        if code!=200: raise RuntimeError('Write after one node loss failed '+json.dumps(p))
        ok_run(read(s['base_text']),s['B'],new)
        result={'stopped_node':old,'new_leader':new,'observed_election_ms':election_ms,'committed_write':p,'before':before,'during':statuses()}
    finally: restart_node(old)
    end=time.monotonic()+15
    while time.monotonic()<end:
        states=statuses()
        try:
            _,v=request(old,'/api/fs/labels/'+p['label'])
            if v.get('ca_id')==s['B']:
                result['recovered_label']=v; result['after']=states
                ok_run(read(s['base_text']),s['B'],old)
                return record('outage_recovery',result)
        except Exception: pass
        time.sleep(.2)
    raise RuntimeError('Restart recovery not observed by deadline')
def quorum():
    s=state_data() or seed(); keep=leader(); removed=[i for i in (1,2,3) if i!=keep]
    for i in removed: kill_node(i)
    label='uncertain-'+uuid.uuid4().hex[:8]
    try:
        start=time.monotonic(); code,r=request(keep,'/api/fs/push',{'label':label,'ca_id':s['B']},timeout=12)
        if code==200: raise RuntimeError('STOP: unexpected acknowledged write with one of three voters')
        result={'status':code,'response':r,'elapsed_ms':round((time.monotonic()-start)*1000,2),'label':label,'warning':'A timeout is an uncertain outcome; it may commit after quorum returns. No automatic retry.'}
    finally:
        for i in removed: restart_node(i)
    time.sleep(1); n=leader(); _,r=request(n,'/api/fs/labels/'+label)
    result['after_recovery_observation']=r
    return record('quorum_loss',result)

def up(binary):
    c=config(True); image=c['id']+':local'; network=c['id']
    # Probe ports before creating anything; never stop a conflicting process.
    for port in [38101,38102,38103,38201,38202,38203]:
        with socket.socket() as sock: sock.bind(('127.0.0.1',port))
    subprocess.run([sys.executable,str(HERE/'build-image.py'),'--binary',str(Path(binary).resolve()),'--context',str(STATE/'image'),'--tag',image],check=True)
    docker('network','create','--internal','--label','mcp-js-demo-owner='+c['id'],network)
    for v in ['objects','node1','node2','node3']:
        docker('volume','create','--label','mcp-js-demo-owner='+c['id'],name(v))
    common=['--detach','--env','NO_PROXY=*','--env','no_proxy=*','--env','HTTP_PROXY=','--env','HTTPS_PROXY=','--env','http_proxy=','--env','https_proxy=','--network',network,'--label','mcp-js-demo-owner='+c['id'],'--memory','512m','--cpus','1','--pids-limit','128','--cap-drop','ALL','--security-opt','no-new-privileges']
    docker('run',*common,'--name',name('objects'),'--network-alias','objects','--mount','type=volume,src='+name('objects')+',dst=/data','--entrypoint','/usr/bin/python3',image,'/object-store.py')
    for i in (1,2,3):
        env={'AWS_ENDPOINT_URL':'http://objects:9000','AWS_S3_FORCE_PATH_STYLE':'true','AWS_ACCESS_KEY_ID':'demo-only','AWS_SECRET_ACCESS_KEY':'demo-only','AWS_REGION':'us-east-1','AWS_EC2_METADATA_DISABLED':'true','AWS_REQUEST_CHECKSUM_CALCULATION':'WHEN_REQUIRED','AWS_RESPONSE_CHECKSUM_VALIDATION':'WHEN_REQUIRED'}
        envargs=sum((['--env',k+'='+v] for k,v in env.items()),[])
        peers=','.join(f'node{j}@node{j}:4000' for j in (1,2,3) if j!=i)
        docker('run',*common,*envargs,'--name',name(i),'--network-alias','node'+str(i),'-p',f'127.0.0.1:{38100+i}:3000','-p',f'127.0.0.1:{38200+i}:4000','--mount','type=volume,src='+name('node'+str(i))+',dst=/data',image,'--http-port','3000','--bind-host','0.0.0.0','--cluster-port','4000','--node-id','node'+str(i),'--advertise-addr',f'node{i}:4000','--peers',peers,'--session-db-path','/data/sessions','--heap-store','none','--fs-store','s3','--s3-bucket','demo','--cache-dir','/data/cache','--policies-json','/policies.json','--heap-memory-max','64')
    n=leader(25); return record('up',{'leader':n,'status':statuses(),'storage':'single local PUT/GET fixture; NOT replicated storage'})
def down():
    c=config(); failures=[]
    for x in [1,2,3,'objects']:
        try:
            owned('container',name(x)); docker('rm','-f',name(x))
        except Exception as e: failures.append(str(e))
    try: owned('network',c['id']); docker('network','rm',c['id'])
    except Exception as e: failures.append(str(e))
    return record('down',{'volumes_retained':True,'notes':failures})
def cleanup():
    down(); c=config(); removed=[]
    for x in ['node1','node2','node3','objects']:
        owned('volume',name(x)); docker('volume','rm',name(x)); removed.append(name(x))
    docker('image','rm',c['id']+':local',check=False)
    return record('cleanup',{'removed_volumes':removed,'evidence_retained':str(STATE)})
from alerts import Alerts
from observability import Observability
OBS=Observability(sys.modules[__name__])
ALERTS=Alerts(sys.modules[__name__])
ACTIONS={'obs-up':OBS.up,'query':OBS.query,'logs':OBS.logs,'dashboard':OBS.dashboard,'publish':OBS.publish,'seed':seed,'policy':policy,'branches':branches,'merge':merge,'conflict':conflict,'reset':reset,'outage':outage,'quorum':quorum,'status':statuses}
def serve(port):
    from pairing import Pairing
    pairing=Pairing(sys.modules[__name__])
    c=config(); origin=f'http://127.0.0.1:{port}'
    class Handler(BaseHTTPRequestHandler):
        def reply(self,code,obj):
            data=json.dumps(obj).encode(); self.send_response(code); self.send_header('Content-Type','application/json'); self.send_header('Content-Length',str(len(data))); self.end_headers(); self.wfile.write(data)
        def do_GET(self):
            if self.headers.get('Host') not in ('127.0.0.1:'+str(port),'localhost:'+str(port)): return self.reply(403,{'error':'loopback Host required'})
            if self.path=='/api/state': return self.reply(200,{'state':state_data(),'nodes':statuses(),'mode':'LIVE local controller','token':c['token']})
            if self.path=='/api/evidence':
                p=STATE/'events.jsonl'; return self.reply(200,[json.loads(x) for x in p.read_text().splitlines()] if p.exists() else [])
            files={'/animation/player.html':'animation/player.html','/animation/storyboard.json':'animation/storyboard.json','/animation/output/player-manifest.json':'animation/output/player-manifest.json','/':'slides.html','/slides.html':'slides.html','/recorded-evidence.json':'recorded-evidence.json'}
            if self.path.startswith('/animation/output/') and self.path.endswith('.mp4'):
                filename=self.path.rsplit('/',1)[1]
                if filename and all(c.isalnum() or c in '-_.' for c in filename) and '..' not in filename:files[self.path]='animation/output/'+filename
            if self.path not in files: return self.reply(404,{'error':'not found'})
            p=HERE/files[self.path]
            if not p.exists(): return self.reply(404,{'error':'not found'})
            data=p.read_bytes(); self.send_response(200); self.send_header('Content-Type','text/html; charset=utf-8' if p.suffix=='.html' else ('video/mp4' if p.suffix=='.mp4' else 'application/json')); self.send_header('Content-Length',str(len(data))); self.end_headers(); self.wfile.write(data)
        def do_POST(self):
            if self.headers.get('Host') not in ('127.0.0.1:'+str(port),'localhost:'+str(port)): return self.reply(403,{'error':'loopback Host required'})
            if self.path.startswith('/mcp/') or self.path.startswith('/api/pair/'):
                if self.headers.get('X-Demo-Token')!=c['token'] or self.headers.get('Origin') not in (None,origin):
                    return self.reply(403,{'error':'demo token required; external browser origins forbidden'})
                try:
                    n=int(self.headers.get('Content-Length','0'))
                    if n>65536: return self.reply(413,{'error':'body too large'})
                    body=json.loads(self.rfile.read(n))
                    with LOCK:
                        if self.path.startswith('/api/pair/'):
                            action=self.path.rsplit('/',1)[1]
                            result=pairing.merge(body) if action=='merge' else pairing.action(action,body)
                            return self.reply(200,result)
                        sid=self.path.rsplit('/',1)[1]; method=body.get('method'); params=body.get('params',{})
                        if method=='initialize': result={'protocolVersion':params.get('protocolVersion','2025-03-26'),'capabilities':{'tools':{}},'serverInfo':{'name':'mcp-js-paired-demo','version':'1.0'}}
                        elif method=='notifications/initialized':
                            self.send_response(202); self.send_header('Content-Length','0'); self.end_headers(); return
                        elif method=='tools/list': result={'tools':[{'name':'run_js','description':'Run code only in this Pi session filesystem branch. fs/session overrides forbidden. Returns terminal status and fs root.','inputSchema':{'type':'object','properties':{'code':{'type':'string'}},'required':['code'],'additionalProperties':False}},{'name':'branch_info','description':'Show this conversation filesystem binding','inputSchema':{'type':'object','properties':{}}}]}
                        elif method=='tools/call':
                            try:
                                value=pairing.call(sid,params['name'],params.get('arguments',{})); failed=value.get('status')=='failed'
                                result={'content':[{'type':'text','text':json.dumps(value)}],'structuredContent':value,'isError':failed}
                            except Exception as e: result={'content':[{'type':'text','text':str(e)}],'isError':True}
                        elif method=='ping': result={}
                        else: return self.reply(200,{'jsonrpc':'2.0','id':body.get('id'),'error':{'code':-32601,'message':'unsupported method'}})
                    return self.reply(200,{'jsonrpc':'2.0','id':body.get('id'),'result':result})
                except Exception as e: return self.reply(400,{'error':str(e)})
            if self.headers.get('Origin')!=origin or self.headers.get('X-Demo-Token')!=c['token']: return self.reply(403,{'error':'local origin and demo token required'})
            if self.path=='/api/alert':
                try:
                    n=int(self.headers.get('Content-Length','0'))
                    if n>8192: return self.reply(413,{'error':'alert too large'})
                    with LOCK: return self.reply(200,ALERTS.intake(self.rfile.read(n).decode()))
                except Exception as e: return self.reply(400,{'error':str(e)})
            action=self.path.removeprefix('/api/action/')
            if action not in ACTIONS: return self.reply(404,{'error':'unknown action'})
            if not LOCK.acquire(blocking=False): return self.reply(409,{'error':'another demo action is running'})
            try: self.reply(200,ACTIONS[action]())
            except Exception as e: record('error',{'action':action,'error':str(e)}); self.reply(500,{'error':str(e)})
            finally: LOCK.release()
    print('Local slides/controller: '+origin,flush=True)
    ThreadingHTTPServer(('127.0.0.1',port),Handler).serve_forever()
if __name__=='__main__':
    p=argparse.ArgumentParser(); p.add_argument('command',choices=['up','down','cleanup','serve','test',*ACTIONS]); p.add_argument('--binary',default=str(HERE.parents[1]/'target/debug/server')); p.add_argument('--port',type=int,default=38080); a=p.parse_args()
    try:
        if a.command=='up': result=up(a.binary)
        elif a.command=='serve': serve(a.port); result={}
        elif a.command=='down': result=down()
        elif a.command=='cleanup': result=cleanup()
        elif a.command=='test':
            result={}
            for action in ['seed','policy','branches','merge','conflict','reset','outage','quorum']:
                result[action]=ACTIONS[action](); print('PASS '+action,flush=True)
            save(STATE/'test-results.json',result)
        else: result=ACTIONS[a.command]()
        print(json.dumps(result,indent=2))
    except Exception as e: print('ERROR: '+str(e),file=sys.stderr); sys.exit(1)
