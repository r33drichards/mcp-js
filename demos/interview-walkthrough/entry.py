#!/usr/bin/env python3
"""Path-independent entry points; doctor never mutates state or calls models."""
import os,sys,json,pathlib,urllib.request,shutil
HERE=pathlib.Path(__file__).resolve().parent
STATE=pathlib.Path(os.environ.get('MCP_JS_DEMO_STATE',HERE/'.state')).expanduser().resolve()
os.chdir(HERE)
os.environ['MCP_JS_DEMO_STATE']=str(STATE)
python=STATE/'notebook-venv/bin/python'
command=sys.argv[1] if len(sys.argv)>1 else 'doctor'
if command=='doctor':
 owner=json.loads((STATE/'ownership.json').read_text())
 print('Demo:',HERE,'\nState:',STATE,'\nProject:',owner['id'])
 print('Python:',python if python.exists() else sys.executable)
 print('Pi installed:',(HERE/'pi/node_modules/.bin/pi').exists())
 for port in (38080,38300,38880):
  with urllib.request.urlopen('http://127.0.0.1:'+str(port),timeout=5) as r:print('http://127.0.0.1:'+str(port),r.status)
 print('Read-only check complete; no model or state mutation.')
elif command in ('pi','python'):
 if command=='pi':
  owner=json.loads((STATE/'ownership.json').read_text())
  os.environ.setdefault('MCP_JS_DEMO_TOKEN',owner['token'])
  binary=HERE/'pi/node_modules/.bin/pi'
  if not binary.exists():raise SystemExit('Pi is not installed in this checkout; see README preparation.')
  if not shutil.which('node'):
   nodes=sorted((HERE.parents[2]/'tools').glob('node-*/bin/node'))
   if not nodes:raise SystemExit('Node is unavailable; set PATH to the approved installed Node runtime.')
   os.environ['PATH']=str(nodes[-1].parent)+os.pathsep+os.environ.get('PATH','')
  os.chdir(HERE/'pi');os.execv(str(binary),[str(binary),'--tools','codemode','--extension','./paired-fork.ts',*sys.argv[2:]])
 else:
  binary=str(python) if python.exists() else sys.executable
  os.execv(binary,[binary,*sys.argv[2:]])
else:raise SystemExit('Expected doctor, pi, or python')
