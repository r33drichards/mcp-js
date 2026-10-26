#!/usr/bin/env python3
"""Negative controls in disposable source copies; never patch the working tree."""
import os, pathlib, shutil, subprocess, tempfile
ROOT = pathlib.Path(__file__).resolve().parents[2]
source = (ROOT/'server/src/cluster.rs').read_text()
ack = '''                if state.role != Role::Leader || state.current_term != entry.term {
                    return Err("lost proposal leadership; outcome may be unknown".into());
                }
                if state.log.get((entry.index - 1) as usize) != Some(entry) {
                    return Err("proposal replaced".into());
                }
'''
prefix = '''                    .is_some_and(|e| e.term == req.prev_log_term))
        {
'''
term = '                || expected_term.is_some_and(|term| term != state.current_term)'
mutations = [
 ('index_only_ack', ack, '', 'replaced_claim_must_not_report_committed'),
 ('truncate_prefix', prefix, prefix + '''            state.log.truncate(req.prev_log_index.saturating_sub(1) as usize);
            self.truncate_log_from(req.prev_log_index).unwrap();
''', 'committed_receipt_must_not_be_truncated_by_inconsistent_prefix'),
 ('remove_term_fence', term, '', 'scheduler_proposal_checks_expected_term_under_append_lock'),
]
for name, old, new, test in mutations:
    assert source.count(old)==1, f'{name}: mutation site drifted'
    with tempfile.TemporaryDirectory(prefix='scheduler-mutation-') as directory:
        target=pathlib.Path(directory)
        for file in ['server/src/cluster.rs','server/src/cluster_regressions.rs','server/cluster-tests/Cargo.toml','server/cluster-tests/Cargo.lock','server/cluster-tests/src/lib.rs','server/tests/cluster_learner.rs','server/tests/cluster_simulation.rs']:
            dest=target/file; dest.parent.mkdir(parents=True,exist_ok=True); shutil.copy2(ROOT/file,dest)
        (target/'server/src/cluster.rs').write_text(source.replace(old,new))
        result=subprocess.run(['cargo','test','--locked','--manifest-path',str(target/'server/cluster-tests/Cargo.toml'),test,'--','--nocapture'],capture_output=True,text=True,timeout=180)
        log=result.stdout+result.stderr
        assert result.returncode==101 and 'test result: FAILED' in log and f'{test} ... FAILED' in log, f'{name}: expected assertion failure, got:\n{log}'
        print(f'{name}: detected by {test}',flush=True)
