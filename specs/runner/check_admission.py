#!/usr/bin/env python3
"""Run bounded TLC checks and export normalized generated traces for Rust replay."""
import argparse, hashlib, json, pathlib, re, subprocess, tempfile
ROOT = pathlib.Path(__file__).resolve().parent
# Official v1.8.0 release asset, verified against GitHub's published SHA-256 digest.
SHA = 'd15655eeaff4b3f7b3d65c46b838bc0f35d131b2ef4b1e473689b62c98d4726f'
p = argparse.ArgumentParser(); p.add_argument('--jar', required=True); p.add_argument('--out', default=str(ROOT/'admission-generated')); p.add_argument('--deep', action='store_true'); args=p.parse_args()
jar=pathlib.Path(args.jar).resolve(); assert hashlib.sha256(jar.read_bytes()).hexdigest()==SHA, 'Unexpected TLC binary: pin/check the tool before changing this checksum'
out=pathlib.Path(args.out).resolve(); out.mkdir(parents=True,exist_ok=True)
steps,faults=(14,0) if args.deep else (10,0)
ops=['claim','send','duplicate','drop','reserve','reject_duplicate','admit','lost_ack','uncertain','consume','launch','crash','restart','partition','heal','takeover','owner_crash']
summary=[]
def run(name, *, target='none', invariant=None, replay=False, index=False):
    with tempfile.TemporaryDirectory() as scratch:
        scratch=pathlib.Path(scratch)
        cfg=scratch/'run.cfg'
        invariants='NoStaleAdmission OneAdmission OneLaunch LaunchRequiresAdmission'
        if invariant: invariants+=' '+invariant
        cfg.write_text(f'SPECIFICATION Spec\nCONSTANTS MaxSteps = {steps} SkipEpoch = {str(replay).upper()} SkipDedup = {str(index).upper()} Target = "{target}"\nINVARIANTS {invariants}\nCHECK_DEADLOCK FALSE\n')
        raw=out/f'{name}.tlc.json'
        cmd=['java','-XX:+UseParallelGC','-Xmx2g','-cp',str(jar),'tlc2.TLC','-workers','1','-seed','1','-fp','0','-noGenerateSpecTE','-metadir',str(scratch/'states'),'-dumpTrace','json',str(raw),'-config',str(cfg),str(ROOT/'RunnerAdmission.tla')]
        result=subprocess.run(cmd,capture_output=True,text=True,timeout=180 if args.deep else 60)
        log=result.stdout+result.stderr; (out/f'{name}.log').write_text(log)
        expected='NoStaleAdmission' if replay else 'OneAdmission' if index else invariant
        if expected:
            assert result.returncode==12 and f'Invariant {expected} is violated.' in log, (name,result.returncode,log[-2000:])
            data=json.loads(raw.read_text()); edges=data['counterexample']['action']; states=[edges[0][0][1]]+[e[2][1] for e in edges]
            normalized={'format':1,'model':'RunnerAdmission','name':name,'seed':1,'max_steps':steps,'max_faults':faults,'expected_violation':expected,'mutation': 'skip_epoch' if replay else 'skip_dedup' if index else None,'states':states}
            (out/f'{name}.json').write_text(json.dumps(normalized,indent=2)+'\n')
        else: assert result.returncode==0 and 'No error has been found.' in log, (name,result.returncode,log[-2000:])
        match=re.search(r'([\d,]+) states generated, ([\d,]+) distinct states found',log)
        summary.append({'case':name,'expected':expected or 'safe','generated':int(match[1].replace(',','')),'distinct':int(match[2].replace(',',''))})
        print(name,summary[-1],flush=True)
run('safe')
for op in ops + ['late_launch','stale_rejected','lost_ack_takeover','consumed_crash','reserved_restart']: run('witness_'+op,target=op,invariant='WitnessAbsent')
run('diagnostic_late_launch',invariant='NoLateLaunchDiagnostic')
run('mutant_skip_epoch',replay=True)
run('mutant_skip_dedup',index=True)
(out/'summary.json').write_text(json.dumps({'tlc_sha256':SHA,'model_sha256':hashlib.sha256((ROOT/'RunnerAdmission.tla').read_bytes()).hexdigest(),'seed':1,'max_steps':steps,'cases':summary},indent=2)+'\n')
