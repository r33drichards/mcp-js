"""Untrusted alert intake for the alert-to-dashboard investigation."""
import json,time,uuid
class Alerts:
 def __init__(self,d): self.d=d
 def intake(self,text):
  if not isinstance(text,str) or len(text)>8192:raise ValueError('Alert must be JSON text <=8KiB')
  a=json.loads(text)
  labels=a.get('labels',{})
  if labels.get('service')!='checkout' or labels.get('alertname')!='CheckoutErrors':raise ValueError('Only CheckoutErrors for checkout is in demo scope')
  # annotations are retained as inert strings. No URL, query, command or path is executed.
  clean={'id':uuid.uuid4().hex,'labels':{'service':'checkout','alertname':'CheckoutErrors'},'annotations':{str(k)[:80]:str(v)[:1000] for k,v in a.get('annotations',{}).items()},'received':time.time(),'trusted_scope':'local alert-to-dashboard investigation only','plan':'query via Grafana MCP, save scratch evidence, stage dashboard, explicitly publish, verify graph rendering'}
  self.d.save(self.d.STATE/'alert.json',clean);return self.d.record('alert_received',clean)
