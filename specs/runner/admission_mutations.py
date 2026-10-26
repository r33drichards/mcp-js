#!/usr/bin/env python3
"""Fail named regressions using disposable unsafe production-source mutations."""
import pathlib,shutil,subprocess,tempfile
ROOT=pathlib.Path(__file__).resolve().parents[2]
runner='scheduler-runner/src/replicated.rs'; worker='server/src/scheduled_worker.rs'
mutations=[
 ('remove_fence',runner,[('if !running\n                || request.worker_incarnation.is_empty()', 'if request.worker_incarnation.is_empty()'),('                || request.dispatch.epoch != envelope.epoch\n',''),('                || request.dispatch.owner != envelope.owner\n','')],'takeover_rejects_stale_admission_but_preserves_previously_admitted_launch'),
 ('remove_dedup',runner,[('                || envelope.admissions.contains_key(&request.dispatch.key())\n','')],'admission_and_launch_bindings_duplicates_and_flush_failures'),
 ('reusable_launch',worker, [('''        self.db
            .compare_and_swap(key, Some(reservation), Some(b"consumed".as_slice()))
            .map_err(|e| e.to_string())?
            .map_err(|_| "launch permission already consumed".to_string())?;''','''        self.db.insert(key,b"consumed").map_err(|e|e.to_string())?;''')], 'takeover_rejects_stale_admission_but_preserves_previously_admitted_launch'),
 ('unflushed_launch',worker,[('''        self.flush()?;
        Ok(Launch {''','''        Ok(Launch {''')],'admission_and_launch_bindings_duplicates_and_flush_failures'),
]
for name,file,changes,test in mutations:
    with tempfile.TemporaryDirectory(prefix='admission-mutation-') as tmp:
        root=pathlib.Path(tmp)
        for directory in ['scheduler-runner','scheduler-core']:
            shutil.copytree(ROOT/directory,root/directory,ignore=shutil.ignore_patterns('target','.git'))
        for file_name in ['cluster.rs','cluster_regressions.rs','scheduled_protocol.rs','scheduled_worker.rs']:
            dest=root/'server/src'/file_name;dest.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(ROOT/'server/src'/file_name,dest)
        path=root/file;source=path.read_text()
        for old,new in changes:
            assert source.count(old)==1,(name,'mutation site drifted');source=source.replace(old,new)
        path.write_text(source)
        run=subprocess.run(['cargo','test','--locked','--manifest-path',str(root/'scheduler-runner/Cargo.toml'),'--lib',test,'--','--nocapture'],capture_output=True,text=True,timeout=240)
        log=run.stdout+run.stderr
        assert run.returncode==101 and 'test result: FAILED' in log and f'{test} ... FAILED' in log,(name,log[-6000:])
        print(name+': detected by '+test,flush=True)
