#!/usr/bin/env python3
"""Resolve external release tags to digests before starting this generated project."""
import json,subprocess
import demo as d
p=d.STATE/'compose.json';c=json.loads(p.read_text());lock={}
for name,s in c['services'].items():
 image=s.get('image')
 if not image or 'build' in s or image==d.config()['id']+':local':continue
 if '@sha256:' not in image:
  d.docker('pull',image)
  info=json.loads(d.docker('image','inspect',image))[0];digests=info.get('RepoDigests') or []
  if not digests:raise RuntimeError('No immutable digest for '+image)
  s['image']=digests[0]
 lock[name]=s['image']
d.save(p,c);p.chmod(0o600);d.save(d.STATE/'images.lock.json',lock)
