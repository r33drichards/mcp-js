import asyncio,json,unittest
from pathlib import Path
from unittest.mock import patch
from types import SimpleNamespace as NS
import demo
from observability import dashboard_projection
from notebook_support import run_action

class RehearsalTests(unittest.TestCase):
 def test_quorum_restores_when_second_stop_fails(self):
  stopped=[];restored=[]
  def stop(i):
   stopped.append(i)
   if len(stopped)==2:raise RuntimeError('stop failed')
  with patch.object(demo,'config',return_value={}),patch.object(demo,'state_data',return_value={'B':'base'}),patch.object(demo,'leader',return_value=1),patch.object(demo,'kill_node',side_effect=stop),patch.object(demo,'restart_node',side_effect=restored.append):
   with self.assertRaisesRegex(RuntimeError,'stop failed'):demo.quorum()
  self.assertEqual(restored,[2,3])
 def test_policy_captures_allowed_write_and_keeps_base(self):
  with patch.object(demo,'state_data',return_value={'B':'base','base_text':'original'}),patch.object(demo,'ok_run',return_value={'fs':'allowed-root','status':'completed'}) as ok,patch.object(demo,'exec_js',return_value={'status':'failed','error':'policy denied'}),patch.object(demo,'record',side_effect=lambda event,data:data):
   result=demo.policy()
  self.assertEqual(result['allowed_write']['fs'],'allowed-root')
  self.assertEqual(ok.call_args_list[1].args[1],'allowed-root')
  self.assertEqual(ok.call_args_list[-1].args[1],'base')
 def test_same_title_changed_query_is_not_published(self):
  old={'uid':'mcp-anomaly','title':'same','panels':[{'id':1,'targets':[{'expr':'old'}]}]}
  new=json.loads(json.dumps(old));new['panels'][0]['targets'][0]['expr']='new'
  self.assertNotEqual(dashboard_projection(old),dashboard_projection(new))
 def test_notebook_has_no_saved_outputs_or_auto_live_call(self):
  nb=json.loads((Path(__file__).parent/'walkthrough.ipynb').read_text())
  for c in nb['cells']:
   if c['cell_type']=='code':
    self.assertEqual(c['outputs'],[])
    self.assertIsNone(c['execution_count'])
 def test_agent_loop_mock_only(self):
  # Offline mock validates protocol plumbing, NOT model or live MCP behavior.
  nb=json.loads((Path(__file__).parent/'walkthrough.ipynb').read_text())
  source=next(''.join(c['source']) for c in nb['cells'] if c['cell_type']=='code' and 'async def investigation_loop' in ''.join(c['source']))
  scope={'json':json,'os':__import__('os')};exec(source,scope)
  class Model:
   def __init__(self):self.n=0
   async def count_tokens(self,**kw):
    from botocore.session import get_session
    from botocore.validate import validate_parameters
    validate_parameters(kw,get_session().get_service_model('bedrock-runtime').operation_model('CountTokens').input_shape)
    return {'inputTokens':5}
   async def converse(self,**kw):
    self.n+=1
    from botocore.session import get_session
    from botocore.validate import validate_parameters
    validate_parameters(kw,get_session().get_service_model('bedrock-runtime').operation_model('Converse').input_shape)
    if self.n==2:assert kw['messages'][-1]['content'][0]['toolResult']['toolUseId']=='one'
    return {'output':{'message':{'role':'assistant','content':[{'toolUse':{'toolUseId':'one','name':'branch_info','input':{}}}] if self.n==1 else [{'text':'MOCK ONLY'}]}},'stopReason':'tool_use' if self.n==1 else 'end_turn','usage':{'inputTokens':5,'outputTokens':2,'totalTokens':7}}
  class Session:
   async def list_tools(self):return NS(tools=[NS(name=x,description='',inputSchema={'type':'object'}) for x in ['run_js','branch_info']])
   async def call_tool(self,name,arguments):return NS(isError=False,model_dump=lambda **kw:{'isError':False,'structuredContent':{'root':'MOCK'}})
  with patch.dict('os.environ',{'BEDROCK_MODEL_ID':'offline-mock'}):
   result=asyncio.run(scope['investigation_loop'](Model(),Session(),{'labels':{'service':'checkout','alertname':'CheckoutErrors'}},'test',lambda x:None))
  self.assertEqual(result[-2]['content'][0]['toolResult']['toolUseId'],'one')

if __name__=='__main__':unittest.main()
