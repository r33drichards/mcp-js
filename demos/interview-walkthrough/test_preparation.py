#!/usr/bin/env python3
"""Regression tests use tiny local repositories; no downloads or Docker builds."""
import importlib.util,subprocess,tempfile,unittest
from pathlib import Path
import prepare
spec=importlib.util.spec_from_file_location('build_context',Path(__file__).with_name('build-context.py'));context=importlib.util.module_from_spec(spec);spec.loader.exec_module(context)
def git(root,*args):return subprocess.check_output(['git','-C',str(root),*args],stderr=subprocess.DEVNULL,text=True).strip()
class Preparation(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.root=Path(self.tmp.name);self.repo=self.root/'origin';self.repo.mkdir();git(self.repo,'init');(self.repo/'Dockerfile').write_text('FROM scratch\n');(self.repo/'keep.txt').write_text('committed source');git(self.repo,'add','.');git(self.repo,'-c','user.name=Test','-c','user.email=test@example.invalid','commit','-m','fixture');self.commit=git(self.repo,'rev-parse','HEAD');self.old=prepare.d.STATE;prepare.d.STATE=self.root/'state';prepare.d.STATE.mkdir()
 def tearDown(self):prepare.d.STATE=self.old;self.tmp.cleanup()
 def test_fresh_matching_head_materializes(self):
  result=prepare.checkout('browser',str(self.repo),self.commit);self.assertTrue((result/'Dockerfile').is_file())
 def test_existing_no_checkout_materializes(self):
  target=prepare.d.STATE/'browser';subprocess.run(['git','clone','--no-checkout',str(self.repo),str(target)],check=True,capture_output=True)
  self.assertFalse((target/'Dockerfile').exists());prepare.checkout('browser',str(self.repo),self.commit);self.assertTrue((target/'Dockerfile').exists())
 def test_modified_source_is_preserved_and_rejected(self):
  target=prepare.checkout('browser',str(self.repo),self.commit);(target/'keep.txt').write_text('user edit')
  with self.assertRaises(RuntimeError):prepare.checkout('browser',str(self.repo),self.commit)
  self.assertEqual((target/'keep.txt').read_text(),'user edit')
 def test_export_excludes_untracked_state_and_tracked_sensitive_paths(self):
  (self.repo/'.env').write_text('DUMMY_TRACKED_SECRET_FOR_TEST');git(self.repo,'add','.env');git(self.repo,'-c','user.name=Test','-c','user.email=test@example.invalid','commit','-m','sensitive fixture')
  state=self.repo/'demos/interview-walkthrough/.state';state.mkdir(parents=True);(state/'ownership.json').write_text('DUMMY_UNTRACKED_TOKEN_FOR_TEST');(self.repo/'keep.txt').write_text('dirty working tree')
  out=self.root/'export';context.export(self.repo,'HEAD',out)
  self.assertFalse((out/'.env').exists());self.assertFalse((out/'demos').exists());self.assertFalse((out/'.git').exists());self.assertEqual((out/'keep.txt').read_text(),'committed source')
  all_text=''.join(f.read_text() for f in out.rglob('*') if f.is_file());self.assertNotIn('DUMMY_',all_text)
 def test_root_dockerignore_protects_state(self):
  ignore=(Path(__file__).resolve().parents[2]/'.dockerignore').read_text()
  for rule in ['**/.state/**','**/.env','**/.aws/**','**/credentials.json']:self.assertIn(rule,ignore)
if __name__=='__main__':unittest.main()
