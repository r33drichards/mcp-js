#!/usr/bin/env python3
"""Remove only resources matching this state's unique ownership label."""
import sys
import demo as d
c=d.config();selector='label=mcp-js-demo-owner='+c['id']
for cid in d.docker('ps','-aq','--filter',selector).split():
 d.owned('container',cid);d.docker('rm','-f',cid)
for nid in d.docker('network','ls','-q','--filter',selector).split():
 info=d.docker('network','inspect',nid)
 import json
 if json.loads(info)[0].get('Labels',{}).get('mcp-js-demo-owner')!=c['id']:raise RuntimeError('Not owned')
 d.docker('network','rm',nid)
if len(sys.argv)>1 and sys.argv[1]=='reset':
 for vol in d.docker('volume','ls','-q','--filter',selector).split():
  d.owned('volume',vol);d.docker('volume','rm',vol)
 print('Owned Docker volumes removed. Journal retained; use a NEW state directory for a new rehearsal.')
