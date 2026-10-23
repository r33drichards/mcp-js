#!/usr/bin/env python3
"""One real Astronomy checkout every 10s; finite budget, no fabricated telemetry."""
import json,time,uuid,urllib.request,urllib.error
BASE='http://frontend:8080'
def call(path,data=None):
 r=urllib.request.Request(BASE+path,None if data is None else json.dumps(data).encode(),headers={'Content-Type':'application/json'})
 try:
  with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(r,timeout=8) as response:return response.status,response.read()
 except urllib.error.HTTPError as e:return e.code,e.read()
# Same request shape and synthetic card fixture as upstream load-generator/people.json.
for i in range(180):
 started=time.monotonic();user='interview-'+str(uuid.uuid4())
 try:
  call('/api/products/0PUK6V6EV0')
  cart,_=call('/api/cart',{'item':{'productId':'0PUK6V6EV0','quantity':1},'userId':user})
  if cart>=400:raise RuntimeError('Cart failed before checkout: '+str(cart))
  status,_=call('/api/checkout',{'userId':user,'email':'rehearsal@example.invalid','address':{'streetAddress':'1 Synthetic Way','zipCode':'94043','city':'Mountain View','state':'CA','country':'United States'},'userCurrency':'USD','creditCard':{'creditCardNumber':'4432-8015-6152-0454','creditCardExpirationMonth':1,'creditCardExpirationYear':2039,'creditCardCvv':672}})
  print(json.dumps({'attempt':i+1,'checkout_http_status':status,'elapsed_seconds':round(time.monotonic()-started,3)}),flush=True)
 except Exception as e:print(json.dumps({'attempt':i+1,'error':str(e)}),flush=True)
 time.sleep(max(0,10-(time.monotonic()-started)))
print('Traffic budget complete: 180 attempts maximum. Restart only if another bounded run is needed.',flush=True)
