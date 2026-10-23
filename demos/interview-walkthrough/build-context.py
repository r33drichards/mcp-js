#!/usr/bin/env python3
"""Export a commit-only Docker context; never send the working tree or demo state."""
import argparse,io,json,subprocess,tarfile
from pathlib import Path,PurePosixPath
BLOCKED={'.state','.git','.aws','.ssh','.pi','.venv','node_modules','credentials','credentials.json','auth.json','id_rsa','id_ed25519'}
def excluded(name):
 parts=PurePosixPath(name).parts
 return any(p in BLOCKED or p=='.env' or p.startswith('.env.') for p in parts)
def export(repo,revision,output):
 commit=subprocess.check_output(['git','-C',str(repo),'rev-parse',revision+'^{commit}'],text=True).strip()
 output=Path(output)
 if output.exists() and any(output.iterdir()):raise RuntimeError('Build export destination must be empty')
 output.mkdir(parents=True,exist_ok=True)
 raw=subprocess.check_output(['git','-C',str(repo),'archive','--format=tar',commit])
 with tarfile.open(fileobj=io.BytesIO(raw)) as archive:
  members=[]
  for member in archive.getmembers():
   path=PurePosixPath(member.name)
   if path.is_absolute() or '..' in path.parts:raise RuntimeError('Unsafe archive path')
   if not excluded(member.name):members.append(member)
  archive.extractall(output,members=members,filter='data')
 (output/'build-source.json').write_text(json.dumps({'commit':commit,'source':'git archive; untracked/ignored working-tree files excluded','sensitive_paths':'excluded even if tracked'},indent=2)+'\n')
 return commit
if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('--repo',type=Path,required=True);p.add_argument('--revision',default='HEAD');p.add_argument('--output',type=Path,required=True);a=p.parse_args();print(export(a.repo,a.revision,a.output))
