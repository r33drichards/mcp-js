#!/usr/bin/env python3
"""Local replay exporter for a pinned real NAB CPU series. No remediation API.
Samples are scraped NOW; original naive timestamps are provenance, not UTC.
"""
import csv,json,os,time
from pathlib import Path
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
rows=list(csv.DictReader(Path(os.environ.get('NAB_CSV','/data.csv')).open()))
START=int(os.environ.get('REPLAY_START_ROW','1400')); STRIDE=float(os.environ.get('REPLAY_ROWS_PER_SECOND','10')); started=time.monotonic()
def sample():
 i=min(len(rows)-1,START+int((time.monotonic()-started)*STRIDE));r=rows[i]
 return {'row':i,'value':float(r['value']),'source_timestamp':r['timestamp'],'source_timezone':'unspecified','scrape_clock':'present time','kind':'historical NAB replay; root cause not established'}
class H(BaseHTTPRequestHandler):
 def reply(self,body,kind='text/html'):
  b=body.encode();self.send_response(200);self.send_header('Content-Type',kind);self.send_header('Content-Length',str(len(b)));self.end_headers();self.wfile.write(b)
 def do_GET(self):
  s=sample()
  if self.path=='/metrics':
   self.reply('# TYPE demo_cpu_utilization_percent gauge\ndemo_cpu_utilization_percent '+str(s['value'])+'\n# TYPE demo_source_row gauge\ndemo_source_row '+str(s['row'])+'\n','text/plain; version=0.0.4');return
  if self.path=='/provenance':self.reply(json.dumps(s),'application/json');return
  self.reply('''<!doctype html><meta charset="utf-8"><title>NAB CPU replay</title><style>body{font:24px system-ui;margin:4em;background:#101820;color:#e4f2e8}pre{white-space:pre-wrap}</style><h1>Historical CPU replay</h1><p>Real NAB observations, accelerated onto the present scrape clock. This is not live AWS and not a proven outage.</p><pre id="result"></pre><script>async function update(){let r=await fetch('/provenance');document.querySelector('pre').textContent=JSON.stringify(await r.json(),null,2)}update();setInterval(update,1000)</script>''')
ThreadingHTTPServer(('0.0.0.0',8000),H).serve_forever()
