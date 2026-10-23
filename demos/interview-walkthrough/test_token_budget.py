"""Offline protocol/cost guards; no AWS, MCP or model traffic."""
import asyncio,copy,json,os,unittest
from pathlib import Path
from types import SimpleNamespace as NS
from unittest.mock import patch
from botocore.session import get_session
from botocore.validate import validate_parameters

nb=json.loads((Path(__file__).parent/'walkthrough.ipynb').read_text())
source=next(''.join(c['source']) for c in nb['cells'] if c['cell_type']=='code' and ''.join(c['source']).startswith('import asyncio'))
scope={'json':json,'os':os};exec(source,scope);loop=scope['investigation_loop']
ALERT={'labels':{'service':'checkout','alertname':'CheckoutErrors'}}
class Session:
 def __init__(self,size=10,schema_size=0):self.size=size;self.schema_size=schema_size
 async def list_tools(self):
  return NS(tools=[NS(name=n,description='schema marker'+'x'*self.schema_size,inputSchema={'type':'object','properties':{}}) for n in ['run_js','branch_info']])
 async def call_tool(self,name,arguments):
  return NS(isError=False,model_dump=lambda **kw:{'isError':False,'structuredContent':{'evidence':'x'*self.size}})
class Model:
 def __init__(self,counts=(100,100)):
  self.counts=iter(counts);self.count_inputs=[];self.calls=[];self.entered=asyncio.Event();self.block=None
 async def count_tokens(self,**kw):
  validate_parameters(kw,get_session().get_service_model('bedrock-runtime').operation_model('CountTokens').input_shape)
  self.count_inputs.append(copy.deepcopy(kw))
  if self.block=='count':self.entered.set();await asyncio.Event().wait()
  return {'inputTokens':next(self.counts)}
 async def converse(self,**kw):
  validate_parameters(kw,get_session().get_service_model('bedrock-runtime').operation_model('Converse').input_shape)
  self.calls.append(copy.deepcopy(kw))
  if self.block=='inference':self.entered.set();await asyncio.Event().wait()
  return {'output':{'message':{'role':'assistant','content':[{'toolUse':{'toolUseId':'one','name':'branch_info','input':{}}}]}},'stopReason':'tool_use','usage':{'inputTokens':1,'outputTokens':2,'totalTokens':3}}

class BudgetTests(unittest.IsolatedAsyncioTestCase):
 async def asyncSetUp(self):
  self.env=patch.dict(os.environ,{'BEDROCK_MODEL_ID':'offline-model'});self.env.start();self.addCleanup(self.env.stop)
 async def test_full_count_payload_matches_request_including_results(self):
  m=Model();await loop(m,Session(),ALERT,'SYSTEM marker',lambda x:None,max_steps=2)
  for count,call in zip(m.count_inputs,m.calls):
   self.assertEqual(count['modelId'],call['modelId'])
   self.assertEqual(count['input']['converse'],{k:call[k] for k in ['system','messages','toolConfig']})
  self.assertIn('toolResult',m.count_inputs[1]['input']['converse']['messages'][-1]['content'][0])
 async def test_first_full_input_over_budget_stops_without_inference(self):
  m=Model((8193,))
  with self.assertRaisesRegex(RuntimeError,'token budget'):await loop(m,Session(),ALERT,'system',lambda x:None)
  self.assertEqual(len(m.calls),0)
 async def test_cumulative_history_exceeds_budget_before_second_call(self):
  m=Model((5000,4000))
  with self.assertRaisesRegex(RuntimeError,'token budget'):await loop(m,Session(),ALERT,'system',lambda x:None)
  self.assertEqual(len(m.calls),1)
 async def test_oversized_tool_result_stops_before_recount(self):
  m=Model()
  with self.assertRaisesRegex(RuntimeError,'Tool result too large'):await loop(m,Session(40000),ALERT,'system',lambda x:None)
  self.assertEqual(len(m.calls),1);self.assertEqual(len(m.count_inputs),1)
 async def test_oversized_system_or_tool_schema_stops_before_count(self):
  for prompt,session in [('x'*120001,Session()),('system',Session(schema_size=70000))]:
   m=Model()
   with self.assertRaisesRegex(RuntimeError,'byte safety'):await loop(m,session,ALERT,prompt,lambda x:None)
   self.assertEqual(m.count_inputs,[]);self.assertEqual(m.calls,[])
 async def test_unsupported_counting_fails_closed(self):
  m=Model()
  async def denied(**kw):raise RuntimeError('CountTokens unsupported')
  m.count_tokens=denied
  with self.assertRaisesRegex(RuntimeError,'unsupported'):await loop(m,Session(),ALERT,'system',lambda x:None)
  self.assertEqual(m.calls,[])
 async def test_cancellation_while_counting_sends_no_inference(self):
  m=Model();m.block='count';task=asyncio.create_task(loop(m,Session(),ALERT,'system',lambda x:None))
  await m.entered.wait();task.cancel()
  with self.assertRaises(asyncio.CancelledError):await task
  self.assertEqual(m.calls,[])
 async def test_cancelled_inference_reserved_and_never_retried(self):
  m=Model();m.block='inference';events=[];task=asyncio.create_task(loop(m,Session(),ALERT,'system',events.append))
  await m.entered.wait();task.cancel()
  with self.assertRaises(asyncio.CancelledError):await task
  self.assertEqual(len(m.calls),1)
  self.assertEqual(next(e for e in events if 'inference_attempt' in e)['reserved_input_tokens'],100)
 async def test_six_attempt_limit(self):
  m=Model((10,)*6);await loop(m,Session(),ALERT,'system',lambda x:None)
  self.assertEqual(len(m.calls),6)
  self.assertTrue(all(c['inferenceConfig']['maxTokens']==1024 for c in m.calls))

if __name__=='__main__':unittest.main()
