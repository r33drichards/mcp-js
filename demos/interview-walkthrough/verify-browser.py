#!/usr/bin/env python3
"""Actual mcp-js -> upstream browser MCP. Screenshot requires human data review."""
import json,base64
import demo as d
from observability import Observability
s=d.state_data();root=s.get('dashboard_root')
if not root:raise SystemExit('Publish a selected root first')
args={'tab':'interview-dashboard','operations':[{'type':'setViewport','params':{'width':1440,'height':1000}},{'type':'navigate','params':{'url':'http://grafana:3000/d/mcp-anomaly'}},{'type':'wait','params':{'ms':3000}},{'type':'evaluate','params':{'script':'JSON.stringify({title:document.title,text:document.body.innerText,canvasCount:document.querySelectorAll("canvas").length})'}},{'type':'screenshot','params':{'fullPage':False}}]}
code='(async()=>{console.log(JSON.stringify(await mcp.callTool("browser","browser_execute",'+json.dumps(args)+')));})()'
r=d.ok_run(code,root,4,30);tool=json.loads(r['console']['data'])
if tool.get('isError'):raise SystemExit('Browser MCP failed: '+str(tool))
d.save(d.STATE/'browser-result.json',tool)
for c in tool.get('content',[]):
 if c.get('type')=='image':(d.STATE/'dashboard.png').write_bytes(base64.b64decode(c['data']))
print('Recorded actual browser response/screenshot. Inspect graph data, legends, time range and errors before marking handoff complete. Canvas count alone is insufficient.')
