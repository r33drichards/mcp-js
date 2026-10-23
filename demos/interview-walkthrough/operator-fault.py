#!/usr/bin/env python3
"""Operator-only fault injection. Never exposed as an agent tool."""
import sys,json,time
import demo as d
if len(sys.argv)!=2 or sys.argv[1] not in ['on','off']:raise SystemExit('Usage: operator-fault.py on|off')
d.owned('container',d.name('flagd'))
p=d.STATE/'flagd/demo.flagd.json'
if not p.exists():raise SystemExit('Run Astronomy preparation first')
s=json.loads(p.read_text());s['flags']['paymentFailure']['defaultVariant']='100%' if sys.argv[1]=='on' else 'off'
# Preserve inode: flagd bind-mount watches the prepared file, never upstream source.
p.write_text(json.dumps(s,indent=2)+'\n')
d.record('operator_fault',{'flag':'paymentFailure','variant':s['flags']['paymentFailure']['defaultVariant'],'time':time.time(),'not_agent_remediation':True})
print('Flag file updated; verify flagd reload and actual telemetry before claiming injection took effect.')
