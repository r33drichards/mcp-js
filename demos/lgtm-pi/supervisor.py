import subprocess,signal,time,sys,os,socket,secrets
os.environ.setdefault('VNC_PASSWORD',secrets.token_urlsafe(24))
children=[]
def stop(*_):
 for p in children:p.terminate()
 for p in children:
  try:p.wait(timeout=8)
  except subprocess.TimeoutExpired:p.kill()
 sys.exit(0)
signal.signal(signal.SIGTERM,stop);signal.signal(signal.SIGINT,stop)
commands=[['/entrypoint'],['/minimal/mcp-grafana','--transport','streamable-http','--address','127.0.0.1:8000','--allowed-hosts','127.0.0.1:8000','--enabled-tools','prometheus,datasource,loki','--disable-write','--usage-stats','disabled']]
for port,policy,extra in [('3000','host',['--fs-store','none']),('3001','scratch',['--fs-store','dir','--fs-dir','/scratch/blobs'])]:
 commands.append(['/server','--http-port',port,'--bind-host','0.0.0.0','--heap-store','none','--session-db-path','/scratch/sessions-'+port,'--policies-json','/minimal/'+policy+'-policies.json','--mcp-config','/minimal/mcp.json',*extra])
for command in commands:
 if command[0]=='/server':
  for port in [8000,8081]:
   for attempt in range(120):
    try:
     with socket.create_connection(('127.0.0.1',port),timeout=1):break
    except OSError:
     if attempt==119:raise
     time.sleep(1)
 children.append(subprocess.Popen(command))
while all(p.poll() is None for p in children):time.sleep(1)
print('A required subprocess stopped; stopping container',flush=True);stop()
