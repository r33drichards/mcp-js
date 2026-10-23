#!/usr/bin/env python3
"""Operator selects a bound session root for publication after explicit merge."""
import sys,json
import demo as d
if len(sys.argv)!=2:raise SystemExit('Usage: select-root.py PAIRED_SESSION_ID')
s=json.loads((d.STATE/'pairs.json').read_text());binding=s['sessions'][sys.argv[1]]
state=d.state_data();state['dashboard_root']=binding['root'];d.save(d.STATE/'demo.json',state)
d.record('operator_selected_publication_root',{'session':sys.argv[1],'root':binding['root']});print(binding['root'])
