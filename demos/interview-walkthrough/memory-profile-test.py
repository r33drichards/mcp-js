#!/usr/bin/env python3
"""Validate generated staged topology without starting any workload."""
import json,os,unittest
from pathlib import Path
import yaml
from unittest.mock import patch
import demo
STATE=Path(os.environ['MCP_JS_DEMO_STATE'])
class StagedProfile(unittest.TestCase):
 def setUp(self):self.c=json.loads((STATE/'compose.investigation.json').read_text())
 def test_dependency_closure(self):
  services=self.c['services']
  for n,s in services.items():self.assertTrue(set(s.get('depends_on',{}))<=set(services),n)
 def test_no_fake_telemetry(self):
  c=yaml.safe_load((STATE/'collector.investigation.yml').read_text())
  self.assertEqual(c['service']['pipelines']['traces']['exporters'],['spanmetrics'])
  self.assertEqual(c['service']['pipelines']['logs']['exporters'],['otlphttp/loki'])
  self.assertEqual(set(c['service']['pipelines']['metrics']['receivers']),{'otlp','spanmetrics'})
  self.assertNotIn('KAFKA_ADDR',self.c['services']['checkout']['environment'])
 def test_stock_caps_unchanged_and_bounded_support(self):
  full=json.loads((STATE/'compose.json').read_text());services=self.c['services']
  for n,s in services.items():
   cap=int(s.get('mem_limit',s.get('deploy',{}).get('resources',{}).get('limits',{}).get('memory',0)));self.assertGreater(cap,0,n)
   if n in full['services'] and full['services'][n].get('deploy',{}).get('resources',{}).get('limits',{}).get('memory'):
    self.assertEqual(s['deploy']['resources']['limits']['memory'],full['services'][n]['deploy']['resources']['limits']['memory'],n)
 def test_one_voter_has_separate_storage_and_browser_guard(self):
  services=self.c['services'];self.assertNotIn('node2',services);self.assertNotIn('node3',services);self.assertNotIn('query',services)
  args=services['node1']['command'];self.assertEqual(args[args.index('--peers')+1],'')
  self.assertEqual(services['node1']['volumes'][0]['source'],'solo')
  self.assertEqual(services['node1']['network_mode'],services['browser']['network_mode'])
  self.assertEqual(services['node1']['depends_on']['browser']['condition'],'service_healthy')
 def test_single_phase_rejects_cluster_failure_actions(self):
  with patch.object(demo,'config',return_value={'id':'test','node_ids':[1],'query_node':1}):
   self.assertEqual(demo.name(4),'test-1')
   for action in [demo.outage,demo.quorum]:
    with self.assertRaisesRegex(RuntimeError,'three-voter'):action()
 def test_second_phase_is_separate_three_voters(self):
  c=json.loads((STATE/'compose.cluster.json').read_text());self.assertEqual(set(c['services']),{'node1','node2','node3','objects'})
  for n in ['node1','node2','node3']:self.assertEqual(c['services'][n]['volumes'][0]['source'],n)
if __name__=='__main__':unittest.main()
