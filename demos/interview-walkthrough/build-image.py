#!/usr/bin/env python3
"""Build a local scratch image from a supplied trusted Linux build; no pulls."""
import argparse, json, re, shutil, subprocess, sys, os
from pathlib import Path
p=argparse.ArgumentParser(); p.add_argument('--binary', required=True); p.add_argument('--context',required=True); p.add_argument('--tag',required=True); a=p.parse_args()
root=Path(a.context); root.mkdir(parents=True,exist_ok=True)
fs=root/'rootfs'; fs.mkdir(exist_ok=True)
def copy(source, dest=None):
    source=Path(source); target=fs/str(dest or source).lstrip('/')
    target.parent.mkdir(parents=True,exist_ok=True)
    if source.is_dir(): shutil.copytree(source,target,dirs_exist_ok=True,symlinks=False)
    else: shutil.copy2(source,target)
def libs(exe):
    result=subprocess.run(['ldd',str(exe)],capture_output=True,text=True)
    for name in re.findall(r'(/[^\s()]+)',result.stdout):
        if Path(name).is_file(): copy(name)
copy(Path(a.binary).resolve(),'/server'); libs(a.binary)
copy('/etc/ssl/certs/ca-certificates.crt')
py=Path('/usr/bin/python3').resolve()
if not py.exists(): raise SystemExit('Requires system /usr/bin/python3 for the isolated S3 fixture')
copy(py,'/usr/bin/python3'); libs(py)
version=subprocess.check_output([str(py),'-c','import sys;print(f"{sys.version_info.major}.{sys.version_info.minor}")'],text=True).strip()
stdlib=Path('/usr/lib')/('python'+version); copy(stdlib)
for lib in (stdlib/'lib-dynload').glob('*.so'): libs(lib)
copy(Path(__file__).with_name('object-store.py'),'/object-store.py')
copy(Path(__file__).with_name('filesystem.rego'),'/filesystem.rego')
(fs/'policies.json').write_text(json.dumps({'filesystem':{'policies':[{'url':'file:///filesystem.rego','rule':'data.mcp.filesystem.allow'}]}}))
(fs/'data').mkdir(exist_ok=True)
(fs/'tmp').mkdir(exist_ok=True); (fs/'tmp').chmod(0o1777)
(root/'Dockerfile').write_text('FROM scratch\nCOPY --chown=1000:1000 rootfs /\nENTRYPOINT ["/server"]\n')
os.environ['DOCKER_CONFIG']=str(root.parent/'docker-config')
subprocess.run(['docker','build','--network=none','--label','mcp-js-demo=interview','-t',a.tag,str(root)],check=True)
